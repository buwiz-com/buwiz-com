//! Shared by the site tests: a [`Site`] built on its own database, and the
//! helpers more than one test file uses.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, header};
use axum::response::Response;
use buwiz_cloud::account::accounts;
use buwiz_cloud::base::config::{Config, MailConfig};
use buwiz_cloud::base::crypto;
use buwiz_cloud::base::state::AppState;
use buwiz_cloud::base::storage::DocumentStore;
use buwiz_cloud::filing::link::SenderLink;
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
}

impl Site {
    pub fn new(db: PgPool) -> Self {
        Self::with_store(db, DocumentStore::Database)
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
        let config = Config {
            host: "127.0.0.1".into(),
            port: 0,
            database_url: String::new(),
            account_origin: ORIGIN.into(),
            extra_origins: Vec::new(),
            secure_cookies: false,
            document_key: [7; 32],
            admin_emails: vec![STAFF.into()],
            mail: MailConfig::Log,
            trust_list: None,
            downloads_url: downloads_url.into(),
            forms_origin: FORMS.into(),
            // Nothing listens there: the star count is unknown.
            github_api_url: "http://127.0.0.1:9".into(),
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        // The account site never reaches the Sender in these tests.
        let sender = SenderLink::new("http://127.0.0.1:9", Zeroizing::new("unused".into()));
        let state = AppState::new(config, db.clone(), store, key, sender);
        Self { db, router: buwiz_cloud::app(state) }
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
    let grant: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_grants (account_id, client_id, scope) VALUES ($1, 'buwiz-desktop', 'app') RETURNING id",
    )
    .bind(account_id)
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
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at)
         VALUES ($1, 'business', $2, '00000', 'Juan', $3, CASE WHEN $3 IN ('approved', 'rejected') THEN now() END)
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
