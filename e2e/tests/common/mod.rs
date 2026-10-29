//! The harness: Postgres (a fresh database per test), the Gatekeeper and
//! the Sender in this process, a fake BIR dispatcher and an SFTP server
//! with a pinned host key (as in the server's `tests/relay.rs`), and an
//! approved TIN for the demo taxpayer Juana Santos.
//!
//! Nothing here reaches BIR. The Sender's dispatcher list and upload hosts
//! are 127.0.0.1 only, the passphrases are fake, and the app's allow-list
//! adds only these local ports.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use bir_submit::secrets::{MemoryStore, SecretStore, StoreError};
use bir_submit::{PollOutcome, Submission, SubmitConfig};
use buwiz_cloud::bir::{self, DispatcherTarget, Mode, dispatcher};
use buwiz_cloud::config::{Config, MailConfig};
use buwiz_cloud::envelope::SealingKey;
use buwiz_cloud::link::SenderLink;
use buwiz_cloud::sender::{Sender, SenderConfig};
use buwiz_cloud::state::AppState;
use buwiz_cloud::storage::DocumentStore;
use ed25519_dalek::SigningKey;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const IAF_PASS: &str = "test iaf passphrase";
pub const DISPATCH_PASS: &str = "test dispatcher passphrase";
pub const SFTP_USER: &str = "lab";
pub const SFTP_PASSWORD: &str = "lab-password";
/// The demo taxpayer: Juana Santos, 000-123-453-00000. Not a real person.
pub const TIN_ROOT: &str = "000123453";
pub const TIN: &str = "00012345300000";
pub const EMAIL: &str = "juana@example.com";
pub const TICKET_SEED: [u8; 32] = [0x11; 32];
/// The test egress key. The real one is made offline and never on a server.
pub const EGRESS_SEED: [u8; 32] = [0x55; 32];

/// The base64url public half of `seed`, as an app pins it.
pub fn egress_public(seed: &[u8; 32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(seed).verifying_key().as_bytes())
}
const SENDER_SECRET: &str = "e2e-sender-secret-0123456789abcdef";

pub type Received = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// SFTP server: accepts `lab`/`lab-password` and keeps uploaded files.
struct SftpServer {
    received: Received,
}

impl russh::server::Server for SftpServer {
    type Handler = SshSession;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> SshSession {
        SshSession { received: self.received.clone(), channels: HashMap::new() }
    }
}

struct SshSession {
    received: Received,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == SFTP_USER && password == SFTP_PASSWORD { Ok(Auth::Accept) } else { Ok(Auth::reject()) }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.channels.remove(&channel) {
            Some(open) if name == "sftp" => {
                session.channel_success(channel)?;
                let handler = SftpFiles { received: self.received.clone(), open: HashMap::new() };
                russh_sftp::server::run(open.into_stream(), handler).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }
}

struct SftpFiles {
    received: Received,
    open: HashMap<String, (String, Vec<u8>)>,
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

impl russh_sftp::server::Handler for SftpFiles {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _version: u32, _extensions: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::new(path, FileAttributes::default())] })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let handle = format!("h{id}");
        self.open.insert(handle.clone(), (filename, Vec::new()));
        Ok(Handle { id, handle })
    }

    async fn write(&mut self, id: u32, handle: String, _offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        if let Some((_, buffer)) = self.open.get_mut(&handle) {
            buffer.extend_from_slice(&data);
        }
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if let Some((path, buffer)) = self.open.remove(&handle) {
            self.received.lock().await.insert(path, buffer);
        }
        Ok(ok(id))
    }
}

/// Starts the SFTP server. Returns its port, its key fingerprint and the
/// files it receives.
pub async fn sftp_server() -> (u16, String, Received) {
    let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[0x42; 32])), "test").unwrap();
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let config = Arc::new(russh::server::Config { keys: vec![key], ..Default::default() });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Received = Arc::default();
    let mut server = SftpServer { received: received.clone() };
    tokio::spawn(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let handler = server.new_client(Some(peer));
            let config = config.clone();
            tokio::spawn(async move {
                let _ = russh::server::run_stream(config, socket, handler).await;
            });
        }
    });
    (port, fingerprint, received)
}

/// A fake `tinDispatcherSFTP.php` that names `upload_host:upload_port`.
pub async fn dispatcher_server(upload_host: &str, upload_port: u16) -> u16 {
    let wrap = |plain: &str| dispatcher::wrap_field(plain, DISPATCH_PASS);
    let body = format!(
        "{{'mode':'1','server':'{}','SSLPort':'443','port':'{upload_port}','username':'{}','password':'{}'}}",
        wrap(upload_host),
        wrap(SFTP_USER),
        wrap(SFTP_PASSWORD),
    );
    let app = axum::Router::new().route(
        dispatcher::PATH,
        axum::routing::get(move |query: axum::extract::RawQuery| {
            let body = body.clone();
            async move {
                let query = query.0.unwrap_or_default();
                assert!(query.starts_with("t=000123453&f=1701Qv2018&v="), "{query}");
                body
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A secret store the test can read too.
#[derive(Clone, Default)]
pub struct SharedStore(pub Arc<MemoryStore>);

impl SecretStore for SharedStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        self.0.get(name)
    }
    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        self.0.set(name, value)
    }
    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.delete(name)
    }
}

pub struct Harness {
    pub db: PgPool,
    pub base: String,
    pub account_id: Uuid,
    pub ticket_key: SigningKey,
    pub received: Received,
    pub sender: Arc<Sender>,
    pub sftp_port: u16,
    pub dispatcher_port: u16,
}

/// A fresh database on `DATABASE_URL`'s server, migrated.
async fn fresh_database() -> PgPool {
    let admin_url = std::env::var("DATABASE_URL").expect("DATABASE_URL (run scripts/v6.sh)");
    let admin = PgPoolOptions::new().max_connections(1).connect(&admin_url).await.expect("Postgres");
    let name = format!("e2e_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&admin).await.unwrap();
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    let db = PgPoolOptions::new().max_connections(5).connect(&format!("{base}/{name}")).await.unwrap();
    buwiz_cloud::MIGRATOR.run(&db).await.unwrap();
    db
}

impl Harness {
    /// The egress list names the fake dispatcher and the SFTP server.
    pub async fn start() -> Self {
        Self::start_with(|dispatcher, sftp| vec![format!("127.0.0.1:{dispatcher}"), format!("127.0.0.1:{sftp}")]).await
    }

    /// `egress` picks the signed list's hosts from the dispatcher and SFTP
    /// ports.
    pub async fn start_with(egress: impl FnOnce(u16, u16) -> Vec<String>) -> Self {
        let db = fresh_database().await;
        let (sftp_port, fingerprint, received) = sftp_server().await;
        let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let egress_list = buwiz_cloud::egress::sign_list(
            &SigningKey::from_bytes(&EGRESS_SEED),
            &egress(dispatcher_port, sftp_port),
            now,
            now + 3600,
        )
        .unwrap();

        // The Sender, with fake passphrases and local hosts only.
        let ticket_key = SigningKey::from_bytes(&TICKET_SEED);
        let sender = Sender::new(SenderConfig {
            relay: bir::Settings {
                mode: Mode::Live,
                iaf_passphrase: Zeroizing::new(IAF_PASS.into()),
                dispatcher_passphrase: Zeroizing::new(DISPATCH_PASS.into()),
                dispatchers: vec![DispatcherTarget { host: "127.0.0.1".into(), port: dispatcher_port, tls: false }],
                sftp_hosts: vec!["127.0.0.1".into()],
                pinned_keys: vec![fingerprint],
                lab: None,
                tls: bir::Settings::tls_config(),
            },
            sealing_key: SealingKey::from_bytes(&[0x22; 32]).unwrap(),
            receipt_key: SigningKey::from_bytes(&[0x33; 32]),
            ticket_keys: vec![ticket_key.verifying_key()],
            secret: Zeroizing::new(SENDER_SECRET.into()),
            issuer: "http://buwiz.test".into(),
        });
        let sender_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sender_url = format!("http://{}", sender_listener.local_addr().unwrap());
        tokio::spawn(buwiz_cloud::sender::serve(sender_listener, sender.clone()));

        // The Gatekeeper: no BIR settings, no HPKE private key.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let config = Config {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            database_url: String::new(),
            account_origin: base.clone(),
            extra_origins: Vec::new(),
            secure_cookies: false,
            document_key: [0x44; 32],
            admin_emails: Vec::new(),
            mail: MailConfig::Log,
            egress_list: Some(egress_list),
        };
        let state = AppState::new(
            config,
            db.clone(),
            DocumentStore::Database,
            ticket_key.clone(),
            SenderLink::new(&sender_url, Zeroizing::new(SENDER_SECRET.into())),
        );
        let app = buwiz_cloud::web::router(state.clone(), buwiz_cloud::pages::router(state));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // Juana's account, with her TIN approved by staff.
        let account_id: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) RETURNING id")
            .bind(EMAIL)
            .fetch_one(&db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tin_claims (account_id, tin_root, branch_code, registered_name, status, decided_at)
             VALUES ($1, $2, '00000', 'Juana Santos', 'approved', now())",
        )
        .bind(account_id)
        .bind(TIN_ROOT)
        .execute(&db)
        .await
        .unwrap();
        Self { db, base, account_id, ticket_key, received, sender, sftp_port, dispatcher_port }
    }

    /// The app's client config: this server, with the test egress key
    /// pinned through the override.
    pub fn client_config(&self) -> SubmitConfig {
        let mut config = SubmitConfig::new(Some(&self.base));
        config.server_url = self.base.clone();
        config.egress_keys = vec![egress_public(&EGRESS_SEED)];
        config.device_name = "Juana's laptop".into();
        config
    }

    /// Device-flow sign-in with the real client; Juana approves on the
    /// account site (here: straight in the database layer).
    pub async fn signed_in(&self) -> (Submission, SharedStore) {
        self.signed_in_with(self.client_config()).await
    }

    pub async fn signed_in_with(&self, config: SubmitConfig) -> (Submission, SharedStore) {
        let store = SharedStore::default();
        let submission = Submission::with(config, Box::new(store.clone()));
        let code = submission.start_sign_in().await.expect("device flow starts");
        assert!(code.verification_uri_complete.contains(&code.user_code));
        assert!(
            buwiz_cloud::oauth::decide_device(&self.db, &code.user_code, self.account_id, true).await.unwrap(),
            "approved"
        );
        assert_eq!(submission.poll_sign_in().await.expect("poll"), PollOutcome::SignedIn);
        (submission, store)
    }

    pub async fn submission_status(&self, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM submissions WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}
