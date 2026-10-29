//! buwiz-cloud: accounts, TIN activation, app sign-in and the submission
//! relay for the Buwiz apps.

pub mod accounts;
pub mod bir;
pub mod claims;
pub mod config;
pub mod crypto;
pub mod error;
pub mod mail;
pub mod notice;
pub mod oauth;
pub mod pages;
pub mod relay;
pub mod state;
pub mod submissions;
pub mod tin;
pub mod web;
use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

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
    sqlx::migrate!().run(&db).await.context("migrations failed")?;

    let relay = config::relay_settings()?;
    let receipt_key = config::receipt_key(config.secure_cookies)?;
    tracing::info!(mode = relay.mode.as_str(), "submission relay");
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
    let state = AppState::new(config, db, relay, receipt_key);
    let app = web::router(state.clone(), pages::router(state));
    let listener =
        tokio::net::TcpListener::bind(&address).await.with_context(|| format!("could not bind {address}"))?;
    tracing::info!("listening on http://{address}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
