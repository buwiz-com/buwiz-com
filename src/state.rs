//! Shared, long-lived values: settings, database pool, mailer.

use std::ops::Deref;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use sqlx::PgPool;

use crate::bir;
use crate::config::Config;
use crate::mail::Mailer;
use crate::storage::DocumentStore;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: PgPool,
    pub mailer: Mailer,
    pub relay: bir::Settings,
    pub receipt_key: SigningKey,
    pub documents: DocumentStore,
}

impl AppState {
    pub fn new(
        config: Config,
        db: PgPool,
        relay: bir::Settings,
        receipt_key: SigningKey,
        documents: DocumentStore,
    ) -> Self {
        let mailer = Mailer::new(config.mail.clone());
        Self(Arc::new(Inner { config, db, mailer, relay, receipt_key, documents }))
    }
}

impl Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}
