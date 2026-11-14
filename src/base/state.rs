//! Shared, long-lived values: settings, database pool, mailer.

use std::ops::Deref;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use sqlx::PgPool;

use crate::account::mail::Mailer;
use crate::account::verify::Verification;
use crate::base::config::Config;
use crate::base::storage::DocumentStore;
use crate::filing::link::SenderLink;
use crate::ocr::Reader;
use crate::sites::downloads::{self, Downloads};
use crate::sites::github::Stars;
use crate::uptime::Uptime;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

/// The Gatekeeper's state. It holds no BIR secret and no HPKE private key:
/// sends go to the Sender through [`SenderLink`].
pub struct Inner {
    pub config: Config,
    pub db: PgPool,
    pub mailer: Mailer,
    pub documents: DocumentStore,
    /// Signs tickets.
    pub ticket_key: SigningKey,
    pub sender: SenderLink,
    /// Reads claim documents on this server, one at a time.
    pub ocr: Reader,
    /// `AUTO_VERIFY`: whether a clear TIN ID photo activates its TIN
    /// automatically ([`crate::account::verify`]). Off unless `on`.
    pub verification: Verification,
    /// The released version of the app (eBIRForms), if any.
    pub downloads: Downloads,
    /// The released version of Buwiz Bot, if any, for buwiz.com's download
    /// button.
    pub bot_downloads: Downloads,
    /// Buwiz Bot's GitHub stars, for buwiz.com's GitHub links.
    pub stars: Stars,
    /// BIR uptime (`BIR_UPTIME`, [`crate::uptime`]): day keys, rate
    /// limiters, the record's settings.
    pub uptime: Arc<Uptime>,
    /// Set when the server is told to stop: a request that would start a
    /// send is refused from then on ([`crate::account::web`]).
    pub stopping: Stopping,
}

/// Whether the server is stopping (SIGTERM came): from then on a request
/// that would start a send is answered `503` before anything starts, while
/// those already under way finish and their records are written.
#[derive(Default)]
pub struct Stopping(std::sync::atomic::AtomicBool);

impl Stopping {
    pub fn begin(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_on(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl AppState {
    pub fn new(
        config: Config,
        db: PgPool,
        documents: DocumentStore,
        ticket_key: SigningKey,
        sender: SenderLink,
    ) -> Self {
        let mailer = Mailer::new(config.mail.clone(), &config.account_origin);
        let downloads = Downloads::new(&config.downloads_url, downloads::APP, true);
        let bot_downloads = Downloads::new(&config.downloads_url, downloads::BOT_APP, false);
        let stars = Stars::new(&config.github_api_url);
        let uptime = Arc::new(Uptime::from_env(config.account_origin.starts_with("https://")));
        Self(Arc::new(Inner {
            config,
            db,
            mailer,
            documents,
            ticket_key,
            sender,
            ocr: Reader::from_env(),
            verification: Verification::from_env(),
            downloads,
            bot_downloads,
            stars,
            uptime,
            stopping: Stopping::default(),
        }))
    }
}

impl AppState {
    /// The same state with automatic activation set to `verification`, for
    /// tests. Only before the state is shared (cloned).
    pub fn with_verification(mut self, verification: Verification) -> Self {
        Arc::get_mut(&mut self.0).expect("set before the state is shared").verification = verification;
        self
    }

    /// The same state with BIR uptime set to `uptime`, for tests. Only
    /// before the state is shared (cloned).
    pub fn with_uptime(mut self, uptime: Uptime) -> Self {
        Arc::get_mut(&mut self.0).expect("set before the state is shared").uptime = Arc::new(uptime);
        self
    }
}

impl Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}
