mod accounts;
mod claims;
mod config;
mod crypto;
mod error;
mod mail;
mod notice;
mod oauth;
mod pages;
mod state;
mod tin;
mod web;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let address = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config, db);
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
