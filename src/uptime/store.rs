//! Writes what a try showed: one `bir_tries` row, and the upload server's
//! host key in `bir_host_keys`. Nothing written names the account, the
//! network, the TIN or the return: only the day-keyed hashes of
//! [`super::keys`], and times to the minute.
//!
//! A record is written after the app has its `result`, in a task of its own
//! ([`Recorder`]), so recording never delays a send. Shutdown waits a few
//! seconds for the records still being written ([`Recorder::drain`]).
//!
//! A host-key sighting is read from bytes the user's app relays, so it can
//! be forged, and the keys of different key periods cannot be compared: it
//! is information only and never grounds to pin a key
//! (docs/relay-protocol.md §7).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::Notify;
use uuid::Uuid;

use super::Uptime;
use super::classify::Mode;
use super::keys::{Keys, Network};
use super::observe::Observed;
use crate::base::crypto;

/// How many sightings a host key keeps.
pub const HOST_KEY_SIGHTINGS: i64 = 50;

/// The most records being written at once; past it a record is lost (and
/// logged), so a database that hangs cannot pile tasks up.
pub const MAX_PENDING: usize = 256;

/// How long one record may take before it is given up.
const RECORD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A relay try, from the moment the bridge hands the job to the Sender.
/// What it holds of the account and the network stays in memory.
pub struct RelayTry {
    pub account_id: Uuid,
    pub network: Network,
    /// The `buwiz-client` header, if it reads as `<app>/<version>`.
    pub client: Option<String>,
    /// The ticket's admission; `open` for v1 and for tickets from before.
    pub admission: String,
    pub started_at: OffsetDateTime,
}

/// `at` to the minute.
pub fn to_minute(at: OffsetDateTime) -> OffsetDateTime {
    at.replace_second(0).and_then(|at| at.replace_nanosecond(0)).unwrap_or(at)
}

/// The admissions a row may hold.
const ADMISSIONS: [&str; 5] = ["open", "admit", "scout", "legacy_scout", "try_anyway"];

/// The records being written after their try ended.
#[derive(Default)]
pub struct Recorder {
    pending: AtomicUsize,
    idle: Notify,
    closed: AtomicBool,
}

impl Recorder {
    /// A place for one record, taken before the app is told, so that
    /// [`Recorder::settled`] waits for it from then on. `None` (and logged)
    /// when the server is shutting down or too many are being written.
    pub fn slot(uptime: &Arc<Uptime>) -> Option<Slot> {
        let recorder = &uptime.recorder;
        if recorder.closed.load(Ordering::SeqCst) {
            tracing::warn!("BIR uptime: the server is stopping; a try is not recorded");
            return None;
        }
        if recorder.pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
            recorder.pending.fetch_sub(1, Ordering::SeqCst);
            tracing::error!("BIR uptime: too many tries are being recorded; one is lost");
            return None;
        }
        Some(Slot { uptime: uptime.clone() })
    }

    /// How many records are being written.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Waits until no record is being written.
    pub async fn settled(&self) {
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.pending() == 0 {
                return;
            }
            idle.await;
        }
    }

    /// At shutdown: takes no new record and waits up to `limit` for those
    /// being written. `false` (and logged) if some were still being written,
    /// which end with the process.
    pub async fn drain(&self, limit: std::time::Duration) -> bool {
        self.closed.store(true, Ordering::SeqCst);
        if tokio::time::timeout(limit, self.settled()).await.is_ok() {
            return true;
        }
        tracing::error!(pending = self.pending(), "BIR uptime: stopping before every try was recorded");
        false
    }
}

/// One record's place in the [`Recorder`]: given back when the record is
/// written, or given up.
pub struct Slot {
    uptime: Arc<Uptime>,
}

impl Slot {
    /// Writes the record in a task of its own.
    pub fn record(self, db: PgPool, relay: RelayTry, observed: Observed, ended_at: OffsetDateTime) {
        tokio::spawn(async move {
            record(&self.uptime, &db, &relay, &observed, ended_at).await;
            drop(self);
        });
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let recorder = &self.uptime.recorder;
        if recorder.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            recorder.idle.notify_waiters();
        }
    }
}

/// The host-key sightings this process has written: a sighting that adds
/// nothing (the same key, key period, account, network and outcome) is not
/// written again, so a pinned key's row is not rewritten by every upload.
#[derive(Default)]
pub struct Sightings {
    written: Mutex<HashSet<(String, Keys, bool)>>,
}

/// The most sightings [`Sightings`] holds before it starts again.
const MAX_SIGHTINGS: usize = 10_000;

impl Sightings {
    fn known(&self, sighting: &(String, Keys, bool)) -> bool {
        self.written.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(sighting)
    }

    fn add(&self, sighting: (String, Keys, bool)) {
        let mut written = self.written.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if written.len() >= MAX_SIGHTINGS {
            written.clear();
        }
        written.insert(sighting);
    }
}

/// Records a finished relay try, under the keys of `ended_at`, when it is
/// recorded (keys never turn back, so a try that started before midnight
/// and ends after it gets the new day's). Never fails the send: an error is
/// logged (without the try's details) and the try is lost.
pub async fn record(uptime: &Uptime, db: &PgPool, relay: &RelayTry, observed: &Observed, ended_at: OffsetDateTime) {
    let keys = uptime.keys.keys(relay.account_id, &relay.network, ended_at);
    if observed.counted {
        uptime.day_sets.note(relay.account_id, &relay.network, ended_at);
    }
    let insert = async {
        insert_try(db, relay, observed, &keys.current, ended_at).await?;
        if let Some(fingerprint) = &observed.host_key
            && observed.mode == Mode::Live
        {
            let sighting = (fingerprint.clone(), keys.current, observed.key_exchange_done);
            if !uptime.sightings.known(&sighting) {
                note_host_key(db, fingerprint, &keys.current, observed.key_exchange_done, ended_at).await?;
                uptime.sightings.add(sighting);
            }
        }
        Ok::<(), sqlx::Error>(())
    };
    match tokio::time::timeout(RECORD_TIMEOUT, insert).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, "BIR uptime: could not record a try"),
        Err(_) => tracing::error!("BIR uptime: recording a try took too long; it is lost"),
    }
}

async fn insert_try(
    db: &PgPool,
    relay: &RelayTry,
    observed: &Observed,
    keys: &Keys,
    ended_at: OffsetDateTime,
) -> sqlx::Result<()> {
    let classified = &observed.classified;
    let admission = if ADMISSIONS.contains(&relay.admission.as_str()) { relay.admission.as_str() } else { "open" };
    let started_at = to_minute(relay.started_at);
    sqlx::query(
        "INSERT INTO bir_tries (started_at, ended_at, source, mode, outcome, stage, dispatcher, upload,
             failure_class, failure_kind, touched_bir, opens, admission, counted, key_epoch, account_key,
             network_key, client)
         VALUES ($1, $2, 'relay', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(started_at)
    .bind(to_minute(ended_at).max(started_at))
    .bind(observed.mode.as_str())
    .bind(classified.outcome.as_str())
    .bind(classified.stage.as_str())
    .bind(classified.dispatcher.as_str())
    .bind(classified.upload.as_str())
    .bind(observed.class().map(|class| class.as_str()))
    .bind(observed.kind.map(|kind| kind.as_str()))
    .bind(observed.touched_bir)
    .bind(i16::try_from(observed.opens).unwrap_or(i16::MAX))
    .bind(admission)
    .bind(observed.counted)
    .bind(keys.epoch)
    .bind(keys.account.as_slice())
    .bind(keys.network.as_slice())
    .bind(&relay.client)
    .execute(db)
    .await?;
    Ok(())
}

/// Adds a sighting of an upload server's host key: the keys of the account
/// and network that saw it, their key period, and whether the Sender went
/// on to complete the key exchange (the newest 50, each once). The row is
/// written only when the sighting adds something: a new key, or a new
/// account, network or outcome in this key period.
pub async fn note_host_key(
    db: &PgPool,
    fingerprint: &str,
    keys: &Keys,
    kex_done: bool,
    at: OffsetDateTime,
) -> sqlx::Result<()> {
    let sighting = json!([{
        "epoch": keys.epoch,
        "account_key": crypto::hex(&keys.account),
        "network_key": crypto::hex(&keys.network),
        "kex_done": kex_done,
    }]);
    sqlx::query(
        "INSERT INTO bir_host_keys (fingerprint, first_seen, last_seen, seen) VALUES ($1, $2, $2, $3)
         ON CONFLICT (fingerprint) DO UPDATE SET
             last_seen = GREATEST(bir_host_keys.last_seen, EXCLUDED.last_seen),
             seen = (SELECT jsonb_agg(sighting ORDER BY n)
                       FROM (SELECT sighting, n
                               FROM jsonb_array_elements(bir_host_keys.seen || EXCLUDED.seen) WITH ORDINALITY AS s (sighting, n)
                              ORDER BY n DESC LIMIT $4) AS newest)
         WHERE NOT bir_host_keys.seen @> EXCLUDED.seen",
    )
    .bind(fingerprint)
    .bind(to_minute(at))
    .bind(sighting)
    .bind(HOST_KEY_SIGHTINGS)
    .execute(db)
    .await?;
    Ok(())
}

/// What a host key's sightings show within one key period: distinct
/// accounts and networks, and how many of those accounts saw the Sender
/// complete the key exchange. Keys of different periods cannot be compared,
/// so nothing is counted across them. Information only: never grounds to
/// pin a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    pub epoch: i64,
    pub accounts: usize,
    pub networks: usize,
    pub kex_done_accounts: usize,
}

/// A `bir_host_keys.seen` array by key period, in the order each period
/// was first seen there.
pub fn periods(seen: &Value) -> Vec<Period> {
    struct Sets<'a> {
        epoch: i64,
        accounts: HashSet<&'a str>,
        networks: HashSet<&'a str>,
        done: HashSet<&'a str>,
    }
    let mut sets: Vec<Sets> = Vec::new();
    for sighting in seen.as_array().into_iter().flatten() {
        let (Some(epoch), Some(account), Some(network)) =
            (sighting["epoch"].as_i64(), sighting["account_key"].as_str(), sighting["network_key"].as_str())
        else {
            continue;
        };
        let at = match sets.iter().position(|period| period.epoch == epoch) {
            Some(at) => at,
            None => {
                sets.push(Sets { epoch, accounts: HashSet::new(), networks: HashSet::new(), done: HashSet::new() });
                sets.len() - 1
            }
        };
        let period = &mut sets[at];
        period.accounts.insert(account);
        period.networks.insert(network);
        if sighting["kex_done"].as_bool() == Some(true) {
            period.done.insert(account);
        }
    }
    sets.into_iter()
        .map(|period| Period {
            epoch: period.epoch,
            accounts: period.accounts.len(),
            networks: period.networks.len(),
            kex_done_accounts: period.done.len(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sightings_count_only_within_one_key_period() {
        let sighting = |epoch: i64, account: &str, network: &str, done: bool| json!({ "epoch": epoch, "account_key": account, "network_key": network, "kex_done": done });
        // Three accounts on three networks, but across three key periods:
        // no period has more than one of each.
        let seen = json!([
            sighting(7, "a1", "n1", false),
            sighting(9, "a2", "n2", false),
            sighting(4, "a3", "n3", false),
            sighting(9, "a2", "n2", false),
            sighting(9, "a4", "n2", true),
            json!({ "epoch": "odd" }),
        ]);
        assert_eq!(
            periods(&seen),
            [
                Period { epoch: 7, accounts: 1, networks: 1, kex_done_accounts: 0 },
                Period { epoch: 9, accounts: 2, networks: 1, kex_done_accounts: 1 },
                Period { epoch: 4, accounts: 1, networks: 1, kex_done_accounts: 0 },
            ]
        );
        assert!(periods(&json!(null)).is_empty());
    }
}
