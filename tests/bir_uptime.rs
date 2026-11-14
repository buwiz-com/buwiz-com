//! BIR uptime's record, end to end through the Gatekeeper's bridge: a fake
//! Sender (a loopback WebSocket where `SENDER_URL` points) scripts the
//! frames of a send, a fake app answers them, and each try leaves one
//! classified `bir_tries` row. Only loopback connections, made-up hosts
//! (`dispatcher.test`, `backup.test`, `upload.test`) and made-up data.
//!
//! What an app hears does not change below `BIR_UPTIME=public`, and from
//! there only by `class` added at the end of `result`; the Sender's
//! `sender` frame never reaches it.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
use axum::response::Response;
use base64::Engine;
use buwiz_cloud::base::crypto;
use buwiz_cloud::base::state::AppState;
use buwiz_cloud::relay::Control;
use buwiz_cloud::sender::{self, Mode as RelayMode};
use buwiz_cloud::uptime::keys::{MANILA, Network};
use buwiz_cloud::uptime::observe::TryObserver;
use buwiz_cloud::uptime::store::{self, RelayTry};
use buwiz_cloud::uptime::{Level, Uptime};
use common::{ACCOUNT_HOST, Site, TestDevice, add_claim};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

const SECRET: &str = "a made-up Sender secret, 32 characters or more";
const ROOT: &str = "000123453";
const EMAIL: &str = "ana@example.test";
const REFUSED: &str = "Connection refused (os error 61)";
const HOME: &str = "203.0.113.7";

/// One step of the fake Sender, after it read the job.
#[derive(Clone, Debug)]
enum Step {
    Text(String),
    /// Bytes from the BIR's side on a stream: `[stream][bytes]`.
    Binary(u8, Vec<u8>),
    /// Waits for this many frames from the bridge (each at most 2 s).
    Expect(usize),
    /// Ends the socket without a result.
    Drop,
}

fn hello(mode: RelayMode) -> Step {
    Step::Text(sender::sender_frame(mode))
}

fn open(stream: u8, host: &str, port: u16) -> Step {
    Step::Text(serde_json::to_string(&Control::Open { stream, host: host.into(), port }).unwrap())
}

fn result(status: &str, error: Option<&str>, receipt: Option<String>) -> Step {
    let control =
        Control::Result { status: status.into(), error: error.map(Into::into), receipt: receipt.map(Value::String) };
    Step::Text(serde_json::to_string(&control).unwrap())
}

fn receipt() -> String {
    let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
    let claims =
        json!({ "mode": "live", "status": "sent", "remote_path": "/1601C/made-up.xml", "payload_sha256": "ab" });
    buwiz_cloud::filing::jws::sign(&key, sender::RECEIPT_TYP, &claims)
}

/// The fake Sender: each job's socket runs the current script.
#[derive(Clone, Default)]
struct FakeSender {
    script: Arc<Mutex<Vec<Step>>>,
    /// The text frames it got from the bridge, newest job last.
    heard: Arc<Mutex<Vec<String>>>,
}

async fn fake_relay(State(fake): State<FakeSender>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |mut socket| async move {
        let _job = socket.recv().await;
        let script = fake.script.lock().unwrap().clone();
        for step in script {
            match step {
                Step::Text(text) => {
                    let _ = socket.send(WsMessage::Text(text.into())).await;
                }
                Step::Binary(stream, bytes) => {
                    let _ = socket.send(WsMessage::Binary([&[stream], bytes.as_slice()].concat().into())).await;
                }
                Step::Expect(count) => {
                    for _ in 0..count {
                        match tokio::time::timeout(Duration::from_secs(2), socket.recv()).await {
                            Ok(Some(Ok(WsMessage::Text(text)))) => fake.heard.lock().unwrap().push(text.to_string()),
                            Ok(Some(Ok(_))) => {}
                            _ => break,
                        }
                    }
                }
                Step::Drop => return,
            }
        }
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await {}
    })
}

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

/// How the fake app answers an `open` for a port.
#[derive(Clone)]
enum Answer {
    /// Connected; the far end then sends these bytes.
    Open(Vec<u8>),
    Refuse(&'static str, Option<&'static str>),
    /// Goes away instead of answering.
    Leave,
}

/// What the app got, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Got {
    Text(String),
    Binary(Vec<u8>),
}

struct World {
    db: PgPool,
    account: Uuid,
    claim: Uuid,
    device: TestDevice,
}

impl World {
    async fn new(db: PgPool) -> Self {
        let account: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) RETURNING id")
            .bind(EMAIL)
            .fetch_one(&db)
            .await
            .unwrap();
        let claim = add_claim(&db, account, ROOT, "approved").await;
        let device = TestDevice::new(&db, account).await;
        Self { db, account, claim, device }
    }

    /// A v1 return waiting to be sent.
    async fn v1(&self) -> Uuid {
        let id = Uuid::new_v4();
        let xml = b"<made-up return/>";
        let (nonce, ciphertext) = crypto::seal(&[7; 32], format!("submission/{id}").as_bytes(), xml).unwrap();
        sqlx::query(
            "INSERT INTO submissions (id, account_id, claim_id, form_type, period, filename, xml_sha256, xml_nonce,
                 xml_ciphertext, status)
             VALUES ($1, $2, $3, '1601Cv2018', '102026', $4, $5, $6, $7, 'ready')",
        )
        .bind(id)
        .bind(self.account)
        .bind(self.claim)
        .bind(format!("{ROOT}00000-1601Cv2018-102026#{EMAIL}#.xml"))
        .bind(crypto::sha256(xml))
        .bind(nonce)
        .bind(ciphertext)
        .execute(&self.db)
        .await
        .unwrap();
        id
    }

    /// A v2 return, sealed, with its ticket's admission.
    async fn v2(&self, admission: Option<&str>) -> Uuid {
        let id = Uuid::new_v4();
        let jti = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tickets (jti, account_id, device_id, claim_id, form_type, period, payload_sha256, email,
                 idempotency_key, jws, expires_at, used_at, bir_admission)
             VALUES ($1, $2, $3, $4, '1601Cv2018', '102026', '\\x00', $5, $6, 'made.up.ticket',
                 now() + interval '5 minutes', now(), $7)",
        )
        .bind(jti)
        .bind(self.account)
        .bind(self.device.device_id)
        .bind(self.claim)
        .bind(EMAIL)
        .bind(jti.to_string())
        .bind(admission)
        .execute(&self.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO submissions (id, account_id, claim_id, form_type, period, filename, xml_sha256, status,
                 ticket_jti, device_id, envelope)
             VALUES ($1, $2, $3, '1601Cv2018', '102026', $4, '\\x00', 'ready', $5, $6, $7)",
        )
        .bind(id)
        .bind(self.account)
        .bind(self.claim)
        .bind(format!("{ROOT}00000-1601Cv2018-102026#{EMAIL}#.xml"))
        .bind(jti)
        .bind(self.device.device_id)
        .bind(r#"{"kid":"made-up","enc":"AA","ct":"AA"}"#)
        .execute(&self.db)
        .await
        .unwrap();
        id
    }
}

/// A Gatekeeper on loopback whose Sender is a fake one (or nothing).
struct Gatekeeper {
    base: String,
    sender: FakeSender,
    state: AppState,
    /// How long the last send took until the app had its `result`.
    result_after: Mutex<Option<Duration>>,
}

impl Gatekeeper {
    async fn new(world: &World, level: Level) -> Self {
        Self::with_ports(world, level, vec![22]).await
    }

    /// With `ports` as the upload window's.
    async fn with_ports(world: &World, level: Level, ports: Vec<u16>) -> Self {
        let sender = FakeSender::default();
        let router =
            axum::Router::new().route("/internal/relay", axum::routing::get(fake_relay)).with_state(sender.clone());
        let sender_url = serve(router).await;
        Self::at(world, Uptime::new(level, ports, false, OffsetDateTime::now_utc()), &sender_url, sender).await
    }

    /// With the Sender at `sender_url`.
    async fn at(world: &World, uptime: Uptime, sender_url: &str, sender: FakeSender) -> Self {
        let site = Site::with_sender(world.db.clone(), sender_url, SECRET, uptime);
        let base = serve(site.router).await;
        Self { base, sender, state: site.state, result_after: Mutex::new(None) }
    }

    /// Opens the relay for `id` as the app does, from `ip`, answering each
    /// `open` by its port, and gives what the app got, once the try's
    /// record is written.
    async fn send(&self, world: &World, id: Uuid, script: Vec<Step>, answers: &[(u16, Answer)], ip: &str) -> Vec<Got> {
        let got = self.send_only(world, id, script, answers, ip).await;
        self.state.uptime.recorder.settled().await;
        got
    }

    /// [`Gatekeeper::send`] without waiting for the record.
    async fn send_only(
        &self,
        world: &World,
        id: Uuid,
        script: Vec<Step>,
        answers: &[(u16, Answer)],
        ip: &str,
    ) -> Vec<Got> {
        *self.sender.script.lock().unwrap() = script;
        let answers: HashMap<u16, Answer> = answers.iter().cloned().collect();
        let path = format!("/api/submissions/{id}/relay");
        let mut request = format!("{}{path}", self.base.replacen("http", "ws", 1)).into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert("host", ACCOUNT_HOST.parse().unwrap());
        headers.insert("authorization", format!("Bearer {}", world.device.token).parse().unwrap());
        headers.insert("cf-connecting-ip", ip.parse().unwrap());
        headers.insert("buwiz-client", "desktop/0.2.0".parse().unwrap());
        for (name, value) in world.device.signature("GET", &path, b"") {
            headers.insert(name, value.parse().unwrap());
        }
        let started = Instant::now();
        *self.result_after.lock().unwrap() = None;
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let mut got = Vec::new();
        while let Ok(Some(Ok(message))) = tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
            match message {
                Message::Text(text) => {
                    got.push(Got::Text(text.to_string()));
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    if frame["type"] == "result" {
                        *self.result_after.lock().unwrap() = Some(started.elapsed());
                    }
                    if frame["type"] != "open" {
                        continue;
                    }
                    let stream = frame["stream"].as_u64().unwrap() as u8;
                    let port = frame["port"].as_u64().unwrap() as u16;
                    match answers.get(&port).cloned().unwrap_or(Answer::Refuse(REFUSED, None)) {
                        Answer::Open(bytes) => {
                            let opened = json!({ "type": "opened", "stream": stream }).to_string();
                            socket.send(Message::Text(opened.into())).await.unwrap();
                            if !bytes.is_empty() {
                                socket
                                    .send(Message::Binary([&[stream], bytes.as_slice()].concat().into()))
                                    .await
                                    .unwrap();
                            }
                        }
                        Answer::Refuse(reason, kind) => {
                            let mut refused = json!({ "type": "refused", "stream": stream, "reason": reason });
                            if let Some(kind) = kind {
                                refused["kind"] = json!(kind);
                            }
                            socket.send(Message::Text(refused.to_string().into())).await.unwrap();
                        }
                        Answer::Leave => {
                            let _ = socket.close(None).await;
                            break;
                        }
                    }
                }
                Message::Binary(bytes) => got.push(Got::Binary(bytes.to_vec())),
                Message::Close(_) => break,
                _ => {}
            }
        }
        got
    }
}

/// A row of `bir_tries`, as the tests read it.
#[derive(Debug, sqlx::FromRow)]
struct TryRow {
    mode: String,
    outcome: String,
    stage: String,
    dispatcher: String,
    upload: String,
    failure_class: Option<String>,
    failure_kind: Option<String>,
    touched_bir: bool,
    opens: i16,
    admission: String,
    counted: bool,
    client: Option<String>,
    account_key: Vec<u8>,
    network_key: Option<Vec<u8>>,
    key_epoch: i32,
    started_at: OffsetDateTime,
    ended_at: OffsetDateTime,
}

impl TryRow {
    /// mode, outcome, stage, dispatcher, upload, class, kind, counted.
    fn summary(&self) -> (&str, &str, &str, &str, &str, Option<&str>, Option<&str>, bool) {
        (
            &self.mode,
            &self.outcome,
            &self.stage,
            &self.dispatcher,
            &self.upload,
            self.failure_class.as_deref(),
            self.failure_kind.as_deref(),
            self.counted,
        )
    }
}

async fn tries(db: &PgPool) -> Vec<TryRow> {
    sqlx::query_as(
        "SELECT mode, outcome, stage, dispatcher, upload, failure_class, failure_kind, touched_bir, opens, admission,
             counted, client, account_key, network_key, key_epoch, started_at, ended_at
         FROM bir_tries ORDER BY id",
    )
    .fetch_all(db)
    .await
    .unwrap()
}

/// The rows once there are `count`. An app that went away hears no result,
/// so when it is done the bridge may still be waiting for the Sender's, and
/// its try is not yet being recorded.
async fn wait_for(db: &PgPool, count: usize) -> Vec<TryRow> {
    for _ in 0..100 {
        let rows = tries(db).await;
        if rows.len() >= count {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tries(db).await
}

async fn newest(db: &PgPool) -> TryRow {
    tries(db).await.pop().expect("a try was recorded")
}

async fn status(db: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, mode FROM submissions WHERE id = $1").bind(id).fetch_one(db).await.unwrap()
}

fn texts(got: &[Got]) -> Vec<Value> {
    got.iter()
        .filter_map(|got| match got {
            Got::Text(text) => Some(serde_json::from_str(text).unwrap()),
            Got::Binary(_) => None,
        })
        .collect()
}

fn the_result(got: &[Got]) -> Option<Value> {
    texts(got).into_iter().find(|frame| frame["type"] == "result")
}

fn front_desk_down_words() -> String {
    let attempt = |scheme: &str, host: &str, port: u16| {
        format!("{scheme}://{host}: the app refused to connect to {host}:{port}: {REFUSED}")
    };
    format!(
        "BIR's dispatcher did not answer: {}; {}; {}; {}",
        attempt("https", "dispatcher.test", 443),
        attempt("http", "dispatcher.test", 80),
        attempt("https", "backup.test", 443),
        attempt("http", "backup.test", 80)
    )
}

/// Every front-desk host refuses.
fn front_desk_down() -> (Vec<Step>, Vec<(u16, Answer)>) {
    let words = front_desk_down_words();
    let script = vec![
        hello(RelayMode::Live),
        open(1, "dispatcher.test", 443),
        Step::Expect(1),
        open(2, "dispatcher.test", 80),
        Step::Expect(1),
        open(3, "backup.test", 443),
        Step::Expect(1),
        open(4, "backup.test", 80),
        Step::Expect(1),
        result("failed", Some(&words), None),
    ];
    (script, vec![(443, Answer::Refuse(REFUSED, Some("refused"))), (80, Answer::Refuse(REFUSED, None))])
}

/// The front desk answers over TLS, the upload server opens and lands it.
fn landed() -> (Vec<Step>, Vec<(u16, Answer)>) {
    let script = vec![
        hello(RelayMode::Live),
        open(1, "dispatcher.test", 443),
        Step::Expect(2),
        open(2, "upload.test", 22),
        Step::Expect(2),
        result("sent", None, Some(receipt())),
    ];
    (script, vec![(443, Answer::Open(vec![0x16; 300])), (22, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec()))])
}

/// An SSH server's clear handshake up to NEWKEYS, offering a made-up
/// Ed25519 host key: (the bytes, the key's fingerprint).
fn handshake() -> (Vec<u8>, String) {
    let string = |bytes: &[u8]| [(bytes.len() as u32).to_be_bytes().as_slice(), bytes].concat();
    let packet = |payload: &[u8]| {
        let padding = 8 - (payload.len() + 5) % 8 + 8;
        [((payload.len() + padding + 1) as u32).to_be_bytes().as_slice(), &[padding as u8], payload, &vec![0; padding]]
            .concat()
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    let blob = [string(b"ssh-ed25519"), string(key.verifying_key().as_bytes())].concat();
    let reply = [vec![31], string(&blob), string(&[9; 32]), string(&[1; 83])].concat();
    let bytes = [b"SSH-2.0-made-up\r\n".to_vec(), packet(&[20; 40]), packet(&reply), packet(&[21])].concat();
    let fingerprint =
        format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob)));
    (bytes, fingerprint)
}

/// Each kind of try leaves its row; the app's frames carry no `sender`
/// frame and, at `record`, no new field.
#[sqlx::test]
async fn each_try_leaves_one_classified_row(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Record).await;
    let desk_ok = |bytes: usize| (443, Answer::Open(vec![0x16; bytes]));
    let upload_words = "could not reach BIR's upload server: the app refused to connect to upload.test:22: Connection refused (os error 61)";
    let mode_0 = "BIR's dispatcher did not answer: https://dispatcher.test: dispatcher has no upload server for this TIN (mode 0)";
    let host_key = "the SFTP server's host key is not pinned (see the log for its fingerprint)";
    let (ssh, fingerprint) = handshake();
    let (landed_script, landed_answers) = landed();
    let (down_script, down_answers) = front_desk_down();
    type Summary<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>, bool);
    type Case<'a> = (&'a str, Vec<Step>, Vec<(u16, Answer)>, Summary<'a>);
    let cases: Vec<Case> = vec![
        (
            "sent",
            landed_script.clone(),
            landed_answers.clone(),
            ("live", "sent", "uploaded", "ok", "ok", None, None, true),
        ),
        (
            "the front desk is down",
            down_script.clone(),
            down_answers.clone(),
            ("live", "failed", "dispatcher", "fail", "none", Some("bir"), Some("refused"), true),
        ),
        (
            "the mailbox is down",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(1),
                result("failed", Some(upload_words), None),
            ],
            vec![desk_ok(200), (22, Answer::Refuse(REFUSED, Some("refused")))],
            ("live", "failed", "upload", "ok", "fail", Some("bir"), Some("refused"), true),
        ),
        (
            "no upload service for this TIN",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                result("failed", Some(mode_0), None),
            ],
            vec![desk_ok(200)],
            ("live", "failed", "dispatcher", "none", "none", Some("bir"), Some("no_service"), false),
        ),
        (
            "a host key that is not pinned",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(2),
                result("failed", Some(host_key), None),
            ],
            vec![desk_ok(200), (22, Answer::Open(ssh.clone()))],
            ("live", "failed", "upload", "ok", "none", Some("ours"), Some("host_key"), false),
        ),
        (
            "the app goes away",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(4),
                result("failed", Some("could not reach BIR's upload server: the app disconnected"), None),
            ],
            vec![desk_ok(200), (22, Answer::Leave)],
            ("live", "failed", "upload", "none", "none", Some("user_network"), Some("app_gone"), false),
        ),
        (
            "the Sender says unknown after the app went away: the user's side",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(4),
                result(
                    "unknown",
                    Some(
                        "SFTP failed during the upload: connection reset. BIR may have received this return; check for its confirmation email before sending again.",
                    ),
                    None,
                ),
            ],
            vec![desk_ok(200), (22, Answer::Leave)],
            ("live", "unknown", "upload", "ok", "maybe", Some("user_network"), Some("app_gone"), false),
        ),
        (
            "SSH with no common algorithm: Buwiz's side",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(2),
                result("failed", Some("SSH failed: No common key exchange algorithm"), None),
            ],
            landed_answers.clone(),
            ("live", "failed", "upload", "ok", "none", Some("ours"), Some("protocol"), false),
        ),
        (
            "the Sender stops during the upload",
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(2),
                open(2, "upload.test", 22),
                Step::Expect(2),
                Step::Drop,
            ],
            landed_answers.clone(),
            ("live", "unknown", "upload", "none", "maybe", Some("ours"), Some("sender_down"), false),
        ),
        (
            "no sender frame: mode unknown, never counted",
            landed_script[1..].to_vec(),
            landed_answers.clone(),
            ("unknown", "sent", "uploaded", "ok", "ok", None, None, false),
        ),
        (
            "no sender frame, failed: like lab, no class",
            down_script[1..].to_vec(),
            down_answers.clone(),
            ("unknown", "failed", "dispatcher", "fail", "none", None, None, false),
        ),
        (
            "lab: no class",
            vec![
                hello(RelayMode::Lab),
                open(1, "lab.test", 2222),
                Step::Expect(2),
                result("failed", Some("SFTP login was refused"), None),
            ],
            vec![(2222, Answer::Open(b"SSH-2.0-lab\r\n".to_vec()))],
            ("lab", "failed", "upload", "none", "fail", None, None, false),
        ),
        (
            "not built in",
            vec![hello(RelayMode::Live), result("failed", Some(sender::NOT_BUILT_IN), None)],
            vec![],
            ("live", "failed", "none", "none", "none", Some("ours"), Some("not_built_in"), false),
        ),
    ];
    for (index, (name, script, answers, expected)) in cases.into_iter().enumerate() {
        let id = if index % 2 == 0 { world.v1().await } else { world.v2(None).await };
        let got = gatekeeper.send(&world, id, script, &answers, HOME).await;
        let rows = wait_for(&db, index + 1).await;
        assert_eq!(rows.len(), index + 1, "{name}: one row per try");
        let row = rows.last().unwrap();
        assert_eq!(row.summary(), expected, "{name}");
        assert_eq!((row.admission.as_str(), row.client.as_deref()), ("open", Some("desktop/0.2")), "{name}");
        assert!(row.started_at <= row.ended_at && row.ended_at.second() == 0 && row.started_at.second() == 0);
        assert_eq!((row.account_key.len(), row.network_key.as_ref().map(Vec::len)), (8, Some(8)));
        // The app never sees the sender frame, nor a field it did not
        // have before.
        let frames = texts(&got);
        assert!(frames.iter().all(|frame| frame["type"] != "sender"), "{name}");
        if let Some(result) = the_result(&got) {
            let mut keys: Vec<&str> = result.as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["error", "receipt", "status", "type"], "{name}");
        }
        // Below `public` the submission keeps no mode for a failure.
        let (status, mode) = status(&db, id).await;
        if status != "sent" {
            assert_eq!(mode, None, "{name}");
        }
    }
    // The app that went away got no result; the Sender heard the streams
    // close.
    assert!(gatekeeper.sender.heard.lock().unwrap().iter().any(|text| text.contains("the app disconnected")));
    // The host key it was offered, with who saw it as day keys.
    let keys: Vec<(String, Value)> =
        sqlx::query_as("SELECT fingerprint, seen FROM bir_host_keys").fetch_all(&db).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, fingerprint);
    let seen = keys[0].1.as_array().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["account_key"].as_str().unwrap().len(), 16);
    assert_eq!(seen[0]["network_key"].as_str().unwrap().len(), 16);

    // The same account and network have the same keys all day; another
    // /24 another network key.
    let rows = tries(&db).await;
    assert!(rows.iter().all(|row| row.account_key == rows[0].account_key && row.network_key == rows[0].network_key));
    assert!(rows.iter().all(|row| row.key_epoch == rows[0].key_epoch));
    let (script, answers) = front_desk_down();
    gatekeeper.send(&world, world.v1().await, script.clone(), &answers, "203.0.113.200").await;
    assert_eq!(newest(&db).await.network_key, rows[0].network_key);
    gatekeeper.send(&world, world.v1().await, script, &answers, "198.51.100.4").await;
    let other = newest(&db).await;
    assert_ne!(other.network_key, rows[0].network_key);
    assert_eq!(other.account_key, rows[0].account_key);

    // Nothing in the record says who or where.
    assert_no_one_in_the_record(&db, &world).await;
}

/// Nothing in the record says who or where: not in its text, and not in
/// its bytes (the bytea keys, and the hex keys of the host-key sightings,
/// decoded): no address prefix, no account id, no TIN.
async fn assert_no_one_in_the_record(db: &PgPool, world: &World) {
    let account = world.account.to_string();
    let dump: Vec<String> = sqlx::query_scalar(
        "SELECT to_jsonb(t)::text FROM bir_tries t UNION ALL SELECT to_jsonb(k)::text FROM bir_host_keys k",
    )
    .fetch_all(db)
    .await
    .unwrap();
    assert!(!dump.is_empty());
    for row in &dump {
        for secret in
            [ROOT, EMAIL, "203.0.113", "198.51.100", "127.0.0.1", account.as_str(), "dispatcher.test", "upload.test"]
        {
            assert!(!row.contains(secret), "{secret} in {row}");
        }
    }
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    let keys: Vec<(Vec<u8>, Option<Vec<u8>>)> =
        sqlx::query_as("SELECT account_key, network_key FROM bir_tries").fetch_all(db).await.unwrap();
    for (account_key, network_key) in keys {
        blobs.push(account_key);
        blobs.extend(network_key);
    }
    let seen: Vec<Value> = sqlx::query_scalar("SELECT seen FROM bir_host_keys").fetch_all(db).await.unwrap();
    for sighting in seen.iter().flat_map(|seen| seen.as_array().unwrap()) {
        for field in ["account_key", "network_key"] {
            let hex = sighting[field].as_str().unwrap();
            let bytes = (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap()).collect();
            blobs.push(bytes);
        }
    }
    let raw: Vec<Vec<u8>> = [
        // The networks (the /24s and the loopback), the account id's
        // bytes, the TIN as digits and as bytes.
        vec![203, 0, 113],
        vec![198, 51, 100],
        vec![127, 0, 0],
        world.account.as_bytes().to_vec(),
        ROOT.as_bytes().to_vec(),
    ]
    .into();
    // A raw prefix or id stored would always show; 8 random bytes hold a
    // given 3 bytes by chance about once in a million.
    for blob in &blobs {
        assert_eq!(blob.len(), 8);
        for secret in &raw {
            let window = secret.len().clamp(3, 4);
            for piece in secret.windows(window) {
                assert!(!blob.windows(window).any(|part| part == piece), "{piece:?} in {blob:?}");
            }
        }
    }
}

/// The ticket's admission goes into the row; a v1 return's is `open`.
#[sqlx::test]
async fn the_tickets_admission_is_recorded(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Record).await;
    let (script, answers) = front_desk_down();
    gatekeeper.send(&world, world.v2(Some("scout")).await, script.clone(), &answers, HOME).await;
    assert_eq!(newest(&db).await.admission, "scout");
    gatekeeper.send(&world, world.v2(None).await, script.clone(), &answers, HOME).await;
    assert_eq!(newest(&db).await.admission, "open");
    gatekeeper.send(&world, world.v1().await, script, &answers, HOME).await;
    assert_eq!(newest(&db).await.admission, "open");
}

/// The frames the Gatekeeper sent the app before this feature, written out
/// by hand: the Gatekeeper forwarded every Sender text and binary frame as
/// it came (that Sender sent no `sender` frame), and ended with the
/// `result` as `serde_json` wrote the relay's `Control` then: `type`,
/// `status`, `error`, `receipt`, with `null` for none. Nothing here is made
/// by the code under test.
mod before {
    pub const OPEN_DESK_1: &str = r#"{"type":"open","stream":1,"host":"dispatcher.test","port":443}"#;
    pub const OPEN_DESK_2: &str = r#"{"type":"open","stream":2,"host":"dispatcher.test","port":80}"#;
    pub const OPEN_DESK_3: &str = r#"{"type":"open","stream":3,"host":"backup.test","port":443}"#;
    pub const OPEN_DESK_4: &str = r#"{"type":"open","stream":4,"host":"backup.test","port":80}"#;
    pub const OPEN_UPLOAD: &str = r#"{"type":"open","stream":2,"host":"upload.test","port":22}"#;
    pub const CLOSE_DESK: &str = r#"{"type":"close","stream":1}"#;
    /// A TLS ClientHello's first bytes, from the Sender to the front desk.
    pub const HELLO: &[u8] = &[0x16, 0x03, 0x01, 0x00, 0xa5, 0x01, 0x00, 0x00, 0xa1, 0x03, 0x03];
    /// The Sender's SSH version line, to the mailbox.
    pub const SSH_HELLO: &[u8] = b"SSH-2.0-made-up-sender\r\n";
    pub const FAILED: &str = concat!(
        r#"{"type":"result","status":"failed","error":"BIR's dispatcher did not answer: "#,
        r#"https://dispatcher.test: the app refused to connect to dispatcher.test:443: Connection refused (os error 61); "#,
        r#"http://dispatcher.test: the app refused to connect to dispatcher.test:80: Connection refused (os error 61); "#,
        r#"https://backup.test: the app refused to connect to backup.test:443: Connection refused (os error 61); "#,
        r#"http://backup.test: the app refused to connect to backup.test:80: Connection refused (os error 61)","receipt":null}"#
    );
    pub const UNKNOWN: &str = r#"{"type":"result","status":"unknown","error":"SFTP failed during the upload: connection reset. BIR may have received this return; check for its confirmation email before sending again.","receipt":null}"#;
    /// The Gatekeeper's own words when the Sender's socket ends without a
    /// result after a stream was opened.
    pub const LOST: &str = r#"{"type":"result","status":"unknown","error":"the send ended without an outcome; BIR may have received this return","receipt":null}"#;
    /// The Gatekeeper's own words when the Sender cannot be reached.
    pub const UNREACHABLE: &str = r#"{"type":"result","status":"failed","error":"the sending service is not reachable; try again later","receipt":null}"#;

    pub fn sent(receipt: &str) -> String {
        format!(r#"{{"type":"result","status":"sent","error":null,"receipt":"{receipt}"}}"#)
    }
}

/// One flow of the byte-identity test: the fake Sender's script, the fake
/// app's answers, and what the app got before this feature.
struct Flow {
    name: &'static str,
    script: Vec<Step>,
    answers: Vec<(u16, Answer)>,
    before: Vec<Got>,
    /// The `class` added at `public`, if any.
    class: Option<&'static str>,
}

fn text(raw: &str) -> Step {
    Step::Text(raw.to_owned())
}

fn flows() -> Vec<Flow> {
    use before::*;
    let receipt = receipt();
    let t = |raw: &str| Got::Text(raw.to_owned());
    let b = |stream: u8, bytes: &[u8]| Got::Binary([&[stream], bytes].concat());
    vec![
        Flow {
            name: "the front desk is down",
            script: vec![
                hello(RelayMode::Live),
                text(OPEN_DESK_1),
                Step::Expect(1),
                text(OPEN_DESK_2),
                Step::Expect(1),
                text(OPEN_DESK_3),
                Step::Expect(1),
                text(OPEN_DESK_4),
                Step::Expect(1),
                text(FAILED),
            ],
            answers: vec![(443, Answer::Refuse(REFUSED, Some("refused"))), (80, Answer::Refuse(REFUSED, None))],
            before: vec![t(OPEN_DESK_1), t(OPEN_DESK_2), t(OPEN_DESK_3), t(OPEN_DESK_4), t(FAILED)],
            class: Some("bir"),
        },
        Flow {
            name: "sent",
            script: vec![
                hello(RelayMode::Live),
                text(OPEN_DESK_1),
                Step::Expect(2),
                Step::Binary(1, HELLO.to_vec()),
                Step::Text(CLOSE_DESK.into()),
                text(OPEN_UPLOAD),
                Step::Expect(2),
                Step::Binary(2, SSH_HELLO.to_vec()),
                Step::Text(before::sent(&receipt)),
            ],
            answers: vec![(443, Answer::Open(vec![0x16; 300])), (22, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec()))],
            before: vec![
                t(OPEN_DESK_1),
                b(1, HELLO),
                t(CLOSE_DESK),
                t(OPEN_UPLOAD),
                b(2, SSH_HELLO),
                Got::Text(before::sent(&receipt)),
            ],
            class: None,
        },
        Flow {
            name: "unknown",
            script: vec![
                hello(RelayMode::Live),
                text(OPEN_DESK_1),
                Step::Expect(2),
                Step::Binary(1, HELLO.to_vec()),
                text(OPEN_UPLOAD),
                Step::Expect(2),
                Step::Binary(2, SSH_HELLO.to_vec()),
                text(UNKNOWN),
            ],
            answers: vec![(443, Answer::Open(vec![0x16; 300])), (22, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec()))],
            before: vec![t(OPEN_DESK_1), b(1, HELLO), t(OPEN_UPLOAD), b(2, SSH_HELLO), t(UNKNOWN)],
            class: Some("bir"),
        },
        Flow {
            name: "the Sender lost during the upload",
            script: vec![
                hello(RelayMode::Live),
                text(OPEN_DESK_1),
                Step::Expect(2),
                Step::Binary(1, HELLO.to_vec()),
                text(OPEN_UPLOAD),
                Step::Expect(2),
                Step::Binary(2, SSH_HELLO.to_vec()),
                Step::Drop,
            ],
            answers: vec![(443, Answer::Open(vec![0x16; 300])), (22, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec()))],
            before: vec![t(OPEN_DESK_1), b(1, HELLO), t(OPEN_UPLOAD), b(2, SSH_HELLO), t(LOST)],
            class: Some("ours"),
        },
        Flow {
            name: "the app goes away",
            script: vec![
                hello(RelayMode::Live),
                text(OPEN_DESK_1),
                Step::Expect(2),
                Step::Binary(1, HELLO.to_vec()),
                text(OPEN_UPLOAD),
                Step::Expect(4),
                text(
                    r#"{"type":"result","status":"failed","error":"could not reach BIR's upload server: the app disconnected","receipt":null}"#,
                ),
            ],
            answers: vec![(443, Answer::Open(vec![0x16; 300])), (22, Answer::Leave)],
            before: vec![t(OPEN_DESK_1), b(1, HELLO), t(OPEN_UPLOAD)],
            class: None,
        },
    ]
}

/// A v1 and a v2 send get byte for byte the frames the Gatekeeper sent
/// before this feature (written out in [`before`]), at `off` and `record`;
/// at `public` the same but for `class` added at the end of a `result` that
/// did not get in. The `sender` frame never reaches the app.
#[sqlx::test]
async fn apps_get_the_frames_they_got_before_but_for_the_added_fields(db: PgPool) {
    let world = World::new(db.clone()).await;
    for level in [Level::Off, Level::Record, Level::Public] {
        let gatekeeper = Gatekeeper::new(&world, level).await;
        for flow in flows() {
            for v2 in [false, true] {
                let id = if v2 { world.v2(None).await } else { world.v1().await };
                let got = gatekeeper.send(&world, id, flow.script.clone(), &flow.answers, HOME).await;
                let mut expected = flow.before.clone();
                if let (Level::Public, Some(class), Some(Got::Text(last))) = (level, flow.class, expected.last_mut()) {
                    last.truncate(last.len() - 1);
                    last.push_str(&format!(r#","class":"{class}"}}"#));
                }
                assert_eq!(got, expected, "{}: {level:?}, v2 {v2}", flow.name);
            }
        }
    }
    // The Sender unreachable.
    for level in [Level::Off, Level::Record, Level::Public] {
        let uptime = Uptime::new(level, vec![22], false, OffsetDateTime::now_utc());
        let gatekeeper = Gatekeeper::at(&world, uptime, "http://127.0.0.1:9", FakeSender::default()).await;
        let got = gatekeeper.send(&world, world.v1().await, vec![], &[], HOME).await;
        let expected = match level {
            Level::Public => before::UNREACHABLE.replace(r#""receipt":null}"#, r#""receipt":null,"class":"ours"}"#),
            _ => before::UNREACHABLE.to_owned(),
        };
        assert_eq!(got, [Got::Text(expected)], "unreachable: {level:?}");
    }
    // Off records nothing; record and public a row per try.
    assert_eq!(wait_for(&db, 2 * 2 * flows().len() + 2).await.len(), 2 * 2 * flows().len() + 2);
    // From `public` a failed submission keeps the Sender's mode; below it,
    // none.
    let modes: Vec<Option<String>> =
        sqlx::query_scalar("SELECT mode FROM submissions WHERE status = 'failed' ORDER BY created_at")
            .fetch_all(&db)
            .await
            .unwrap();
    let live = Some("live".to_owned());
    // Per level: two front-desk failures and two app-gone failures.
    assert_eq!(modes[..8], [None, None, None, None, None, None, None, None]);
    assert_eq!(modes[8..12], [live.clone(), live.clone(), live.clone(), live]);
    assert_eq!(modes[12..], [None, None, None]);
}

/// The classes an app is told at `public`: one per side, absent for a try
/// that got in.
#[sqlx::test]
async fn at_public_the_result_says_whose_side(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Public).await;
    let (script, answers) = front_desk_down();
    let got = gatekeeper.send(&world, world.v1().await, script, &answers, HOME).await;
    assert_eq!(the_result(&got).unwrap()["class"], "bir");
    let got = gatekeeper
        .send(
            &world,
            world.v1().await,
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(1),
                result("failed", Some("x"), None),
            ],
            &[(443, Answer::Refuse("dispatcher.test:443 is not on the signed trust list", None))],
            HOME,
        )
        .await;
    assert_eq!(the_result(&got).unwrap()["class"], "ours");
    let got = gatekeeper
        .send(
            &world,
            world.v1().await,
            vec![
                hello(RelayMode::Live),
                open(1, "dispatcher.test", 443),
                Step::Expect(1),
                result("failed", Some("x"), None),
            ],
            &[(443, Answer::Refuse("Network is unreachable (os error 51)", Some("unreachable")))],
            HOME,
        )
        .await;
    assert_eq!(the_result(&got).unwrap()["class"], "user_network");
    let (script, answers) = landed();
    let got = gatekeeper.send(&world, world.v1().await, script, &answers, HOME).await;
    assert_eq!(the_result(&got).unwrap().get("class"), None);
    // Lab: no class to tell.
    let got = gatekeeper
        .send(
            &world,
            world.v1().await,
            vec![hello(RelayMode::Lab), result("failed", Some("no lab SFTP server set"), None)],
            &[],
            HOME,
        )
        .await;
    assert_eq!(the_result(&got).unwrap().get("class"), None);
    // No `sender` frame (a Sender from before it): like lab, nothing told.
    let (script, answers) = front_desk_down();
    let got = gatekeeper.send(&world, world.v1().await, script[1..].to_vec(), &answers, HOME).await;
    assert_eq!(the_result(&got).unwrap().get("class"), None);
    let row = newest(&db).await;
    assert_eq!((row.mode.as_str(), row.failure_class.as_deref(), row.counted), ("unknown", None, false));
}

/// `off` records nothing and still keeps the `sender` frame from the app.
#[sqlx::test]
async fn off_records_nothing(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Off).await;
    let (script, answers) = landed();
    let got = gatekeeper.send(&world, world.v1().await, script, &answers, HOME).await;
    assert_eq!(the_result(&got).unwrap()["status"], "sent");
    assert!(texts(&got).iter().all(|frame| frame["type"] != "sender"));
    assert!(tries(&db).await.is_empty());
    let keys: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_host_keys").fetch_one(&db).await.unwrap();
    assert_eq!(keys, 0);
}

/// A recorder that cannot write never breaks a send: the app hears the
/// result and the submission reads it.
#[sqlx::test]
async fn a_send_concludes_when_the_record_cannot_be_written(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Public).await;
    sqlx::query("ALTER TABLE bir_tries ADD CONSTRAINT refuse_every_row CHECK (false) NOT VALID")
        .execute(&db)
        .await
        .unwrap();
    let (script, answers) = front_desk_down();
    let id = world.v1().await;
    let got = gatekeeper.send(&world, id, script, &answers, HOME).await;
    let result = the_result(&got).unwrap();
    assert_eq!((result["status"].as_str(), result["class"].as_str()), (Some("failed"), Some("bir")));
    assert_eq!(status(&db, id).await, ("failed".to_owned(), Some("live".to_owned())));
    let (script, answers) = landed();
    let id = world.v2(None).await;
    let got = gatekeeper.send(&world, id, script, &answers, HOME).await;
    assert_eq!(the_result(&got).unwrap()["status"], "sent");
    assert_eq!(status(&db, id).await.0, "sent");
    sqlx::query("ALTER TABLE bir_tries DROP CONSTRAINT refuse_every_row").execute(&db).await.unwrap();
    assert!(tries(&db).await.is_empty());
}

/// The Sender unreachable: recorded as Buwiz's side, nothing opened.
#[sqlx::test]
async fn an_unreachable_sender_is_ours(db: PgPool) {
    let world = World::new(db.clone()).await;
    // Nothing listens on the discard port.
    let uptime = Uptime::new(Level::Public, vec![22], false, OffsetDateTime::now_utc());
    let gatekeeper = Gatekeeper::at(&world, uptime, "http://127.0.0.1:9", FakeSender::default()).await;
    let got = gatekeeper.send(&world, world.v1().await, vec![], &[], HOME).await;
    let result = the_result(&got).unwrap();
    assert_eq!(result["class"], "ours");
    assert_eq!(
        newest(&db).await.summary(),
        ("unknown", "failed", "none", "none", "none", Some("ours"), Some("sender_down"), false)
    );
}

/// The real Sender (built in, in `dry_run`, as development runs it) says
/// its mode in its first frame; the app never sees it.
#[sqlx::test]
async fn the_real_sender_says_its_mode(db: PgPool) {
    let world = World::new(db.clone()).await;
    let ticket_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let config = sender::SenderConfig::embedded(ticket_key.verifying_key(), common::ORIGIN).unwrap();
    assert_eq!(config.relay.mode, RelayMode::DryRun);
    let secret = config.secret.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(sender::serve(listener, sender::Sender::new(config)));
    let uptime = Uptime::new(Level::Record, vec![22], false, OffsetDateTime::now_utc());
    let site = Site::with_sender(db.clone(), &url, &secret, uptime);
    let base = serve(site.router).await;
    let gatekeeper =
        Gatekeeper { base, sender: FakeSender::default(), state: site.state, result_after: Mutex::new(None) };
    let got = gatekeeper.send(&world, world.v1().await, vec![], &[], HOME).await;
    // The made-up return does not match its file name: refused before
    // anything opens.
    let result = the_result(&got).unwrap();
    assert!(result["error"].as_str().unwrap().ends_with("(return_mismatch)"), "{result}");
    assert_eq!(texts(&got).len(), 1, "the result alone");
    let row = newest(&db).await;
    assert_eq!(row.summary(), ("dry_run", "failed", "none", "none", "none", None, None, false));
    assert!(!row.touched_bir && row.opens == 0);
}

/// The app has its `result` at once, however long the record takes: the
/// record is written after the result, off the send's path.
#[sqlx::test]
async fn the_result_never_waits_for_the_record(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Record).await;
    // Another session holds `bir_tries` for 3 seconds: every insert waits.
    let mut lock = db.begin().await.unwrap();
    sqlx::query("LOCK TABLE bir_tries IN ACCESS EXCLUSIVE MODE").execute(&mut *lock).await.unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        lock.rollback().await.unwrap();
    });
    let (script, answers) = front_desk_down();
    let id = world.v1().await;
    let got = gatekeeper.send_only(&world, id, script, &answers, HOME).await;
    let after = gatekeeper.result_after.lock().unwrap().expect("a result");
    assert!(after < Duration::from_millis(1500), "the result came after {after:?}");
    assert_eq!(the_result(&got).unwrap()["status"], "failed");
    assert_eq!(status(&db, id).await.0, "failed");
    // The record is being written, and is once the table is free.
    assert_eq!(gatekeeper.state.uptime.recorder.pending(), 1);
    gatekeeper.state.uptime.recorder.settled().await;
    release.await.unwrap();
    assert_eq!(tries(&db).await.len(), 1);
    // At shutdown, new records are refused and those written are waited for.
    assert!(gatekeeper.state.uptime.recorder.drain(Duration::from_secs(1)).await);
    let (script, answers) = front_desk_down();
    gatekeeper.send(&world, world.v1().await, script, &answers, HOME).await;
    assert_eq!(tries(&db).await.len(), 1);
}

/// A try that started before Manila midnight and ends after it gets the
/// keys of when it is recorded: the same account and network have the same
/// keys as a try that started after midnight.
#[sqlx::test]
async fn keys_are_taken_when_a_try_is_recorded(db: PgPool) {
    let manila = |hms: (u8, u8, u8), day: u8| {
        time::Date::from_calendar_date(2026, time::Month::November, day)
            .unwrap()
            .with_hms(hms.0, hms.1, hms.2)
            .unwrap()
            .assume_offset(MANILA)
    };
    let uptime = Uptime::new(Level::Record, vec![22], false, manila((23, 59, 0), 11));
    let account = Uuid::new_v4();
    let network = Network::V4([203, 0, 113]);
    let observed = || {
        let mut observer = TryObserver::new(&[22]);
        observer.sender_text(&sender::sender_frame(RelayMode::Live));
        observer.finish("failed", Some(sender::NOT_BUILT_IN))
    };
    let relay = |started_at| RelayTry {
        account_id: account,
        network: network.clone(),
        client: None,
        admission: "open".into(),
        started_at,
    };
    // A starts at 23:59:40; B at 00:00:05 and ends first, at 00:00:15; A
    // ends at 00:00:30.
    let a = relay(manila((23, 59, 40), 11));
    let b = relay(manila((0, 0, 5), 12));
    store::record(&uptime, &db, &b, &observed(), manila((0, 0, 15), 12)).await;
    store::record(&uptime, &db, &a, &observed(), manila((0, 0, 30), 12)).await;
    let rows = tries(&db).await;
    assert_eq!(rows.len(), 2);
    let keys = |row: &TryRow| (row.key_epoch, row.account_key.clone(), row.network_key.clone());
    assert_eq!(keys(&rows[1]), keys(&rows[0]));
    // Its start stays its own.
    assert_eq!(rows[1].started_at, manila((23, 59, 0), 11));
}

/// A host key is recorded with whether the Sender went on to complete the
/// key exchange, and its row is written only when a sighting adds
/// something: not again for the same account and network in a key period.
#[sqlx::test]
async fn host_key_sightings_say_whether_the_key_exchange_completed(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::new(&world, Level::Record).await;
    let (ssh, fingerprint) = handshake();
    let upload = |last: Step| {
        vec![
            hello(RelayMode::Live),
            open(1, "dispatcher.test", 443),
            Step::Expect(2),
            open(2, "upload.test", 22),
            Step::Expect(2),
            last,
        ]
    };
    let answers = [(443, Answer::Open(vec![0x16; 200])), (22, Answer::Open(ssh.clone()))];
    let not_pinned =
        result("failed", Some("the SFTP server's host key is not pinned (see the log for its fingerprint)"), None);
    gatekeeper.send(&world, world.v1().await, upload(not_pinned.clone()), &answers, HOME).await;
    let row = || async {
        sqlx::query_as::<_, (Value, String)>("SELECT seen, xmin::text FROM bir_host_keys WHERE fingerprint = $1")
            .bind(&fingerprint)
            .fetch_one(&db)
            .await
            .unwrap()
    };
    let (seen, version) = row().await;
    assert_eq!(seen.as_array().unwrap().len(), 1);
    assert_eq!(seen[0]["kex_done"], false);
    // The same again from the same account and network adds nothing: the
    // row is not rewritten.
    gatekeeper.send(&world, world.v1().await, upload(not_pinned.clone()), &answers, HOME).await;
    assert_eq!(row().await, (seen.clone(), version.clone()));
    // A try that got in completed it: a new sighting.
    gatekeeper.send(&world, world.v1().await, upload(result("sent", None, Some(receipt()))), &answers, HOME).await;
    let (seen, version) = row().await;
    assert_eq!(seen.as_array().unwrap().iter().map(|s| s["kex_done"].clone()).collect::<Vec<_>>(), [false, true]);
    // Every pinned upload from here adds nothing either.
    for _ in 0..3 {
        gatekeeper.send(&world, world.v1().await, upload(result("sent", None, Some(receipt()))), &answers, HOME).await;
    }
    assert_eq!(row().await, (seen.clone(), version.clone()));
    // Another network in the same key period is new.
    gatekeeper.send(&world, world.v1().await, upload(not_pinned), &answers, "198.51.100.4").await;
    let (seen, _) = row().await;
    assert_eq!(seen.as_array().unwrap().len(), 3);
    let periods = store::periods(&seen);
    assert_eq!(periods.len(), 1);
    assert_eq!((periods[0].accounts, periods[0].networks, periods[0].kex_done_accounts), (1, 2, 1));

    // The database itself does not rewrite the row for a sighting it holds
    // (another process, or one this one has forgotten).
    let keys = buwiz_cloud::uptime::keys::Keys { epoch: 77, account: [1; 8], network: [2; 8] };
    let now = OffsetDateTime::now_utc();
    store::note_host_key(&db, &fingerprint, &keys, true, now).await.unwrap();
    let before = row().await;
    store::note_host_key(&db, &fingerprint, &keys, true, now).await.unwrap();
    assert_eq!(row().await, before);
}

/// `BIR_UPTIME_UPLOAD_PORTS` decides which stream is the upload: with
/// 2222, a stream on 2222 is the mailbox and one on 22 the front desk.
#[sqlx::test]
async fn the_upload_window_is_on_the_ports_set(db: PgPool) {
    let world = World::new(db.clone()).await;
    let gatekeeper = Gatekeeper::with_ports(&world, Level::Record, vec![2222]).await;
    let script = |port: u16| {
        vec![
            hello(RelayMode::Live),
            open(1, "dispatcher.test", 443),
            Step::Expect(2),
            open(2, "upload.test", port),
            Step::Expect(2),
            result("failed", Some("SSH failed: Disconnected"), None),
        ]
    };
    let answers = [
        (443, Answer::Open(vec![0x16; 200])),
        (2222, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec())),
        (22, Answer::Open(b"SSH-2.0-made-up\r\n".to_vec())),
    ];
    gatekeeper.send(&world, world.v1().await, script(2222), &answers, HOME).await;
    assert_eq!(newest(&db).await.summary(), ("live", "failed", "upload", "ok", "fail", Some("bir"), Some("ssh"), true));
    gatekeeper.send(&world, world.v1().await, script(22), &answers, HOME).await;
    let row = newest(&db).await;
    assert_eq!((row.stage.as_str(), row.upload.as_str()), ("dispatcher", "none"));
}

/// Why the bridge does not look at the TIN before recording: a claim's TIN
/// always parses (the database refuses one that would not), and a test
/// TIN's return is refused before any Sender, so it is never recorded.
#[sqlx::test]
async fn no_tin_that_should_not_be_recorded_reaches_the_record(db: PgPool) {
    let world = World::new(db.clone()).await;
    for (root, branch) in [("00012345X", "00000"), ("00012345", "00000"), ("000123453", "0000A")] {
        let refused = sqlx::query("UPDATE tin_claims SET tin_root = $2, branch_code = $3 WHERE id = $1")
            .bind(world.claim)
            .bind(root)
            .bind(branch)
            .execute(&db)
            .await;
        match refused {
            Err(sqlx::Error::Database(error)) => assert_eq!(error.code().as_deref(), Some("23514"), "{root}"),
            other => panic!("{root}-{branch} should break a check: {other:?}"),
        }
    }
    let gatekeeper = Gatekeeper::new(&world, Level::Record).await;
    let (script, answers) = front_desk_down();
    gatekeeper.send(&world, world.v1().await, script.clone(), &answers, HOME).await;
    assert_eq!(tries(&db).await.len(), 1);

    let test_claim: Uuid = sqlx::query_scalar(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ($1, 'business', '000000000', '00001', 'TEST TAXPAYER', 'approved', now(), 'admin') RETURNING id",
    )
    .bind(world.account)
    .fetch_one(&db)
    .await
    .unwrap();
    let id = world.v1().await;
    sqlx::query("UPDATE submissions SET claim_id = $2 WHERE id = $1")
        .bind(id)
        .bind(test_claim)
        .execute(&db)
        .await
        .unwrap();
    let got = gatekeeper.send(&world, id, script, &answers, HOME).await;
    // Refused before any Sender: the result alone.
    assert_eq!(got.len(), 1);
    let result = the_result(&got).unwrap();
    assert_eq!(result["error"], buwiz_cloud::filing::gate::Standing::TestOnly.failure());
    assert_eq!(tries(&db).await.len(), 1);
}
