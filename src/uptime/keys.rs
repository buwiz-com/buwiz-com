//! The keys an uptime record holds instead of who and where: the account
//! and the network of a try, each as `HMAC-SHA256(day_key, …)` cut to 8
//! bytes. The day key is 32 random bytes made at start and at each Manila
//! midnight, held only in memory; the one before is kept for 3 hours after
//! it is replaced, so a try just after midnight can be compared with those
//! just before. After that nobody can tell which account or network a key
//! was.
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
/// `CF-Connecting-IP` is logged as an error, at most once an hour, because
/// every try would then count as one network; staff are told from
/// [`NetworkReader::missing_header_at`].
pub struct NetworkReader {
    production: bool,
    missing: Mutex<Option<OffsetDateTime>>,
}

impl NetworkReader {
    pub fn new(production: bool) -> Self {
        Self { production, missing: Mutex::new(None) }
    }

    pub fn network(&self, headers: &HeaderMap, now: OffsetDateTime) -> Network {
        match client_ip(headers) {
            Some(address) => Network::of(address),
            None if self.production => {
                let mut missing = self.missing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if missing.is_none_or(|at| now - at >= MISSING_HEADER_EVERY) {
                    *missing = Some(now);
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
        *self.missing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// One try's keys under one day key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Keys {
    /// Which day key: the Unix minute it was made (unique in a process).
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
    fn new(now: OffsetDateTime, after: Option<i32>) -> Self {
        let minute = i32::try_from(now.unix_timestamp().div_euclid(60)).unwrap_or(i32::MAX);
        let epoch = match after {
            Some(previous) if previous >= minute => previous.saturating_add(1),
            _ => minute,
        };
        let mut key = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rng(), key.as_mut());
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
    fn turn(&mut self, now: OffsetDateTime) {
        let today = manila_day(now);
        if today != self.current.day {
            let kept_until = manila_midnight(today) + PREVIOUS_KEPT;
            let next = DayKey::new(now, Some(self.current.epoch));
            let old = std::mem::replace(&mut self.current, next);
            // A key two days old is never kept: only yesterday's, and only
            // until 3 AM.
            self.previous = (old.day.next_day() == Some(today)).then_some((old, kept_until));
        }
        if self.previous.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.previous = None;
        }
    }
}

/// The day keys: the current one and, for 3 hours after Manila midnight,
/// the one before. A restart starts with a new key and no previous one.
pub struct KeyRing {
    ring: Mutex<Ring>,
}

impl KeyRing {
    pub fn new(now: OffsetDateTime) -> Self {
        Self { ring: Mutex::new(Ring { current: DayKey::new(now, None), previous: None }) }
    }

    /// The keys of a try by `account` from `network`, at `now`.
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

/// A fixed-window rate limiter held in memory: at most `limit` hits per key
/// in each `window` that starts at the key's first hit. Keys (an IP
/// address, an account id) are never stored anywhere else.
pub struct RateLimiter<K> {
    limit: u32,
    window: Duration,
    hits: Mutex<Hits<K>>,
}

struct Hits<K> {
    by_key: HashMap<K, (OffsetDateTime, u32)>,
    /// When the map grows past this, windows that ended are dropped.
    prune_at: usize,
}

const PRUNE_FLOOR: usize = 4096;

impl<K: Eq + Hash> RateLimiter<K> {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window, hits: Mutex::new(Hits { by_key: HashMap::new(), prune_at: PRUNE_FLOOR }) }
    }

    /// Counts a hit for `key`: `Ok`, or how long until it may go again.
    pub fn check(&self, key: K, now: OffsetDateTime) -> Result<(), Duration> {
        let mut hits = self.hits.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if hits.by_key.len() >= hits.prune_at {
            let window = self.window;
            hits.by_key.retain(|_, (start, _)| now - *start < window);
            hits.prune_at = (hits.by_key.len() * 2).max(PRUNE_FLOOR);
        }
        let entry = hits.by_key.entry(key).or_insert((now, 0));
        if now - entry.0 >= self.window || now < entry.0 {
            *entry = (now, 0);
        }
        if entry.1 >= self.limit {
            return Err(self.window - (now - entry.0));
        }
        entry.1 += 1;
        Ok(())
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
        // 00:10 on the 12th: a new key, and the old one alongside.
        let after = at("2026-11-11T16:10:00Z");
        let new = ring.keys(ana, &home, after);
        assert_ne!(new.current, old.current);
        assert!(new.current.epoch > old.current.epoch);
        assert_eq!(new.previous, Some(old.current));
        // Until 03:00 Manila, then gone.
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T18:59:59Z")).previous, Some(old.current));
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T19:00:00Z")).previous, None);
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T19:00:00Z")).current, new.current);
        assert_eq!(ring.epochs(at("2026-11-11T19:00:00Z")), (new.current.epoch, None));
        // A process that sleeps through a whole day keeps no two-day-old key.
        let ring = KeyRing::new(before);
        assert_eq!(ring.keys(ana, &home, at("2026-11-13T00:30:00Z")).previous, None);
        // The first use after 03:00 still turns the key, without the old.
        let ring = KeyRing::new(before);
        assert_eq!(ring.keys(ana, &home, at("2026-11-11T20:00:00Z")).previous, None);
    }

    #[test]
    fn epochs_only_grow_even_when_the_clock_goes_back() {
        let now = at("2026-11-11T16:10:00Z");
        let ring = KeyRing::new(now);
        let (first, _) = ring.epochs(now);
        // The clock steps back across midnight: another day, so another key,
        // whose epoch is still newer.
        let (second, _) = ring.epochs(at("2026-11-11T15:50:00Z"));
        assert_eq!(second, first + 1);
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
    fn the_rate_limiter_drops_ended_windows_as_it_grows() {
        let now = at("2026-11-11T07:12:00Z");
        let limiter = RateLimiter::new(1, Duration::minutes(1));
        for key in 0..PRUNE_FLOOR as u32 {
            limiter.check(key, now).unwrap();
        }
        assert_eq!(limiter.len(), PRUNE_FLOOR);
        limiter.check(u32::MAX, now + Duration::minutes(2)).unwrap();
        assert_eq!(limiter.len(), 1);
        assert!(!limiter.is_empty());
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
