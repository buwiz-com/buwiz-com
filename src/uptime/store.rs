//! Writes what a try showed: one `bir_tries` row, and the upload server's
//! host key in `bir_host_keys`. Nothing written names the account, the
//! network, the TIN or the return: only the day-keyed hashes of
//! [`super::keys`], and times to the minute.

use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::Uptime;
use super::classify::Mode;
use super::keys::{Keys, Network};
use super::observe::Observed;
use crate::base::crypto;

/// How many sightings a host key keeps.
pub const HOST_KEY_SIGHTINGS: i64 = 50;

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

/// Records a finished relay try. Never fails the send: an error is logged
/// (without the try's details) and the try is lost.
pub async fn record(uptime: &Uptime, db: &PgPool, relay: &RelayTry, observed: &Observed, ended_at: OffsetDateTime) {
    let keys = uptime.keys.keys(relay.account_id, &relay.network, relay.started_at);
    if observed.counted {
        uptime.day_sets.note(relay.account_id, &relay.network, ended_at);
    }
    let insert = async {
        insert_try(db, relay, observed, &keys.current, ended_at).await?;
        if let Some(fingerprint) = &observed.host_key
            && observed.mode == Mode::Live
        {
            note_host_key(db, fingerprint, &keys.current, ended_at).await?;
        }
        Ok::<(), sqlx::Error>(())
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), insert).await {
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

/// Adds a sighting of an upload server's host key: when, and the keys of
/// the account and network that saw it (the newest 50, each once).
pub async fn note_host_key(db: &PgPool, fingerprint: &str, keys: &Keys, at: OffsetDateTime) -> sqlx::Result<()> {
    let sighting = json!([{
        "epoch": keys.epoch,
        "account_key": crypto::hex(&keys.account),
        "network_key": crypto::hex(&keys.network),
    }]);
    sqlx::query(
        "INSERT INTO bir_host_keys (fingerprint, first_seen, last_seen, seen) VALUES ($1, $2, $2, $3)
         ON CONFLICT (fingerprint) DO UPDATE SET
             last_seen = GREATEST(bir_host_keys.last_seen, EXCLUDED.last_seen),
             seen = CASE WHEN bir_host_keys.seen @> EXCLUDED.seen THEN bir_host_keys.seen ELSE (
                 SELECT jsonb_agg(sighting ORDER BY n)
                   FROM (SELECT sighting, n
                           FROM jsonb_array_elements(bir_host_keys.seen || EXCLUDED.seen) WITH ORDINALITY AS s (sighting, n)
                          ORDER BY n DESC LIMIT $4) AS newest) END",
    )
    .bind(fingerprint)
    .bind(to_minute(at))
    .bind(sighting)
    .bind(HOST_KEY_SIGHTINGS)
    .execute(db)
    .await?;
    Ok(())
}
