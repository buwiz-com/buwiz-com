//! V6: what an attacker might try against submission v2, each refused.

mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bir_submit::device::{self, DeviceKey, HEADER_DEVICE};
use bir_submit::pipe::Ended;
use bir_submit::seal;
use bir_submit::secrets::SecretStore;
use bir_submit::{ClientError, Submitter};
use buwiz_cloud::relay::{Control, Pipe};
use buwiz_cloud::sender::Job;
use buwiz_cloud::tickets::{self, TicketClaims};
use common::{EMAIL, Harness, TIN};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A 1701Q savefile body naming `tin` (14 digits) for Q1 2026.
fn xml(tin: &str) -> Vec<u8> {
    let line = |key: &str, value: &str| format!("<div>frm1701q:{key}={value}frm1701q:{key}=</div>\t\r\n            ");
    let mut xml = String::from("<?xml version='1.0'?>\t\r\n            ");
    xml += &line("txtYear", "2026");
    xml += &line("DateQuarter_1", "true");
    xml += &line("DateQuarter_2", "false");
    xml += &line("DateQuarter_3", "false");
    xml += &line("txtTIN1", &tin[0..3]);
    xml += &line("txtTIN2", &tin[3..6]);
    xml += &line("txtTIN3", &tin[6..9]);
    xml += &line("txtBranchCode", &tin[9..]);
    xml.into_bytes()
}

fn ticket_body(tin: &str, xml: &[u8], key: &str) -> Value {
    json!({
        "tin": tin, "form": "1701Qv2018", "period": "2026Q1",
        "payload_sha256": hex(&Sha256::digest(xml)), "email": EMAIL, "idempotency_key": key,
    })
}

/// The Sender's sealing key, as the app takes it: from the signed list.
async fn sealing_key(submitter: &Submitter) -> seal::SealingKey {
    let list = submitter.trust_list().await.unwrap();
    submitter.sealing_key(&list).await.unwrap()
}

/// A pipe whose app has gone: any connection the Sender asks for fails at
/// once, so nothing can be sent through it.
fn no_app() -> Pipe {
    let (out, _) = tokio::sync::mpsc::channel(1);
    let (_, inbound) = tokio::sync::mpsc::channel(1);
    Pipe::new(out, inbound)
}

/// A request signed with the device key in `store`, sent as is: answers
/// the HTTP status and the server's error code.
async fn raw_signed(
    harness: &Harness,
    store: &common::SharedStore,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String) {
    let session: Value = serde_json::from_str(&store.get("session").unwrap().unwrap()).unwrap();
    let key = DeviceKey::from_stored(&store.get("device_key").unwrap().unwrap()).unwrap();
    let mut request = reqwest::Client::new()
        .request(method.parse().unwrap(), format!("{}{path}", harness.base))
        .bearer_auth(session["access_token"].as_str().unwrap())
        .header(HEADER_DEVICE, session["device_id"].as_str().unwrap())
        .header("content-type", "application/json")
        .body(body.to_vec());
    for (name, value) in key.sign(method, path, body, device::now(), &device::nonce()) {
        request = request.header(name, value);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or_default();
    (status, body["error"]["code"].as_str().unwrap_or_default().to_owned())
}

fn server_code(result: Result<Value, ClientError>) -> String {
    match result {
        Err(ClientError::Server { code, .. }) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Ticket, seal, v2 submission: answers (jti, ticket, submission view).
async fn submit(submitter: &Submitter, xml_for_ticket: &[u8], sealed: &[u8], key: &str) -> (String, String, Value) {
    let ticket = submitter.signed("POST", "/api/tickets", &ticket_body(TIN, xml_for_ticket, key)).await.unwrap();
    let jti = ticket["jti"].as_str().unwrap().to_owned();
    let jws = ticket["ticket"].as_str().unwrap().to_owned();
    let envelope = seal::seal(&sealing_key(submitter).await, &jti, sealed).unwrap();
    let view =
        submitter.signed("POST", "/api/v2/submissions", &json!({ "ticket": jws, "envelope": envelope })).await.unwrap();
    (jti, jws, view)
}

#[tokio::test]
async fn a_tin_not_active_on_the_account_gets_no_ticket() {
    let harness = Harness::start().await;
    let (submission, _) = harness.signed_in().await;
    let other = "00012345400000";
    let code = server_code(
        submission.submitter().signed("POST", "/api/tickets", &ticket_body(other, &xml(other), "k1")).await,
    );
    assert_eq!(code, "tin_not_active");
    // Someone else's approved TIN is no better.
    let stranger: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('other@example.com') RETURNING id")
        .fetch_one(&harness.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tin_claims (account_id, tin_root, branch_code, registered_name, status)
         VALUES ($1, '000123454', '00000', 'Someone Else', 'approved')",
    )
    .bind(stranger)
    .execute(&harness.db)
    .await
    .unwrap();
    let code = server_code(
        submission.submitter().signed("POST", "/api/tickets", &ticket_body(other, &xml(other), "k2")).await,
    );
    assert_eq!(code, "tin_not_active");
}

#[tokio::test]
async fn a_ticket_works_once() {
    let harness = Harness::start().await;
    let (submission, _) = harness.signed_in().await;
    let submitter = submission.submitter();
    let body = xml(TIN);
    let (jti, jws, _) = submit(submitter, &body, &body, "once").await;

    // Again at the Gatekeeper, with a fresh signature.
    let envelope = seal::seal(&sealing_key(submitter).await, &jti, &body).unwrap();
    let again = submitter.signed("POST", "/api/v2/submissions", &json!({ "ticket": jws, "envelope": envelope })).await;
    assert_eq!(server_code(again), "ticket_used");

    // And straight at the Sender: a ticket it has seen is refused before
    // the envelope is opened.
    let job = Job::V2 {
        submission_id: Uuid::new_v4(),
        account_id: harness.account_id,
        ticket: jws,
        envelope: serde_json::from_value(serde_json::to_value(&envelope).unwrap()).unwrap(),
    };
    let pipe = no_app();
    let _first = harness.sender.handle(&pipe, job.clone()).await;
    let second = harness.sender.handle(&pipe, job).await;
    assert!(
        matches!(&second, Control::Result { status, error: Some(error), .. } if status == "failed" && error.contains("already used")),
        "{second:?}"
    );
    assert!(harness.received.lock().await.is_empty());
}

#[tokio::test]
async fn a_replayed_or_stale_signature_is_refused() {
    let harness = Harness::start().await;
    let (submission, store) = harness.signed_in().await;
    let session: Value = serde_json::from_str(&store.get("session").unwrap().unwrap()).unwrap();
    let key = DeviceKey::from_stored(&store.get("device_key").unwrap().unwrap()).unwrap();
    let token = session["access_token"].as_str().unwrap();
    let device_id = session["device_id"].as_str().unwrap();
    let http = reqwest::Client::new();
    let body = serde_json::to_vec(&ticket_body(TIN, &xml(TIN), "replay")).unwrap();
    let send = |timestamp: i64, nonce: String| {
        let mut request = http
            .post(format!("{}/api/tickets", harness.base))
            .bearer_auth(token)
            .header(HEADER_DEVICE, device_id)
            .header("content-type", "application/json")
            .body(body.clone());
        for (name, value) in key.sign("POST", "/api/tickets", &body, timestamp, &nonce) {
            request = request.header(name, value);
        }
        request.send()
    };
    let now = device::now();
    let nonce = device::nonce();
    assert_eq!(send(now, nonce.clone()).await.unwrap().status(), 201);
    let replay = send(now, nonce).await.unwrap();
    assert_eq!(replay.status(), 401);
    assert_eq!(replay.json::<Value>().await.unwrap()["error"]["code"], "nonce_reused");
    let stale = send(now - 120, device::nonce()).await.unwrap();
    assert_eq!(stale.json::<Value>().await.unwrap()["error"]["code"], "clock_skew");
    // A body changed after signing.
    let mut request = http
        .post(format!("{}/api/tickets", harness.base))
        .bearer_auth(token)
        .header(HEADER_DEVICE, device_id)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&ticket_body(TIN, b"other", "replay")).unwrap());
    for (name, value) in key.sign("POST", "/api/tickets", &body, device::now(), &device::nonce()) {
        request = request.header(name, value);
    }
    let tampered = request.send().await.unwrap();
    assert_eq!(tampered.json::<Value>().await.unwrap()["error"]["code"], "bad_signature");
    let _ = submission;
}

#[tokio::test]
async fn an_expired_ticket_is_refused_by_gatekeeper_and_sender() {
    let harness = Harness::start().await;
    let (submission, store) = harness.signed_in().await;
    let submitter = submission.submitter();
    // A ticket like a real one but five minutes old.
    let body = xml(TIN);
    let issued = submitter.signed("POST", "/api/tickets", &ticket_body(TIN, &body, "expired")).await.unwrap();
    let mut claims: TicketClaims = serde_json::from_value(
        serde_json::from_slice::<Value>(
            &URL_SAFE_NO_PAD.decode(issued["ticket"].as_str().unwrap().split('.').nth(1).unwrap()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    claims.iat -= 600;
    claims.exp -= 600;
    let expired = tickets::sign(&harness.ticket_key, &claims);
    let envelope = seal::seal(&sealing_key(submitter).await, &claims.jti.to_string(), &body).unwrap();
    let posted =
        submitter.signed("POST", "/api/v2/submissions", &json!({ "ticket": expired, "envelope": envelope })).await;
    assert_eq!(server_code(posted), "ticket_expired");

    let job = Job::V2 {
        submission_id: Uuid::new_v4(),
        account_id: harness.account_id,
        ticket: expired,
        envelope: serde_json::from_value(serde_json::to_value(&envelope).unwrap()).unwrap(),
    };
    let result = harness.sender.handle(&no_app(), job).await;
    assert!(
        matches!(&result, Control::Result { status, error: Some(error), .. } if status == "failed" && error.contains("expired")),
        "{result:?}"
    );
    let _ = store;
}

#[tokio::test]
async fn a_tampered_envelope_does_not_open() {
    let harness = Harness::start().await;
    let (submission, _) = harness.signed_in().await;
    let submitter = submission.submitter();
    let body = xml(TIN);
    let ticket = submitter.signed("POST", "/api/tickets", &ticket_body(TIN, &body, "tamper")).await.unwrap();
    let jti = ticket["jti"].as_str().unwrap();
    let mut envelope = seal::seal(&sealing_key(submitter).await, jti, &body).unwrap();
    let mut ct = URL_SAFE_NO_PAD.decode(&envelope.ct).unwrap();
    ct[10] ^= 0x01;
    envelope.ct = URL_SAFE_NO_PAD.encode(ct);
    // The Gatekeeper cannot tell: it stores the envelope as it came.
    let view = submitter
        .signed("POST", "/api/v2/submissions", &json!({ "ticket": ticket["ticket"], "envelope": envelope }))
        .await
        .unwrap();
    let ended = submitter.relay(view["relay_path"].as_str().unwrap()).await.unwrap();
    assert!(
        matches!(&ended, Ended::Result { status, error: Some(error), .. } if status == "failed" && error.contains("does not open")),
        "{ended:?}"
    );
    assert_eq!(harness.submission_status(view["id"].as_str().unwrap()).await, "failed");
    assert!(harness.received.lock().await.is_empty(), "nothing reached the SFTP server");
}

#[tokio::test]
async fn an_xml_tin_other_than_the_tickets_is_refused() {
    let harness = Harness::start().await;
    let (submission, _) = harness.signed_in().await;
    let submitter = submission.submitter();
    // The ticket is for Juana's TIN, and its hash is of the XML sealed, but
    // that XML files for another TIN.
    let other = xml("00012345400000");
    let (_, _, view) = submit(submitter, &other, &other, "swap").await;
    let ended = submitter.relay(view["relay_path"].as_str().unwrap()).await.unwrap();
    assert!(
        matches!(&ended, Ended::Result { status, error: Some(error), .. }
            if status == "failed" && error == "the TIN in the return is not the ticket's TIN"),
        "{ended:?}"
    );
    assert!(harness.received.lock().await.is_empty());

    // A hash that does not match the sealed XML is refused too.
    let (_, _, view) = submit(submitter, &xml(TIN), &other, "swap-2").await;
    let ended = submitter.relay(view["relay_path"].as_str().unwrap()).await.unwrap();
    assert!(
        matches!(&ended, Ended::Result { status, error: Some(error), .. } if status == "failed" && error.contains("not the one the ticket names")),
        "{ended:?}"
    );
    assert!(harness.received.lock().await.is_empty());
}

#[tokio::test]
async fn a_revoked_device_is_refused_everywhere() {
    let harness = Harness::start().await;
    let (submission, store) = harness.signed_in().await;
    let submitter = submission.submitter();
    let body = xml(TIN);
    let (_, _, view) = submit(submitter, &body, &body, "revoked").await;
    let session: Value = serde_json::from_str(&store.get("session").unwrap().unwrap()).unwrap();
    let device_id: Uuid = session["device_id"].as_str().unwrap().parse().unwrap();
    let token = session["access_token"].as_str().unwrap().to_owned();

    // Juana removes the device on the account site.
    assert!(buwiz_cloud::devices::revoke(&harness.db, harness.account_id, device_id).await.unwrap());

    // Its stored submission can no longer be relayed: its sign-in is over.
    let relay_path = view["relay_path"].as_str().unwrap();
    assert_eq!(raw_signed(&harness, &store, "GET", relay_path, b"").await, (401, "invalid_token".to_owned()));
    let relay = submitter.relay(relay_path).await;
    assert!(matches!(&relay, Err(ClientError::Server { status: 401, .. })), "{relay:?}");
    assert_eq!(harness.submission_status(view["id"].as_str().unwrap()).await, "ready");
    let me = reqwest::Client::new().get(format!("{}/me", harness.base)).bearer_auth(&token).send().await.unwrap();
    assert_eq!(me.status(), 401);
    // It gets no ticket: the token is refused, and so is the refresh.
    let ticket_request = serde_json::to_vec(&ticket_body(TIN, &body, "revoked-2")).unwrap();
    assert_eq!(
        raw_signed(&harness, &store, "POST", "/api/tickets", &ticket_request).await,
        (401, "invalid_token".to_owned())
    );
    let ticket = submitter.signed("POST", "/api/tickets", &ticket_body(TIN, &body, "revoked-2")).await;
    assert!(matches!(ticket, Err(ClientError::SignedOut)), "{ticket:?}");
    assert!(harness.received.lock().await.is_empty());

    // A device revoked while its sign-in lives on is refused by its
    // signature: device_revoked, for tickets and for the relay.
    let (second, second_store) = harness.signed_in().await;
    let (_, _, second_view) = submit(second.submitter(), &body, &body, "revoked-4").await;
    let session: Value = serde_json::from_str(&second_store.get("session").unwrap().unwrap()).unwrap();
    sqlx::query("UPDATE devices SET revoked_at = now() WHERE id = $1::uuid")
        .bind(session["device_id"].as_str().unwrap())
        .execute(&harness.db)
        .await
        .unwrap();
    let ticket_request = serde_json::to_vec(&ticket_body(TIN, &body, "revoked-3")).unwrap();
    assert_eq!(
        raw_signed(&harness, &second_store, "POST", "/api/tickets", &ticket_request).await,
        (403, "device_revoked".to_owned())
    );
    let second_relay = second_view["relay_path"].as_str().unwrap();
    assert_eq!(raw_signed(&harness, &second_store, "GET", second_relay, b"").await, (403, "device_revoked".to_owned()));
    assert_eq!(harness.submission_status(second_view["id"].as_str().unwrap()).await, "ready");
    let refused = second.submitter().signed("POST", "/api/tickets", &ticket_body(TIN, &body, "revoked-3")).await;
    assert!(matches!(&refused, Err(ClientError::Other(message)) if message.contains("removed")), "{refused:?}");
}

#[tokio::test]
async fn a_trust_list_signed_by_another_key_is_refused_and_the_pipe_opens_nothing() {
    let harness = Harness::start().await;
    // A good client stores a sealed return...
    let (submission, _) = harness.signed_in().await;
    let body = xml(TIN);
    let (_, _, view) = submit(submission.submitter(), &body, &body, "trust").await;

    // ...but an app that pins another trust key trusts no list this
    // server serves: it sends nothing and opens no pipe.
    let mut config = harness.client_config();
    config.trust_keys = vec![common::trust_public(&[0x66; 32])];
    let (stranger, _) = harness.signed_in_with(config).await;
    let refused = stranger.submitter().send(&format!("{TIN}-1701Qv2018-2026Q1#{EMAIL}#.xml"), &body).await;
    assert_eq!(
        refused.err(),
        Some(bir_submit::SendError::NotSent(
            "Can't send: the server's trust list is not signed by a pinned key.".into()
        ))
    );
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets").fetch_one(&harness.db).await.unwrap();
    assert_eq!(tickets, 1, "no ticket was spent on the refused send");
    let relay = stranger.submitter().relay(view["relay_path"].as_str().unwrap()).await;
    assert!(
        matches!(&relay, Err(ClientError::Other(message)) if message.contains("not signed by a pinned key")),
        "{relay:?}"
    );
    assert_eq!(harness.submission_status(view["id"].as_str().unwrap()).await, "ready", "the relay never opened");
    assert!(harness.received.lock().await.is_empty());
}

#[tokio::test]
async fn a_host_off_the_signed_list_is_refused() {
    // The list names the dispatcher but not the SFTP server.
    let harness =
        Harness::start_with(|spec| spec.contents.hosts = vec![format!("127.0.0.1:{}", spec.dispatcher_port)]).await;
    let (submission, _) = harness.signed_in().await;
    let body = xml(TIN);
    let (_, _, view) = submit(submission.submitter(), &body, &body, "off-list").await;
    let ended = submission.submitter().relay(view["relay_path"].as_str().unwrap()).await.unwrap();
    assert!(
        matches!(&ended, Ended::Result { status, error: Some(error), .. }
            if status == "failed" && error.contains("is not on the signed trust list")),
        "{ended:?}"
    );
    assert!(harness.received.lock().await.is_empty());
}

#[tokio::test]
async fn a_sealing_key_not_in_the_signed_list_is_refused_and_nothing_is_sealed() {
    // The Gatekeeper names the Sender's real key, but the signed list
    // carries another one: as if the Gatekeeper served a key of its own.
    let harness = Harness::start_with(|spec| {
        let other = buwiz_cloud::envelope::SealingKey::from_bytes(&[0x77; 32]).unwrap();
        spec.contents.sealing_keys = other.public_document()["keys"].as_array().unwrap().clone();
    })
    .await;
    let (submission, _) = harness.signed_in().await;
    let body = xml(TIN);
    let refused = submission.submitter().send(&format!("{TIN}-1701Qv2018-2026Q1#{EMAIL}#.xml"), &body).await;
    assert_eq!(
        refused.err(),
        Some(bir_submit::SendError::NotSent(
            "Can't send: the server's sealing key is not in the signed trust list.".into()
        ))
    );
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets").fetch_one(&harness.db).await.unwrap();
    let submissions: i64 = sqlx::query_scalar("SELECT count(*) FROM submissions").fetch_one(&harness.db).await.unwrap();
    assert_eq!((tickets, submissions), (0, 0), "no ticket asked for, no envelope posted");
    assert!(harness.received.lock().await.is_empty());
}

#[tokio::test]
async fn a_receipt_signed_by_a_key_not_in_the_signed_list_does_not_make_the_return_sent() {
    // The signed list vouches for another receipt key than the one the
    // Sender signs with: a forged "Sent" must not be believed.
    let harness = Harness::start_with(|spec| {
        let other = ed25519_dalek::SigningKey::from_bytes(&[0x78; 32]);
        spec.contents.receipt_keys = vec![buwiz_cloud::jws::jwk(&other.verifying_key())];
    })
    .await;
    let (submission, _) = harness.signed_in().await;
    let body = xml(TIN);
    let transport = submission.transport();
    let outcome = bir_core::background_cron::SubmissionTransport::store_session_reported(
        transport.as_ref(),
        (),
        &format!("{TIN}-1701Qv2018-2026Q1#{EMAIL}#.xml"),
        &body,
    )
    .await;
    match outcome {
        Err(bir_core::background_cron::StoreFailure::MaybeSent(error)) => assert!(
            error.0.contains("its receipt does not check out")
                && error.0.contains("signed by a key not in the signed trust list"),
            "{error}"
        ),
        Ok(report) => panic!("the queue would mark it Sent: {report:?}"),
        Err(other) => panic!("expected needs-attention, got {other:?}"),
    }
}
