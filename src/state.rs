//! Shared, long-lived values: settings, database pool, mailer.

use std::ops::Deref;
use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::mail::Mailer;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: PgPool,
    pub mailer: Mailer,
}

impl AppState {
    pub fn new(config: Config, db: PgPool) -> Self {
        let mailer = Mailer::new(config.mail.clone());
        Self(Arc::new(Inner { config, db, mailer }))
    }
}

impl Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}
