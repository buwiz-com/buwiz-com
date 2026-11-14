//! The keys an uptime record holds instead of who and where: the account
//! and the network of a try, each as `HMAC-SHA256(day_key, …)` cut to 8
//! bytes. The day key is 32 random bytes made at start and at the first
//! use after each Manila midnight, held only in memory, with a random id
//! (its key period). Keys are taken when a try is recorded and only ever
//! turn forward: a try that started yesterday and ends today gets today's
//! keys. The key before is kept for 3 hours after it is replaced, so a try
//! just after midnight can be compared with those just before. After that
//! nobody can tell which account or network a key was.
//!
//! The network is the caller's address cut to its IPv4 /24 or IPv6 /48,
//! read from `CF-Connecting-IP` (production sits behind a Cloudflare
//! Tunnel, so that header is the only place the address shows). Neither the
//! address nor the network is ever written anywhere. Also here: the
//! in-memory rate limiters (per IP, per account) and the day's distinct
//! accounts and networks.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::Mutex;

use http::HeaderMap;
use sha2::{Digest, Sha256};
use time::{Date, Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Manila time, UTC+8 all year.
pub const MANILA: UtcOffset = match UtcOffset::from_hms(8, 0, 0) {
    Ok(offset) => offset,
    Err(_) => panic!("UTC+8 is an offset"),
};

/// How long the previous day key stays after it is replaced.
pub const PREVIOUS_KEPT: Duration = Duration::hours(3);

/// How often a missing `CF-Connecting-IP` in production is raised.
pub const MISSING_HEADER_EVERY: Duration = Duration::hours(1);

/// The header that carries the caller's address behind the tunnel.
pub const CLIENT_IP_HEADER: &str = "cf-connecting-ip";

/// The Manila day `at` falls on.
pub fn manila_day(at: OffsetDateTime) -> Date {
    at.to_offset(MANILA).date()
}

/// When the Manila day `day` begins.
pub fn manila_midnight(day: Date) -> OffsetDateTime {
    day.midnight().assume_offset(MANILA)
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = Zeroizing::new([0u8; BLOCK]);
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| -> Zeroizing<[u8; BLOCK]> {
        let mut padded = Zeroizing::new([0u8; BLOCK]);
        for (out, key) in padded.iter_mut().zip(block.iter()) {
            *out = key ^ byte;
        }
        padded
    };
    let inner = Sha256::new().chain_update(pad(0x36).as_slice()).chain_update(message).finalize();
    Sha256::new().chain_update(pad(0x5c).as_slice()).chain_update(inner).finalize().into()
}

/// Where a try came from, as far as the record may know: the caller's
/// network, never its address. Held in memory only.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    /// The first three bytes of an IPv4 address (/24).
    V4([u8; 3]),
    /// The first six bytes of an IPv6 address (/48).
    V6([u8; 6]),
    /// Development: no `CF-Connecting-IP`.
    Local,
    /// Production without `CF-Connecting-IP`: every such try is one
    /// network, which only makes the sign harder to move.
    Missing,
}

impl Network {
    pub fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => {
                let [a, b, c, _] = v4.octets();
                Self::V4([a, b, c])
            }
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Self::of(IpAddr::V4(v4)),
                None => {
                    let octets = v6.octets();
                    let mut prefix = [0u8; 6];
                    prefix.copy_from_slice(&octets[..6]);
                    Self::V6(prefix)
                }
            },
        }
    }

    /// What the network key is computed over.
    fn message(&self) -> Vec<u8> {
        let mut message = b"buwiz-bir-network\0".to_vec();
        match self {
            Self::V4(prefix) => {
                message.push(4);
                message.extend_from_slice(prefix);
            }
            Self::V6(prefix) => {
                message.push(6);
                message.extend_from_slice(prefix);
            }
            Self::Local => message.extend_from_slice(b"local"),
            Self::Missing => message.extend_from_slice(b"missing"),
        }
        message
    }
}

/// The caller's address from `CF-Connecting-IP`, if it reads as one.
pub fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers.get(CLIENT_IP_HEADER)?.to_str().ok()?.trim().parse().ok()
}

/// Reads the network of a request. In production a request without
/// `CF-Connecting-IP` is raised at most once an hour, because every try
/// would then count as one network: it is logged as an error and kept as an
/// alert ([`NetworkReader::take_missing_header_alert`]) for the staff mail,
/// which a later release sends. Nobody is mailed yet.
pub struct NetworkReader {
    production: bool,
    missing: Mutex<Missing>,
}

#[derive(Default)]
struct Missing {
    /// When it was last raised.
    raised: Option<OffsetDateTime>,
    /// Raised and not yet taken for the staff mail.
    alert: Option<OffsetDateTime>,
}

impl NetworkReader {
    pub fn new(production: bool) -> Self {
        Self { production, missing: Mutex::new(Missing::default()) }
    }

    pub fn network(&self, headers: &HeaderMap, now: OffsetDateTime) -> Network {
        match client_ip(headers) {
            Some(address) => Network::of(address),
            None if self.production => {
                let mut missing = self.missing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if missing.raised.is_none_or(|at| now - at >= MISSING_HEADER_EVERY || now < at) {
                    missing.raised = Some(now);
                    missing.alert = Some(now);
                    tracing::error!(
                        "BIR uptime: a request reached the server without CF-Connecting-IP; every such try counts as one network"
                    );
                }
                Network::Missing
            }
            None => Network::Local,
        }
    }

    /// When a missing `CF-Connecting-IP` was last raised, if ever.
    pub fn missing_header_at(&self) -> Option<OffsetDateTime> {
        self.missing.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).raised
    }

    /// The alert for the staff mail: when a missing `CF-Connecting-IP` was
    /// raised, once per raise (at most once an hour). The mail is sent by a
    /// later release; until then the alert is only kept here.
    pub fn take_missing_header_alert(&self) -> Option<OffsetDateTime> {
        self.missing.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).alert.take()
    }
}

/// One try's keys under one day key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keys {
    /// Which day key (its key period): a random positive id, new with each
    /// key and never that of the key before.
    pub epoch: i32,
    pub account: [u8; 8],
    pub network: [u8; 8],
}

/// A try's keys under the current day key, and under the previous one while
/// it is kept, so the rules can compare it with tries from before midnight.
/// The record stores `current`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryKeys {
    pub current: Keys,
    pub previous: Option<Keys>,
}

struct DayKey {
    epoch: i32,
    day: Date,
    key: Zeroizing<[u8; 32]>,
}

impl DayKey {
    fn new(now: OffsetDateTime, before: Option<i32>) -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        let epoch = loop {
            let epoch = rng.random_range(1..=i32::MAX);
            if Some(epoch) != before {
                break epoch;
            }
        };
        let mut key = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rng, key.as_mut());
        Self { epoch, day: manila_day(now), key }
    }

    fn keys(&self, account: Uuid, network: &Network) -> Keys {
        let mut account_message = b"buwiz-bir-account\0".to_vec();
        account_message.extend_from_slice(account.as_bytes());
        let cut = |digest: [u8; 32]| -> [u8; 8] {
            let mut out = [0u8; 8];
            out.copy_from_slice(&digest[..8]);
            out
        };
        Keys {
            epoch: self.epoch,
            account: cut(hmac_sha256(self.key.as_slice(), &account_message)),
            network: cut(hmac_sha256(self.key.as_slice(), &network.message())),
        }
    }
}

struct Ring {
    current: DayKey,
    /// The key before, until when it is kept.
    previous: Option<(DayKey, OffsetDateTime)>,
}

impl Ring {
    /// Only ever forward: a clock that steps back keeps the key it has.
    fn turn(&mut self, now: OffsetDateTime) {
        let today = manila_day(now);
        if today > self.current.day {
            let next = DayKey::new(now, Some(self.current.epoch));
            let old = std::mem::replace(&mut self.current, next);
            // A key two days old is never kept: only yesterday's, and only
            // for 3 hours from now, when it was replaced.
            self.previous = (old.day.next_day() == Some(today)).then_some((old, now + PREVIOUS_KEPT));
        }
        if self.previous.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.previous = None;
        }
    }
}

/// The day keys: the current one and, for 3 hours after it replaced it,
/// the one before. A restart starts with a new key and no previous one.
pub struct KeyRing {
    ring: Mutex<Ring>,
}

impl KeyRing {
    pub fn new(now: OffsetDateTime) -> Self {
        Self { ring: Mutex::new(Ring { current: DayKey::new(now, None), previous: None }) }
    }

    /// The keys of a try by `account` from `network`, recorded at `now`
    /// (when it ended, never when it started).
    pub fn keys(&self, account: Uuid, network: &Network, now: OffsetDateTime) -> TryKeys {
        let mut ring = self.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ring.turn(now);
        TryKeys {
            current: ring.current.keys(account, network),
            previous: ring.previous.as_ref().map(|(key, _)| key.keys(account, network)),
        }
    }

    /// The epochs of the current key and of the previous one, if kept.
    pub fn epochs(&self, now: OffsetDateTime) -> (i32, Option<i32>) {
        let mut ring = self.ring.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ring.turn(now);
        (ring.current.epoch, ring.previous.as_ref().map(|(key, _)| key.epoch))
    }
}

/// What the per-IP limiter counts an address as: an IPv4 address itself,
/// an IPv6 address by its /64 (one household or one server gets a whole
/// /64, so counting its addresses one by one limits nothing).
pub fn ip_key(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(octets.into())
            }
        },
    }
}

/// The most keys a limiter holds in one window.
pub const MAX_KEYS: usize = 100_000;

/// A fixed-window rate limiter held in memory: at most `limit` hits per key
/// in each `window`. Windows are shared: when one ends, every count starts
/// again, so nothing is ever scanned. A key new in a window that already
/// holds `cap` keys is refused until the window ends, so the map never
/// grows past `cap`. Keys (an IP address, an account id) are never stored
/// anywhere else.
pub struct RateLimiter<K> {
    limit: u32,
    window: Duration,
    cap: usize,
    hits: Mutex<Hits<K>>,
}

struct Hits<K> {
    /// When the current window began; `None` before the first hit.
    since: Option<OffsetDateTime>,
    by_key: HashMap<K, u32>,
}

impl<K: Eq + Hash> RateLimiter<K> {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self::with_cap(limit, window, MAX_KEYS)
    }

    pub fn with_cap(limit: u32, window: Duration, cap: usize) -> Self {
        Self { limit, window, cap, hits: Mutex::new(Hits { since: None, by_key: HashMap::new() }) }
    }

    /// Counts a hit for `key`: `Ok`, or how long until it may go again.
    pub fn check(&self, key: K, now: OffsetDateTime) -> Result<(), Duration> {
        let (verdict, ended) = {
            let mut hits = self.hits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            // A window that ended (or a clock that went back) starts a new
            // one; the old counts are dropped after the lock is let go.
            let ended = match hits.since {
                Some(since) if now >= since && now - since < self.window => None,
                _ => {
                    hits.since = Some(now);
                    Some(std::mem::take(&mut hits.by_key))
                }
            };
            let since = hits.since.unwrap_or(now);
            let wait = self.window - (now - since);
            let full = hits.by_key.len() >= self.cap;
            let verdict = match hits.by_key.get_mut(&key) {
                Some(count) if *count >= self.limit => Err(wait),
                Some(count) => {
                    *count += 1;
                    Ok(())
                }
                None if full => Err(wait),
                None => {
                    hits.by_key.insert(key, 1);
                    Ok(())
                }
            };
            (verdict, ended)
        };
        drop(ended);
        verdict
    }

    /// How many keys it holds now.
    pub fn len(&self) -> usize {
        self.hits.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The distinct accounts and networks of the Manila day's counted tries,
/// for the day history. Raw ids and networks, in memory only, cleared when
/// the day changes.
#[derive(Default)]
pub struct DaySets {
    sets: Mutex<Sets>,
}

#[derive(Default)]
struct Sets {
    day: Option<Date>,
    accounts: HashSet<Uuid>,
    networks: HashSet<Network>,
}

impl DaySets {
    pub fn note(&self, account: Uuid, network: &Network, at: OffsetDateTime) {
        let day = manila_day(at);
        let mut sets = self.sets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if sets.day != Some(day) {
            if sets.day.is_some_and(|known| known > day) {
                // A try that ended before midnight, noted after: its day's
                // counts are no longer kept here.
                return;
            }
            *sets = Sets { day: Some(day), ..Sets::default() };
        }
        sets.accounts.insert(account);
        sets.networks.insert(network.clone());
    }

    /// (accounts, networks) on `day`, if this process holds that day.
    pub fn counts(&self, day: Date) -> Option<(usize, usize)> {
        let sets = self.sets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (sets.day == Some(day)).then(|| (sets.accounts.len(), sets.networks.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    fn at(raw: &str) -> OffsetDateTime {
        OffsetDateTime::parse(raw, &Rfc3339).unwrap()
    }

    fn headers(address: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_IP_HEADER, address.parse().unwrap());
        headers
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        let hex = crate::base::crypto::hex;
        // Test case 2.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // Test case 6: a key longer than the block is hashed first.
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(hex(&mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn a_network_is_the_v4_slash_24_or_the_v6_slash_48() {
        let of = |raw: &str| Network::of(raw.parse().unwrap());
        assert_eq!(of("203.0.113.7"), Network::V4([203, 0, 113]));
        assert_eq!(of("203.0.113.250"), of("203.0.113.7"));
        assert_ne!(of("203.0.114.7"), of("203.0.113.7"));
        assert_eq!(of("2001:db8:1234:5678::1"), Network::V6([0x20, 0x01, 0x0d, 0xb8, 0x12, 0x34]));
        assert_eq!(of("2001:db8:1234:ffff:ffff::9"), of("2001:db8:1234:5678::1"));
        assert_ne!(of("2001:db8:1235::1"), of("2001:db8:1234::1"));
        // An IPv4 address written as IPv6 is the same IPv4 network.
        assert_eq!(of("::ffff:203.0.113.9"), of("203.0.113.7"));
    }

    #[test]
    fn the_header_gives_the_network_and_its_absence_is_raised_in_production() {
        let now = at("2026-11-11T07:12:00Z");
        let development = NetworkReader::new(false);
        assert_eq!(development.network(&headers(" 198.51.100.4 "), now), Network::V4([198, 51, 100]));
        assert_eq!(development.network(&HeaderMap::new(), now), Network::Local);
        assert_eq!(development.network(&headers("not an address"), now), Network::Local);
        assert_eq!(development.missing_header_at(), None);

        let production = NetworkReader::new(true);
        assert_eq!(production.network(&headers("198.51.100.4"), now), Network::V4([198, 51, 100]));
        assert_eq!(production.missing_header_at(), None);
        assert_eq!(production.network(&HeaderMap::new(), now), Network::Missing);
        assert_eq!(production.missing_header_at(), Some(now));
        // Once an hour.
        production.network(&HeaderMap::new(), now + Duration::minutes(59));
        assert_eq!(production.missing_header_at(), Some(now));
        production.network(&HeaderMap::new(), now + Duration::minutes(60));
        assert_eq!(production.missing_header_at(), Some(now + Duration::minutes(60)));
    }

    #[test]
    fn keys_hide_the_account_and_network_and_differ_by_day_key() {
        let now = at("2026-11-11T07:12:00Z");
        let ana = Uuid::from_u128(1);
        let bob = Uuid::from_u128(2);
        let home = Network::V4([198, 51, 100]);
        let office = Network::V4([203, 0, 113]);
        let ring = KeyRing::new(now);
        let first = ring.keys(ana, &home, now).current;
        assert_eq!(ring.keys(ana, &home, now + Duration::hours(5)).current, first);
        assert_eq!(ring.keys(ana, &office, now).current.account, first.account);
        assert_ne!(ring.keys(ana, &office, now).current.network, first.network);
        assert_eq!(ring.keys(bob, &home, now).current.network, first.network);
        assert_ne!(ring.keys(bob, &home, now).current.account, first.account);
        // Nothing of the id or the address is in a key.
        assert!(!first.account.windows(4).any(|w| ana.as_bytes().windows(4).any(|id| id == w)));
        assert!(!first.network.windows(3).any(|w| w == [198, 51, 100]));
        // Another process (a restart) has another key.
        let restarted = KeyRing::new(now);
        assert_ne!(restarted.keys(ana, &home, now).current.account, first.account);
        assert_eq!(restarted.keys(ana, &home, now).previous, None);
    }

    #[test]
    fn the_key_turns_at_manila_midnight_and_the_old_one_stays_three_hours() {
        // 23:50 in Manila on the 11th.
        let before = at("2026-11-11T15:50:00Z");
        let ana = Uuid::from_u128(1);
        let home = Network::V6([0x20, 0x01, 0x0d, 0xb8, 0, 1]);
        let ring = KeyRing::new(before);
        let old = ring.keys(ana, &home, before);
        assert_eq!(old.previous, None);
        // 00:10 on the 12th: a new key with another id, and the old one
        // alongside.
        let after = at("2026-11-11T16:10:00Z");
        let new = ring.keys(ana, &home, after);
        assert_ne!(new.current, old.current);
        assert_ne!(new.current.epoch, old.current.epoch);
        assert!(new.current.epoch > 0);
        assert_eq!(new.previous, Some(old.current));
        // For 3 hours from 00:10, when it was replaced, then gone.
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T19:09:59Z")).previous, Some(old.current));
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T19:10:00Z")).previous, None);
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T19:10:00Z")).current, new.current);
        assert_eq!(ring.epochs(at("2026-11-11T19:10:00Z")), (new.current.epoch, None));
        // A process that sleeps through a whole day keeps no two-day-old key.
        let ring = KeyRing::new(before);
        assert_eq!(ring.keys(ana, &home, at("2026-11-13T00:30:00Z")).previous, None);
        // A first use late in the night still keeps the old one 3 hours.
        let ring = KeyRing::new(before);
        let late = ring.keys(ana, &home, at("2026-11-11T20:00:00Z"));
        assert!(late.previous.is_some());
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T23:00:00Z")).previous, None);
    }

    #[test]
    fn keys_never_turn_backwards() {
        let ana = Uuid::from_u128(1);
        let home = Network::V4([198, 51, 100]);
        // Recorded at 00:00:15 Manila (a try that started at 00:00:05),
        // then at 00:00:30 a try that started at 23:59:40: both get the
        // keys of the time they are recorded, today's.
        let ring = KeyRing::new(at("2026-11-11T15:59:00Z"));
        let yesterday = ring.keys(ana, &home, at("2026-11-11T15:59:30Z")).current;
        let first = ring.keys(ana, &home, at("2026-11-11T16:00:15Z")).current;
        let second = ring.keys(ana, &home, at("2026-11-11T16:00:30Z")).current;
        assert_ne!(first, yesterday);
        assert_eq!(second, first);
        // A clock that steps back across midnight keeps today's key.
        let back = ring.keys(ana, &home, at("2026-11-11T15:59:50Z"));
        assert_eq!(back.current, first);
        assert_eq!(back.previous, Some(yesterday));
    }

    #[test]
    fn every_key_period_has_its_own_random_id() {
        let now = at("2026-11-11T07:12:00Z");
        // Restarts in the same minute (a crash loop) give each key its own.
        let ids: HashSet<i32> = (0..50).map(|_| KeyRing::new(now).epochs(now).0).collect();
        assert_eq!(ids.len(), 50);
        assert!(ids.iter().all(|id| *id > 0));
    }

    #[test]
    fn a_missing_header_is_an_alert_taken_once_per_raise() {
        let now = at("2026-11-11T07:12:00Z");
        let production = NetworkReader::new(true);
        assert_eq!(production.take_missing_header_alert(), None);
        production.network(&HeaderMap::new(), now);
        production.network(&HeaderMap::new(), now + Duration::minutes(5));
        assert_eq!(production.take_missing_header_alert(), Some(now));
        assert_eq!(production.take_missing_header_alert(), None);
        production.network(&HeaderMap::new(), now + Duration::minutes(61));
        assert_eq!(production.take_missing_header_alert(), Some(now + Duration::minutes(61)));
        // Development never raises it.
        let development = NetworkReader::new(false);
        development.network(&HeaderMap::new(), now);
        assert_eq!(development.take_missing_header_alert(), None);
    }

    #[test]
    fn the_rate_limiter_counts_per_key_per_window() {
        let now = at("2026-11-11T07:12:00Z");
        let limiter = RateLimiter::new(3, Duration::minutes(1));
        for _ in 0..3 {
            assert_eq!(limiter.check("a", now), Ok(()));
        }
        assert_eq!(limiter.check("a", now + Duration::seconds(20)), Err(Duration::seconds(40)));
        assert_eq!(limiter.check("b", now + Duration::seconds(20)), Ok(()));
        assert_eq!(limiter.check("a", now + Duration::minutes(1)), Ok(()));
        // A clock that went back starts a new window rather than locking out.
        assert_eq!(limiter.check("a", now - Duration::minutes(5)), Ok(()));
    }

    #[test]
    fn the_rate_limiter_holds_at_most_its_cap_and_forgets_whole_windows() {
        let now = at("2026-11-11T07:12:00Z");
        let limiter = RateLimiter::with_cap(5, Duration::minutes(1), 100);
        for key in 0..100u32 {
            limiter.check(key, now).unwrap();
        }
        // Full: a known key goes on, a new one waits for the next window.
        assert_eq!(limiter.check(7, now + Duration::seconds(10)), Ok(()));
        assert_eq!(limiter.check(100, now + Duration::seconds(10)), Err(Duration::seconds(50)));
        assert_eq!(limiter.len(), 100);
        // The next window starts empty.
        limiter.check(100, now + Duration::minutes(1)).unwrap();
        assert_eq!(limiter.len(), 1);
        assert!(!limiter.is_empty());
    }

    #[test]
    fn an_ipv6_address_counts_by_its_slash_64() {
        let key = |raw: &str| ip_key(raw.parse().unwrap());
        assert_eq!(key("2001:db8:1:2:aaaa::1"), key("2001:db8:1:2:ffff:ffff:ffff:ffff"));
        assert_eq!(key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
        assert_eq!(key("203.0.113.7"), "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_ne!(key("203.0.113.7"), key("203.0.113.8"));
        assert_eq!(key("::ffff:203.0.113.7"), key("203.0.113.7"));
        let limiter = RateLimiter::new(1, Duration::minutes(1));
        let now = at("2026-11-11T07:12:00Z");
        assert_eq!(limiter.check(key("2001:db8:1:2::1"), now), Ok(()));
        assert!(limiter.check(key("2001:db8:1:2::2"), now).is_err());
    }

    #[test]
    fn day_sets_count_distinct_accounts_and_networks_per_manila_day() {
        let sets = DaySets::default();
        let morning = at("2026-11-11T01:00:00Z");
        let day = manila_day(morning);
        for (account, network) in [(1, [198, 51, 100]), (1, [198, 51, 100]), (2, [198, 51, 100]), (2, [203, 0, 113])] {
            sets.note(Uuid::from_u128(account), &Network::V4(network), morning);
        }
        assert_eq!(sets.counts(day), Some((2, 2)));
        // 00:30 Manila the next day: a new day, and yesterday is gone.
        let next = at("2026-11-11T16:30:00Z");
        sets.note(Uuid::from_u128(3), &Network::Local, next);
        assert_eq!(sets.counts(manila_day(next)), Some((1, 1)));
        assert_eq!(sets.counts(day), None);
        // A late note for yesterday changes nothing.
        sets.note(Uuid::from_u128(4), &Network::Local, morning);
        assert_eq!(sets.counts(manila_day(next)), Some((1, 1)));
    }
}
