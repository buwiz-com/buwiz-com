//! The Gatekeeper's side of the Sender: where it is, the secret it
//! presents, and the public keys it publishes for apps. The Gatekeeper
//! never holds BIR's passphrases or the HPKE private key; it only bridges
//! the app's pipe to the Sender (see [`crate::sender`]).

use anyhow::Context;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::OnceCell;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use zeroize::Zeroizing;

pub type Upstream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct SenderLink {
    /// `http(s)://host:port` of the Sender.
    base: String,
    secret: Zeroizing<String>,
    public: OnceCell<Value>,
    http: reqwest::Client,
}

impl SenderLink {
    pub fn new(base: &str, secret: Zeroizing<String>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            secret,
            public: OnceCell::new(),
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().expect("client"),
        }
    }

    /// The Sender's public keys: `{"receipt_keys":[JWK…],"hpke":{…}}`.
    /// Fetched once.
    pub async fn public(&self) -> anyhow::Result<&Value> {
        self.public
            .get_or_try_init(|| async {
                let response = self
                    .http
                    .get(format!("{}/internal/public", self.base))
                    .bearer_auth(self.secret.as_str())
                    .send()
                    .await
                    .context("the Sender is not reachable")?
                    .error_for_status()
                    .context("the Sender refused")?;
                response.json::<Value>().await.context("the Sender's keys are unreadable")
            })
            .await
    }

    /// Opens a job socket to the Sender.
    pub async fn open(&self) -> anyhow::Result<Upstream> {
        let url = format!("{}/internal/relay", self.base.replacen("http", "ws", 1));
        let mut request = url.into_client_request().context("bad SENDER_URL")?;
        request.headers_mut().insert(
            http::header::AUTHORIZATION,
            format!("Bearer {}", self.secret.as_str()).parse().context("bad SENDER_SECRET")?,
        );
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(crate::submissions::BODY_LIMIT));
        let (socket, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), true)
            .await
            .context("the Sender is not reachable")?;
        Ok(socket)
    }
}
