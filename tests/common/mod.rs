//! Shared by the site tests: a [`Site`] built on its own database, and the
//! helpers more than one test file uses.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, header};
use axum::response::Response;
use buwiz_cloud::account::accounts;
use buwiz_cloud::account::verify::Verification;
use buwiz_cloud::base::config::{Config, MailConfig};
use buwiz_cloud::base::crypto;
use buwiz_cloud::base::state::AppState;
use buwiz_cloud::base::storage::DocumentStore;
use buwiz_cloud::filing::link::SenderLink;
use buwiz_cloud::uptime::Uptime;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const ORIGIN: &str = "http://localhost:3008";
pub const STAFF: &str = "staff@example.test";
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nphoto";
pub const PDF: &[u8] = b"%PDF-1.7\nscan";
/// Nothing listens on the discard port: the downloads bucket is unreachable,
/// so nothing is released.
pub const NO_DOWNLOADS: &str = "http://127.0.0.1:9";
/// The filing site's own host.
pub const FORMS: &str = "https://ebirforms.buwiz.com";
/// The filing site's host, as in [`FORMS`].
pub const FORMS_HOST: &str = "ebirforms.buwiz.com";
/// The account host that hostless test requests go to. Not [`ORIGIN`]'s own
/// host: `localhost:3008` is the Bot site.
pub const ACCOUNT_HOST: &str = "account.localhost:3008";

pub struct Site {
    pub db: PgPool,
    pub router: axum::Router,
    pub state: AppState,
}

impl Site {
    pub fn new(db: PgPool) -> Self {
        Self::with_store(db, DocumentStore::Database)
    }

    /// A site whose `ADMIN_EMAILS` is `admins` instead of [`STAFF`].
    pub fn with_admins(db: PgPool, admins: &[&str]) -> Self {
        Self::configured(
            db,
            DocumentStore::Database,
            NO_DOWNLOADS,
            admins.iter().map(|admin| (*admin).to_owned()).collect(),
        )
    }

    pub fn with_store(db: PgPool, store: DocumentStore) -> Self {
        Self::build(db, store, NO_DOWNLOADS)
    }

    /// A site whose installers are at `downloads_url`.
    pub fn with_downloads(db: PgPool, downloads_url: &str) -> Self {
        Self::build(db, DocumentStore::Database, downloads_url)
    }

    /// The filing site is on [`FORMS`], as `FORMS_ORIGIN` puts it.
    pub fn build(db: PgPool, store: DocumentStore, downloads_url: &str) -> Self {
        Self::configured(db, store, downloads_url, vec![STAFF.into()])
    }

    /// A site with automatic activation on (`AUTO_VERIFY=on`).
    pub fn with_auto(db: PgPool) -> Self {
        Self::configured_with(db, DocumentStore::Database, NO_DOWNLOADS, vec![STAFF.into()], Verification::On)
    }

    fn configured(db: PgPool, store: DocumentStore, downloads_url: &str, admin_emails: Vec<String>) -> Self {
        // As in production: from `AUTO_VERIFY`, which the test runner never
        // sets, so off.
        Self::configured_with(db, store, downloads_url, admin_emails, Verification::from_env())
    }

    /// A site whose Sender is at `sender_url` (a test's own fake, or
    /// nothing) with `secret`, and BIR uptime as `uptime` says.
    pub fn with_sender(db: PgPool, sender_url: &str, secret: &str, uptime: Uptime) -> Self {
        let config = test_config(NO_DOWNLOADS, vec![STAFF.into()]);
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let sender = SenderLink::new(sender_url, Zeroizing::new(secret.into()));
        let state = AppState::new(config, db.clone(), DocumentStore::Database, key, sender).with_uptime(uptime);
        Self { db, router: buwiz_cloud::app(state.clone()), state }
    }

    fn configured_with(
        db: PgPool,
        store: DocumentStore,
        downloads_url: &str,
        admin_emails: Vec<String>,
        verification: Verification,
    ) -> Self {
        let config = test_config(downloads_url, admin_emails);
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        // The account site never reaches the Sender in these tests.
        let sender = SenderLink::new("http://127.0.0.1:9", Zeroizing::new("unused".into()));
        let state = AppState::new(config, db.clone(), store, key, sender).with_verification(verification);
        Self { db, router: buwiz_cloud::app(state.clone()), state }
    }

    /// A request without a `Host` goes to the account host, [`ACCOUNT_HOST`].
    pub async fn send(&self, mut request: Request<Body>) -> Response {
        request.headers_mut().entry(header::HOST).or_insert(header::HeaderValue::from_static(ACCOUNT_HOST));
        self.router.clone().oneshot(request).await.unwrap()
    }

    /// A signed-in account: (id, cookie header value).
    pub async fn person(&self, email: &str) -> (Uuid, String) {
        let id: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) RETURNING id")
            .bind(email)
            .fetch_one(&self.db)
            .await
            .unwrap();
        let token = accounts::start_session(&self.db, id).await.unwrap();
        (id, format!("buwiz_session={token}"))
    }

    /// A new website session for an existing account: its cookie.
    pub async fn person_session(&self, id: Uuid) -> String {
        let token = accounts::start_session(&self.db, id).await.unwrap();
        format!("buwiz_session={token}")
    }

    /// A signed-in account with the stored staff role `role` (`admin`,
    /// `support`).
    pub async fn staff_with_role(&self, email: &str, role: &str) -> (Uuid, String) {
        let (id, cookie) = self.person(email).await;
        sqlx::query("UPDATE accounts SET role = $2 WHERE id = $1").bind(id).bind(role).execute(&self.db).await.unwrap();
        (id, cookie)
    }

    pub async fn get(&self, path: &str, cookie: &str) -> Response {
        self.send(Request::get(path).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await
    }

    pub async fn on_host(&self, host: &str, path: &str) -> Response {
        self.send(Request::get(path).header(header::HOST, host).body(Body::empty()).unwrap()).await
    }

    pub async fn post(&self, path: &str, cookie: &str, form: &str) -> Response {
        let request = Request::post(path)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form.to_owned()))
            .unwrap();
        self.send(request).await
    }

    pub async fn upload(&self, path: &str, cookie: &str, parts: &[(&str, Part)]) -> Response {
        let boundary = "buwiz-test-boundary";
        let mut body = Vec::new();
        for (name, part) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match part {
                Part::Text(value) => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                    );
                    body.extend_from_slice(value.as_bytes());
                }
                Part::File(filename, bytes) => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    body.extend_from_slice(bytes);
                }
            }
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let request = Request::post(path)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }

    pub async fn claim_status(&self, claim_id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM tin_claims WHERE id = $1")
            .bind(claim_id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    pub async fn newest_claim(&self, account_id: Uuid) -> Uuid {
        sqlx::query_scalar("SELECT id FROM tin_claims WHERE account_id = $1 ORDER BY created_at DESC LIMIT 1")
            .bind(account_id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    /// The claim's document readings once none is still running: (file,
    /// status, error).
    pub async fn readings(&self, claim_id: Uuid) -> Vec<(String, String, Option<String>)> {
        for _ in 0..100 {
            let readings: Vec<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT d.filename, r.status, r.error FROM document_readings r
                 JOIN claim_documents d ON d.id = r.document_id WHERE d.claim_id = $1 ORDER BY d.created_at",
            )
            .bind(claim_id)
            .fetch_all(&self.db)
            .await
            .unwrap();
            if readings.iter().all(|(_, status, _)| status != "reading") {
                return readings;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("document readings did not finish");
    }
}

/// The settings of every test site.
fn test_config(downloads_url: &str, admin_emails: Vec<String>) -> Config {
    Config {
        host: "127.0.0.1".into(),
        port: 0,
        database_url: String::new(),
        account_origin: ORIGIN.into(),
        extra_origins: Vec::new(),
        secure_cookies: false,
        document_key: [7; 32],
        admin_emails,
        mail: MailConfig::Log,
        trust_list: None,
        downloads_url: downloads_url.into(),
        forms_origin: FORMS.into(),
        // Nothing listens there: the star count is unknown.
        github_api_url: "http://127.0.0.1:9".into(),
    }
}

pub enum Part {
    Text(&'static str),
    File(&'static str, &'static [u8]),
}

pub fn location(response: &Response) -> String {
    response.headers().get(header::LOCATION).map(|value| value.to_str().unwrap().to_owned()).unwrap_or_default()
}

pub async fn bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
}

pub async fn text(response: Response) -> String {
    String::from_utf8(bytes(response).await).unwrap()
}

/// An app sign-in for `account_id` with the BIR app's scope.
pub async fn app_token(db: &PgPool, account_id: Uuid) -> String {
    token(db, account_id, "buwiz-desktop", "app").await
}

/// A sign-in for `account_id` by `client_id`, with `scope`.
pub async fn token(db: &PgPool, account_id: Uuid, client_id: &str, scope: &str) -> String {
    let grant: Uuid =
        sqlx::query_scalar("INSERT INTO oauth_grants (account_id, client_id, scope) VALUES ($1, $2, $3) RETURNING id")
            .bind(account_id)
            .bind(client_id)
            .bind(scope)
            .fetch_one(db)
            .await
            .unwrap();
    let token = crypto::new_token("bwa_");
    sqlx::query("INSERT INTO oauth_access_tokens (grant_id, token_hash, expires_at) VALUES ($1, $2, now() + interval '15 minutes')")
        .bind(grant)
        .bind(crypto::hash(&token))
        .execute(db)
        .await
        .unwrap();
    token
}

pub async fn add_claim(db: &PgPool, account_id: Uuid, root: &str, status: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ($1, 'business', $2, '00000', 'Juan', $3,
                 CASE WHEN $3 IN ('approved', 'rejected', 'removed', 'revoked') THEN now() END,
                 CASE WHEN $3 IN ('approved', 'removed', 'revoked') THEN 'staff' END)
         RETURNING id",
    )
    .bind(account_id)
    .bind(root)
    .bind(status)
    .fetch_one(db)
    .await
    .unwrap()
}

pub async fn profiles(site: &Site, token: &str, path: &str) -> Value {
    let request =
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    serde_json::from_str::<Value>(&text(site.send(request).await).await).unwrap()["profiles"].clone()
}

pub fn content_type(response: &Response) -> &str {
    response.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap()
}

/// A BIR app signed in for an account, with a registered device key: what
/// the v2 calls (tickets, sealed returns) need.
pub struct TestDevice {
    pub token: String,
    pub device_id: Uuid,
    key: ed25519_dalek::SigningKey,
}

impl TestDevice {
    pub async fn new(db: &PgPool, account_id: Uuid) -> Self {
        let token = app_token(db, account_id).await;
        let grant: Uuid = sqlx::query_scalar(
            "SELECT g.id FROM oauth_grants g JOIN oauth_access_tokens t ON t.grant_id = g.id WHERE t.token_hash = $1",
        )
        .bind(crypto::hash(&token))
        .fetch_one(db)
        .await
        .unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&rand::random());
        let device_id: Uuid = sqlx::query_scalar(
            "INSERT INTO devices (account_id, grant_id, public_key, name) VALUES ($1, $2, $3, 'Test Mac') RETURNING id",
        )
        .bind(account_id)
        .bind(grant)
        .bind(key.verifying_key().as_bytes().as_slice())
        .fetch_one(db)
        .await
        .unwrap();
        Self { token, device_id, key }
    }

    /// The device headers of a request signed with the device key, a fresh
    /// nonce and the time now.
    pub fn signature(&self, method: &str, path: &str, body: &[u8]) -> Vec<(&'static str, String)> {
        use base64::Engine;
        use ed25519_dalek::Signer;
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
        let nonce = Uuid::new_v4().simple().to_string();
        let input = buwiz_cloud::filing::devices::signing_input(
            method,
            path,
            &crypto::hex(&crypto::sha256(body)),
            timestamp,
            &nonce,
        );
        let signature =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.key.sign(input.as_bytes()).to_bytes());
        vec![
            ("buwiz-device", self.device_id.to_string()),
            ("buwiz-timestamp", timestamp.to_string()),
            ("buwiz-nonce", nonce),
            ("buwiz-signature", signature),
        ]
    }

    /// A request signed with the device key, a fresh nonce and the time now.
    pub async fn post(&self, site: &Site, path: &str, body: &Value) -> Response {
        let body = body.to_string();
        let mut request = Request::post(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json");
        for (name, value) in self.signature("POST", path, body.as_bytes()) {
            request = request.header(name, value);
        }
        site.send(request.body(Body::from(body)).unwrap()).await
    }
}

/// A plaintext eBIRForms return as its file name describes it: the form's
/// field prefix (`frm1701q` for `1701Qv2018`), the 14-digit TIN (root and
/// branch) and the year. Synthetic, with only the fields the checks read.
pub fn iaf_return(form_prefix: &str, tin: &str, year: &str) -> String {
    let line = |key: &str, value: &str| format!("<div>{form_prefix}:{key}={value}{form_prefix}:{key}=</div>\r\n");
    let mut xml = String::from("<?xml version='1.0'?>\r\n");
    for (key, value) in [
        ("txtYear", year),
        ("txtTIN1", &tin[0..3]),
        ("txtTIN2", &tin[3..6]),
        ("txtTIN3", &tin[6..9]),
        ("txtBranchCode", &tin[9..]),
    ] {
        xml += &line(key, value);
    }
    xml
}

/// The status and `{"error":{"code","message"}}` of a JSON refusal.
pub async fn refusal(response: Response) -> (u16, String, String) {
    let status = response.status().as_u16();
    let body: Value = serde_json::from_str(&text(response).await).unwrap_or_default();
    let error = &body["error"];
    (
        status,
        error["code"].as_str().unwrap_or_default().to_owned(),
        error["message"].as_str().unwrap_or_default().to_owned(),
    )
}

/// A line of `account_audit`: (action, old value, new value, actor email,
/// source, reason).
pub type AccountLine = (String, Option<String>, Option<String>, String, String, Option<String>);

/// An account's `account_audit` lines, oldest first.
pub async fn account_audit_lines(db: &PgPool, account_id: Uuid) -> Vec<AccountLine> {
    sqlx::query_as(
        "SELECT action, old_value, new_value, actor_email, source, reason FROM account_audit
         WHERE account_id = $1 ORDER BY created_at, id",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap()
}

/// An account's `claim_audit` lines, oldest first: (action, claim status,
/// actor email, source, reason).
pub async fn claim_audit_lines(db: &PgPool, account_id: Uuid) -> Vec<(String, String, String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT action, claim_status, actor_email, source, reason FROM claim_audit
         WHERE account_id = $1 ORDER BY created_at, id",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap()
}

/// Live website sessions of an account.
pub async fn live_sessions(db: &PgPool, account_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM web_sessions WHERE account_id = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(account_id)
    .fetch_one(db)
    .await
    .unwrap()
}
