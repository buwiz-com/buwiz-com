//! Shared, long-lived values: settings, database pool, mailer.

use std::ops::Deref;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use sqlx::PgPool;

use crate::config::Config;
use crate::downloads::Downloads;
use crate::link::SenderLink;
use crate::mail::Mailer;
use crate::ocr::Reader;
use crate::storage::DocumentStore;

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
    /// The released version of the app, if any.
    pub downloads: Downloads,
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
        let downloads = Downloads::new(&config.downloads_url);
        Self(Arc::new(Inner { config, db, mailer, documents, ticket_key, sender, ocr: Reader::from_env(), downloads }))
    }
}

impl Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}
