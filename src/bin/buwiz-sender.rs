//! The Sender: holds BIR's passphrases, the SFTP login and the HPKE private
//! key, and sends returns for the Gatekeeper. Run it on a private network
//! the Gatekeeper reaches at `SENDER_URL`; never expose it publicly.
//!
//! Settings: `SENDER_HOST` (127.0.0.1), `SENDER_PORT` (3009), and those of
//! [`buwiz_cloud::sender::SenderConfig::from_env`].

use anyhow::Context;
use buwiz_cloud::sender::{self, Sender, SenderConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let config = SenderConfig::from_env()?;
    tracing::info!(mode = config.relay.mode.as_str(), "sender");
    let host = std::env::var("SENDER_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = std::env::var("SENDER_PORT").unwrap_or_else(|_| "3009".to_owned());
    let address = format!("{host}:{port}");
    let listener =
        tokio::net::TcpListener::bind(&address).await.with_context(|| format!("could not bind {address}"))?;
    tracing::info!("sender listening on http://{address}");
    sender::serve(listener, Sender::new(config)).await
}
