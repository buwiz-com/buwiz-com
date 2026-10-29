//! The relay end to end, in one process: a fake BIR dispatcher, an SFTP
//! server with a pinned host key, the app's forwarder with its allow-list,
//! and the server sending a return through the pipe.

use std::collections::HashMap;
use std::sync::Arc;

use buwiz_cloud::bir::{self, DispatcherTarget, Job, Mode, SendError, Settings, dispatcher, iaf};
use buwiz_cloud::relay::{Control, Pipe, client};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use tokio::sync::{Mutex, mpsc};
use zeroize::Zeroizing;

const IAF_PASS: &str = "test iaf passphrase";
const DISPATCH_PASS: &str = "test dispatcher passphrase";
const SFTP_USER: &str = "lab";
const SFTP_PASSWORD: &str = "lab-password";
const XML: &[u8] = b"<?xml version=\"1.0\"?><return><tin>123456789</tin></return>";
const FILENAME: &str = "12345678900000-1601Cv2018-062025#juan@example.ph#.xml";

type Received = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// SFTP server: accepts `lab`/`lab-password` and keeps uploaded files.
struct SftpServer {
    received: Received,
    /// When set, writes are never answered: a server that stalls after the
    /// remote file was opened.
    stall: Arc<std::sync::atomic::AtomicBool>,
}

impl russh::server::Server for SftpServer {
    type Handler = SshSession;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> SshSession {
        SshSession { received: self.received.clone(), stall: self.stall.clone(), channels: HashMap::new() }
    }
}

struct SshSession {
    received: Received,
    stall: Arc<std::sync::atomic::AtomicBool>,
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
                let handler =
                    SftpFiles { received: self.received.clone(), stall: self.stall.clone(), open: HashMap::new() };
                russh_sftp::server::run(open.into_stream(), handler).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }
}

struct SftpFiles {
    received: Received,
    stall: Arc<std::sync::atomic::AtomicBool>,
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
        if self.stall.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
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
async fn sftp_server() -> (u16, String, Received) {
    sftp_server_with(Arc::default()).await
}

/// Starts the SFTP server; `stall` makes it stop answering writes.
async fn sftp_server_with(stall: Arc<std::sync::atomic::AtomicBool>) -> (u16, String, Received) {
    let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[0x42; 32])), "test").unwrap();
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let config = Arc::new(russh::server::Config { keys: vec![key], ..Default::default() });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received: Received = Arc::default();
    let mut server = SftpServer { received: received.clone(), stall };
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
async fn dispatcher_server(upload_host: &str, upload_port: u16) -> u16 {
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
                assert!(query.starts_with("t=123456789&f=1601Cv2018&v="), "{query}");
                body
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn settings(mode: Mode, dispatcher_port: u16, sftp_hosts: &[&str], pinned: &str) -> Settings {
    Settings {
        mode,
        iaf_passphrase: Zeroizing::new(IAF_PASS.into()),
        dispatcher_passphrase: Zeroizing::new(DISPATCH_PASS.into()),
        dispatchers: vec![DispatcherTarget { host: "127.0.0.1".into(), port: dispatcher_port, tls: false }],
        sftp_hosts: sftp_hosts.iter().map(|host| (*host).to_owned()).collect(),
        pinned_keys: vec![pinned.to_owned()],
        lab: None,
        tls: Settings::tls_config(),
        sftp_timeout: std::time::Duration::from_secs(5),
    }
}

/// Connects a server pipe to an app forwarder allowed to open `allowed`.
fn pipe(allowed: Vec<(String, u16)>) -> (Pipe, tokio::task::JoinHandle<Option<Control>>) {
    let (to_app, from_server) = mpsc::channel(64);
    let (to_server, from_app) = mpsc::channel(64);
    let app = tokio::spawn(async move { client::serve(to_server, from_server, &allowed).await });
    (Pipe::new(to_app, from_app), app)
}

fn job() -> Job<'static> {
    Job { tin_root: "123456789", form_type: "1601Cv2018", filename: FILENAME, xml: XML }
}

#[tokio::test]
async fn a_return_reaches_the_upload_server_through_the_pipe() {
    let (sftp_port, fingerprint, received) = sftp_server().await;
    let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
    let allowed = vec![("127.0.0.1".to_owned(), dispatcher_port), ("127.0.0.1".to_owned(), sftp_port)];
    let (pipe, app) = pipe(allowed);

    let sent = bir::send(&pipe, &job(), &settings(Mode::Live, dispatcher_port, &["127.0.0.1"], &fingerprint))
        .await
        .expect("sent");
    pipe.finish(Control::Result { status: "sent".into(), error: None, receipt: None }).await;

    assert_eq!(sent.remote_path, format!("/1601Cv2018/{FILENAME}"));
    let files = received.lock().await;
    let uploaded = files.get(&sent.remote_path).expect("file on the server");
    assert_eq!(iaf::decrypt(uploaded, IAF_PASS).unwrap(), XML, "BIR gets the IAF-encrypted return");
    assert_ne!(uploaded.as_slice(), XML);
    assert!(matches!(app.await.unwrap(), Some(Control::Result { status, .. }) if status == "sent"));
}

/// A server that stalls once the remote file is open: BIR may hold part
/// of the file, so the send is "maybe sent", never "not sent".
#[tokio::test]
async fn a_stall_after_the_remote_file_is_opened_may_have_reached_bir() {
    let (sftp_port, fingerprint, received) = sftp_server_with(Arc::new(true.into())).await;
    let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
    let allowed = vec![("127.0.0.1".to_owned(), dispatcher_port), ("127.0.0.1".to_owned(), sftp_port)];
    let (pipe, _app) = pipe(allowed);
    let mut settings = settings(Mode::Live, dispatcher_port, &["127.0.0.1"], &fingerprint);
    settings.sftp_timeout = std::time::Duration::from_secs(3);

    let error = bir::send(&pipe, &job(), &settings).await.expect_err("stalled");
    assert!(
        matches!(&error, SendError::MaybeSent(message) if message.contains("once the remote file was asked for")),
        "{error}"
    );
    assert!(received.lock().await.is_empty(), "the file never closed");
}

#[tokio::test]
async fn an_unpinned_host_key_stops_before_the_password_is_sent() {
    let (sftp_port, _fingerprint, received) = sftp_server().await;
    let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
    let allowed = vec![("127.0.0.1".to_owned(), dispatcher_port), ("127.0.0.1".to_owned(), sftp_port)];
    let (pipe, _app) = pipe(allowed);
    let wrong_pin = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    let error = bir::send(&pipe, &job(), &settings(Mode::Live, dispatcher_port, &["127.0.0.1"], wrong_pin))
        .await
        .expect_err("refused");
    assert!(matches!(&error, SendError::NotSent(message) if message.contains("host key")), "{error}");
    assert!(received.lock().await.is_empty());
}

#[tokio::test]
async fn the_app_refuses_hosts_off_its_allow_list() {
    let (sftp_port, fingerprint, received) = sftp_server().await;
    let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
    // The app allows the dispatcher but not the upload server.
    let (pipe, _app) = pipe(vec![("127.0.0.1".to_owned(), dispatcher_port)]);

    let error = bir::send(&pipe, &job(), &settings(Mode::Live, dispatcher_port, &["127.0.0.1"], &fingerprint))
        .await
        .expect_err("refused");
    assert!(matches!(&error, SendError::NotSent(message) if message.contains("not a BIR server")), "{error}");
    assert!(received.lock().await.is_empty());
}

#[tokio::test]
async fn the_server_refuses_an_upload_host_it_does_not_know() {
    let (sftp_port, fingerprint, received) = sftp_server().await;
    let dispatcher_port = dispatcher_server("127.0.0.1", sftp_port).await;
    let allowed = vec![("127.0.0.1".to_owned(), dispatcher_port), ("127.0.0.1".to_owned(), sftp_port)];
    let (pipe, _app) = pipe(allowed);

    let error = bir::send(&pipe, &job(), &settings(Mode::Live, dispatcher_port, &["ebf2.bir.gov.ph"], &fingerprint))
        .await
        .expect_err("refused");
    assert!(matches!(&error, SendError::NotSent(message) if message.contains("unknown upload server")), "{error}");
    assert!(received.lock().await.is_empty());
}

#[tokio::test]
async fn a_dry_run_touches_no_network() {
    // The app allows nothing, so any connection attempt would fail.
    let (pipe, _app) = pipe(Vec::new());
    let sent = bir::send(&pipe, &job(), &settings(Mode::DryRun, 1, &[], "")).await.expect("dry run");
    assert_eq!(sent.remote_path, format!("/1601Cv2018/{FILENAME}"));
    assert_eq!(sent.payload_sha256.len(), 64);
}

/// The HTTPS dispatcher path: TLS runs inside the pipe. example.com is not
/// a dispatcher, so reaching its HTTP answer is the proof.
#[tokio::test]
#[ignore = "reaches the internet"]
async fn https_dispatcher_requests_travel_through_the_pipe() {
    let (pipe, _app) = pipe(vec![("example.com".to_owned(), 443)]);
    let mut settings = settings(Mode::Live, 443, &["example.com"], "unused");
    settings.dispatchers = vec![DispatcherTarget { host: "example.com".into(), port: 443, tls: true }];
    let error = bir::send(&pipe, &job(), &settings).await.expect_err("example.com is not a dispatcher");
    let SendError::NotSent(message) = &error else { panic!("{error}") };
    assert!(message.contains("answered") || message.contains("not understood"), "{message}");
    assert!(!message.contains("TLS"), "{message}");
}
