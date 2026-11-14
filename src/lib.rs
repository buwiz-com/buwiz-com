//! buwiz-cloud: accounts, TIN activation, app sign-in and the submission
//! relay for the Buwiz apps.

pub mod account;
pub mod base;
#[cfg(feature = "bir")]
// Not in this repository (docs/relay.md); rustfmt must not look for it.
#[rustfmt::skip]
pub mod bir;
pub mod filing;
pub mod ocr;
pub mod relay;
pub mod sender;
pub mod sites;
pub mod uptime;

// The old flat paths (`buwiz_cloud::config`, `crate::tickets`, ...) stay as
// re-exports: the private BIR files restored only on the owner's machine
// (`src/bir/`, `src/relay/client.rs`, their tests and example) and `e2e/`
// use them.
pub use account::{accounts, agents, claims, mail, mcp, notice, oauth, pages, staff, tin, web};
pub use base::{config, crypto, error, state, storage};
pub use filing::{devices, envelope, jws, link, submissions, tickets, trust};
pub use sites::{bot_site, canonical, downloads, github, seo};

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use crate::base::config::Config;
use crate::base::state::AppState;
use crate::filing::link::SenderLink;

/// The database migrations, for tools and tests that set up their own.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// How long, at shutdown, the BIR uptime records still being written get.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Reads settings, migrates the database and serves until SIGTERM (what
/// systemd sends to stop it) or Ctrl-C. Then it is stopping: a request that
/// would start a send (a ticket, a submission, the relay) is answered `503
/// restarting` with `Retry-After`, while requests already under way go on
/// and the BIR uptime records of tries that end get written, for at most 10
/// seconds; the BIR sign's evaluation time is written (at most 5 seconds);
/// then the process exits at once, without exit-time destructors, so
/// blocking work (a document reading) cannot hold the restart. Connections still open (a relay send under way)
/// end with the process, as they did when SIGTERM killed it. SIGUSR1, at
/// any time, only makes the BIR sign take in a note the `bir` command wrote
/// ([`uptime::poke`]).
pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    // First of all, so a SIGUSR1 during the start (the migrations) is held
    // for later instead of ending the process.
    let pokes = pokes();
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

    let address = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config, db, documents, ticket_key, sender);
    #[cfg(unix)]
    if let Some(mut pokes) = pokes {
        let state = state.clone();
        tokio::spawn(async move {
            while pokes.recv().await.is_some() {
                uptime::poke(&state).await;
            }
        });
    }
    #[cfg(not(unix))]
    let () = pokes;
    if let Err(error) = ocr::resume(&state).await {
        tracing::error!(error = ?error, "could not resume document readings");
    }
    let uptime = state.uptime.clone();
    // The sign only (at most 10 seconds) before the port; the day history
    // is the hourly upkeep's, which runs now and then every hour.
    uptime::start(&state).await;
    tokio::spawn({
        let state = state.clone();
        async move {
            loop {
                if let Err(error) = submissions::upkeep(&state.db).await {
                    tracing::error!(error = ?error, "submission upkeep failed");
                }
                if let Err(error) = account::verify::upkeep(&state.db).await {
                    tracing::error!(error = ?error, "automatic activation upkeep failed");
                }
                uptime::hourly(&state, time::OffsetDateTime::now_utc()).await;
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        }
    });
    let listener =
        tokio::net::TcpListener::bind(&address).await.with_context(|| format!("could not bind {address}"))?;
    tracing::info!("listening on http://{address}");
    tokio::spawn(uptime::run_ticker(state.clone()));
    let serve = axum::serve(listener, app(state.clone())).into_future();
    tokio::pin!(serve);
    tokio::select! {
        served = &mut serve => return Ok(served?),
        () = shutdown() => state.stopping.begin(),
    }
    tracing::info!("stopping: sends not started are refused; the records of tries that end are written");
    // Still serving: what is under way finishes, new sends are refused.
    tokio::select! {
        served = &mut serve => served?,
        _ = uptime.recorder.drain(DRAIN) => {}
    }
    uptime::stop(&state, time::OffsetDateTime::now_utc()).await;
    tracing::info!("stopped");
    exit_now()
}

/// Leaves at once, with status 0 and without exit-time destructors, so a
/// native library's static destructors (the OCR runtime's) cannot crash or
/// stall the stop while its worker threads still run. What was logged is
/// flushed first.
fn exit_now() -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    // SAFETY: `_exit` only ends the process; nothing in it is touched.
    unsafe {
        libc::_exit(0)
    }
    #[cfg(not(unix))]
    std::process::exit(0)
}

/// SIGUSR1, caught from the start of the process on.
#[cfg(unix)]
fn pokes() -> Option<tokio::signal::unix::Signal> {
    use tokio::signal::unix::{SignalKind, signal};
    signal(SignalKind::user_defined1())
        .inspect_err(|error| tracing::error!(%error, "SIGUSR1 cannot be caught; a note is taken in within the hour"))
        .ok()
}

#[cfg(not(unix))]
fn pokes() {}

/// The whole site: the routes and pages, behind the host rules of
/// [`canonical`].
pub fn app(state: AppState) -> axum::Router {
    let account_origin: std::sync::Arc<str> = state.config.account_origin.as_str().into();
    web::router(state.clone(), pages::router(state.clone()))
        .layer(axum::middleware::from_fn_with_state(state, canonical::split_site))
        .layer(axum::middleware::from_fn_with_state(account_origin, canonical::host_roots))
        .layer(axum::middleware::from_fn(canonical::no_index))
        .layer(axum::middleware::from_fn(canonical::bare_domain))
}

/// Development only: runs the Sender in this process on a loopback port.
/// Public deployments run `buwiz-sender` on its own (`SENDER_URL`). Built
/// without the `bir` feature, this Sender fails every send with
/// [`sender::NOT_BUILT_IN`].
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

/// SIGTERM or Ctrl-C.
async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::error!(%error, "SIGTERM cannot be caught; stopping on Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
