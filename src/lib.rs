//! buwiz-cloud: accounts, TIN activation, app sign-in and the submission
//! relay for the Buwiz apps.

pub mod accounts;
pub mod agents;
pub mod bir;
pub mod canonical;
pub mod claims;
pub mod config;
pub mod crypto;
pub mod devices;
pub mod envelope;
pub mod error;
pub mod jws;
pub mod link;
pub mod mail;
pub mod mcp;
pub mod notice;
pub mod oauth;
pub mod pages;
pub mod relay;
pub mod sender;
pub mod state;
pub mod storage;
pub mod submissions;
pub mod tickets;
pub mod tin;
pub mod trust;
pub mod web;
use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::link::SenderLink;
use crate::state::AppState;

/// The database migrations, for tools and tests that set up their own.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// Reads settings, migrates the database and serves until Ctrl-C.
pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let config = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("could not connect to DATABASE_URL")?;
    MIGRATOR.run(&db).await.context("migrations failed")?;

    let documents = config::document_store(config.secure_cookies)?;
    let ticket_key = config::ticket_key(config.secure_cookies)?;
    let sender = match config::sender_url(config.secure_cookies)? {
        Some((url, secret)) => SenderLink::new(&url, secret),
        None => embedded_sender(&ticket_key, &config.account_origin).await?,
    };
    tokio::spawn({
        let db = db.clone();
        async move {
            loop {
                if let Err(error) = submissions::upkeep(&db).await {
                    tracing::error!(error = ?error, "submission upkeep failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        }
    });

    let address = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config, db, documents, ticket_key, sender);
    let app = web::router(state.clone(), pages::router(state)).layer(axum::middleware::from_fn(canonical::bare_domain));
    let listener =
        tokio::net::TcpListener::bind(&address).await.with_context(|| format!("could not bind {address}"))?;
    tracing::info!("listening on http://{address}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// Development only: runs the Sender in this process on a loopback port.
/// Public deployments run `buwiz-sender` on its own (`SENDER_URL`).
async fn embedded_sender(ticket_key: &ed25519_dalek::SigningKey, issuer: &str) -> anyhow::Result<SenderLink> {
    let config = sender::SenderConfig::embedded(ticket_key.verifying_key(), issuer)?;
    let secret = config.secret.clone();
    tracing::warn!(mode = config.relay.mode.as_str(), "SENDER_URL is not set; running the Sender in this process");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let sender = sender::Sender::new(config);
    tokio::spawn(async move {
        if let Err(error) = sender::serve(listener, sender).await {
            tracing::error!(error = ?error, "embedded Sender stopped");
        }
    });
    Ok(SenderLink::new(&url, secret))
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
