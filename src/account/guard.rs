//! Keeps scripts from mailing sign-in codes to strangers.
//!
//! Anyone can type any address into the sign-in form, and that address is
//! mailed a code. A script that types a new address each time gets past the
//! limit per address ([`crate::account::accounts::issue_code`]), so two
//! more checks come before a code is made: Cloudflare Turnstile on the form
//! (when `TURNSTILE_SITE_KEY` and `TURNSTILE_SECRET_KEY` are set) and a
//! limit per network. The limit on codes for addresses without an account
//! is the last one, in `issue_code`.
//!
//! The apps' requests ([`crate::account::app_sign_in`]) have a budget of
//! their own per network ([`SignInGuard::count_app_attempt`]): every try
//! counts, whatever comes of it, and none is given back. They never touch
//! the web form's count. Tries at a code, the app's and the web form's
//! together, have another ([`SignInGuard::count_verify_try`]), counted the
//! same way. Both of these let a new network through uncounted when their
//! map is full ([`RateLimiter::fail_open_when_full`]), so a flood of
//! networks locks no other out; behind them the per-address limits, the
//! testing cap and the database slots still hold.

use std::net::IpAddr;
use std::sync::Mutex;

use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};

use crate::base::config::TurnstileConfig;
use crate::uptime::keys::{Bucket, Network, RateLimiter, client_ip};

/// Codes one network (an IPv4 /24, an IPv6 /48) may ask for in an hour. An
/// office or a phone carrier shares one, so it is well above what one
/// person needs.
const CODES_PER_NETWORK: u32 = 10;
const NETWORK_WINDOW: Duration = Duration::hours(1);

/// Tries at `POST /auth/email/start` and `/resend` (JSON, the apps') one
/// network may make in an hour, whether they succeed or not. Counted before
/// anything else is done for them, and never given back, `503 busy`
/// included: giving those back would let a flood go on trying for nothing
/// just when the database is saturated. Room for the people behind one
/// carrier's or office's network.
pub const APP_ATTEMPTS_PER_NETWORK: u32 = 40;

/// Tries at a code, `POST /auth/email/verify`, the apps' JSON and the web
/// form's together, one network may make in an hour, whatever comes of
/// them. Counted before the body is read, and never given back.
pub const VERIFY_TRIES_PER_NETWORK: u32 = 60;

/// How often, at most, staff are mailed that Turnstile's answers lack the
/// hostname or action ([`SignInGuard::take_unbound_alert`]).
const UNBOUND_ALERT_EVERY: Duration = Duration::hours(1);

/// How long Turnstile may take to answer before the sign-in is refused.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Turnstile's tokens are at most this long; a longer one is not sent on.
const MAX_TOKEN_LEN: usize = 2048;

/// The script that draws Turnstile's widget.
pub const WIDGET_SCRIPT: &str = "https://challenges.cloudflare.com/turnstile/v0/api.js";

pub struct SignInGuard {
    turnstile: Option<TurnstileConfig>,
    /// The account site's host: a token is taken only from a widget there.
    hostname: String,
    http: reqwest::Client,
    /// The web form's count: codes per network.
    networks: RateLimiter<Network>,
    /// The apps' count: tries per network. Its own map, so filling it never
    /// touches the web's.
    app_attempts: RateLimiter<Network>,
    /// Tries at a code per network, the apps' and the web form's.
    verify_tries: RateLimiter<Network>,
    /// Turnstile's answers without a hostname or action, for the staff alert.
    unbound: Mutex<Unbound>,
}

#[derive(Default)]
struct Unbound {
    /// When it was last raised.
    raised: Option<OffsetDateTime>,
    /// Raised and not yet taken for the staff mail.
    alert: bool,
}

/// Which widget a token must come from, and how a Turnstile answer without
/// the hostname or action is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Widget {
    /// The sign-in forms' (`sign-in`). An answer without the fields passes,
    /// as every answer did before they were checked, and staff are told.
    Web,
    /// The app-check page's (`app-check`). An answer without them fails.
    App,
}

impl Widget {
    fn action(self) -> &'static str {
        match self {
            Self::Web => WEB_ACTION,
            Self::App => APP_ACTION,
        }
    }
}

/// The host of the account site's origin (`ACCOUNT_ORIGIN`), as Turnstile
/// reports it: `account.buwiz.com`, `localhost`, `[::1]`. Empty when the
/// origin does not parse.
pub fn account_host(origin: &str) -> String {
    origin.parse::<http::Uri>().ok().and_then(|uri| uri.host().map(str::to_owned)).unwrap_or_default()
}

/// Why a request for a code was refused before one was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Turnstile did not vouch for the form post.
    NotHuman,
    /// The caller's network asked for too many codes this hour.
    NetworkTooMany,
}

/// A post [`SignInGuard::check`] let through, as it was counted: what
/// [`SignInGuard::forgive`] takes back.
#[derive(Debug)]
pub struct Counted {
    /// `None` when the post had no address to count; else its network and
    /// the bucket its hit went to.
    network: Option<(Network, Bucket)>,
    at: OffsetDateTime,
}

/// The action the sign-in forms' widget sets (`data-action`).
pub const WEB_ACTION: &str = "sign-in";
/// The action of the app-check page's widget.
pub const APP_ACTION: &str = "app-check";

/// Turnstile's answer; its other fields are not used. A token passes only
/// with the action of the widget it is for and the account site's host,
/// so one widget's token never passes as another's.
#[derive(Deserialize)]
struct Verdict {
    success: bool,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
}

impl SignInGuard {
    /// `hostname` is the account site's host, e.g. `account.buwiz.com`.
    pub fn new(turnstile: Option<TurnstileConfig>, hostname: &str) -> Self {
        Self {
            turnstile,
            hostname: hostname.to_owned(),
            http: reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build().unwrap_or_default(),
            networks: RateLimiter::new(CODES_PER_NETWORK, NETWORK_WINDOW),
            // Nothing is given back to these two, so a full map must not
            // lock new networks out (it would, sharing the overflow).
            app_attempts: RateLimiter::fail_open_when_full(APP_ATTEMPTS_PER_NETWORK, NETWORK_WINDOW),
            verify_tries: RateLimiter::fail_open_when_full(VERIFY_TRIES_PER_NETWORK, NETWORK_WINDOW),
            unbound: Mutex::new(Unbound::default()),
        }
    }

    /// The key of the widget the sign-in forms show, when Turnstile is on.
    pub fn site_key(&self) -> Option<&str> {
        self.turnstile.as_ref().map(|turnstile| turnstile.site_key.as_str())
    }

    /// Whether this form post may be mailed a code. `token` is what the
    /// widget put in the form's `cf-turnstile-response` field. The
    /// network's hour is counted only for posts Turnstile let through.
    pub async fn check(&self, headers: &HeaderMap, token: Option<&str>) -> Result<Counted, Refused> {
        let address = client_ip(headers);
        if let Some(turnstile) = &self.turnstile
            && !self.is_human(turnstile, token.unwrap_or_default(), address, Widget::Web).await
        {
            return Err(Refused::NotHuman);
        }
        // The time is read after Turnstile answered: a slow answer must
        // not come to the limiter with a time from before its window.
        self.count(address, OffsetDateTime::now_utc())
    }

    /// The app check of an in-app request for a code
    /// ([`crate::account::app_sign_in`]): `None` when there is nothing to
    /// check (Turnstile is off, or the app sent no token), else whether
    /// Turnstile accepts `token`, asked exactly as for the web's forms.
    pub async fn app_check(&self, headers: &HeaderMap, token: Option<&str>) -> Option<bool> {
        let turnstile = self.turnstile.as_ref()?;
        let token = token.filter(|token| !token.is_empty())?;
        Some(self.is_human(turnstile, token, client_ip(headers), Widget::App).await)
    }

    /// Counts an app's try at `/auth/email/start` or `/resend` against its
    /// network's hour ([`APP_ATTEMPTS_PER_NETWORK`]): `Err` with the
    /// seconds to wait when the hour's tries are used. Every try counts and
    /// none is given back, so refusals cost the network as much as codes.
    /// Without `CF-Connecting-IP` there is no network to count.
    pub fn count_app_attempt(&self, headers: &HeaderMap) -> Result<(), i64> {
        self.count_app_attempt_at(client_ip(headers), OffsetDateTime::now_utc())
    }

    fn count_app_attempt_at(&self, address: Option<IpAddr>, now: OffsetDateTime) -> Result<(), i64> {
        count_try(&self.app_attempts, address, now)
    }

    /// Counts a try at a code (`POST /auth/email/verify`, an app's or the
    /// web form's) against its network's hour ([`VERIFY_TRIES_PER_NETWORK`]):
    /// `Err` with the seconds to wait when the hour's tries are used. Every
    /// try counts and none is given back. Without `CF-Connecting-IP` there
    /// is no network to count.
    pub fn count_verify_try(&self, headers: &HeaderMap) -> Result<(), i64> {
        self.count_verify_try_at(client_ip(headers), OffsetDateTime::now_utc())
    }

    fn count_verify_try_at(&self, address: Option<IpAddr>, now: OffsetDateTime) -> Result<(), i64> {
        count_try(&self.verify_tries, address, now)
    }

    /// Whether staff are to be told now that Turnstile answered without the
    /// hostname or action: once per raise, at most once an hour.
    pub fn take_unbound_alert(&self) -> bool {
        std::mem::take(&mut self.unbound.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).alert)
    }

    /// Notes an answer without the hostname or action, at `now`.
    fn raise_unbound(&self, now: OffsetDateTime) {
        let mut unbound = self.unbound.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if unbound.raised.is_none_or(|at| now - at >= UNBOUND_ALERT_EVERY || now < at) {
            unbound.raised = Some(now);
            unbound.alert = true;
        }
    }

    /// Counts a post from `address` at `now` against its network's hour.
    fn count(&self, address: Option<IpAddr>, now: OffsetDateTime) -> Result<Counted, Refused> {
        // Without `CF-Connecting-IP` (development, or a tunnel that lost
        // it) there is no network to count, and counting them all as one
        // would let ten posts lock everyone out.
        let network = match address.map(Network::of) {
            Some(network) => {
                let bucket = self.networks.hit(network.clone(), now).map_err(|_| Refused::NetworkTooMany)?;
                Some((network, bucket))
            }
            None => None,
        };
        Ok(Counted { network, at: now })
    }

    /// Gives a network back a post [`Self::check`] counted, when no code
    /// was mailed for it after all (the address had its own limit). Others
    /// on that network are not held back by posts that sent nothing. Once
    /// the hour it was counted in has ended there is nothing to give back.
    pub fn forgive(&self, counted: &Counted) {
        if let Some((network, bucket)) = &counted.network {
            self.networks.release(network, counted.at, *bucket);
        }
    }

    /// Asks Turnstile whether `token` is from a person on `widget` on the
    /// account site. No answer counts as no: a code is not worth mailing on
    /// a guess.
    async fn is_human(
        &self,
        turnstile: &TurnstileConfig,
        token: &str,
        address: Option<IpAddr>,
        widget: Widget,
    ) -> bool {
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return false;
        }
        let mut question = json!({ "secret": turnstile.secret_key, "response": token });
        if let Some(address) = address {
            question["remoteip"] = json!(address.to_string());
        }
        let answer = self
            .http
            .post(&turnstile.verify_url)
            .json(&question)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        match answer {
            Ok(response) => match response.json::<Verdict>().await {
                Ok(verdict) => self.passes(&verdict, widget, OffsetDateTime::now_utc()),
                Err(error) => {
                    tracing::error!(%error, "Turnstile's answer could not be read; the sign-in is refused");
                    false
                }
            },
            Err(error) => {
                tracing::error!(%error, "Turnstile could not be reached; the sign-in is refused");
                false
            }
        }
    }
}

/// Counts a try from `address` at `now` in `limiter`: `Err` with the whole
/// seconds to wait. No address, no count.
fn count_try(limiter: &RateLimiter<Network>, address: Option<IpAddr>, now: OffsetDateTime) -> Result<(), i64> {
    let Some(address) = address else {
        return Ok(());
    };
    limiter.check(Network::of(address), now).map_err(|wait| {
        let seconds = wait.whole_seconds();
        if wait > Duration::seconds(seconds) { seconds + 1 } else { seconds }
    })
}

impl SignInGuard {
    /// Whether Turnstile's `verdict` lets a token of `widget` through. A
    /// hostname or action that is there and differs fails it. One that is
    /// missing (Cloudflare's own answer changed: no caller can drop it)
    /// fails an app's token, and lets a web form's through as before the
    /// fields were checked, logged and raised for staff
    /// ([`Self::take_unbound_alert`]), so web sign-in never stops for it.
    fn passes(&self, verdict: &Verdict, widget: Widget, now: OffsetDateTime) -> bool {
        if !verdict.success {
            return false;
        }
        let action = verdict.action.as_deref().map(|action| action == widget.action());
        let host = verdict.hostname.as_deref().map(|host| host == self.hostname);
        match (action, host) {
            (Some(true), Some(true)) => true,
            (Some(false), _) | (_, Some(false)) => {
                tracing::warn!("a Turnstile token for another widget or host was refused");
                false
            }
            _ if widget == Widget::App => {
                tracing::warn!("Turnstile's answer has no hostname or action; an app's token was refused");
                false
            }
            _ => {
                tracing::warn!("Turnstile's answer has no hostname or action; the web form's token was let through");
                self.raise_unbound(now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(address: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(crate::uptime::keys::CLIENT_IP_HEADER, address.parse().unwrap());
        headers
    }

    fn at(address: &str) -> Option<IpAddr> {
        Some(address.parse().unwrap())
    }

    /// Whether a post from `address` at `now` is counted, or why not.
    fn counted(guard: &SignInGuard, address: &str, now: OffsetDateTime) -> Result<(), Refused> {
        guard.count(at(address), now).map(|_| ())
    }

    #[test]
    fn a_network_gets_ten_codes_an_hour_whatever_address_it_posts_from() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        for last in 0..CODES_PER_NETWORK {
            assert_eq!(counted(&guard, &format!("203.0.113.{last}"), now), Ok(()));
        }
        assert_eq!(counted(&guard, "203.0.113.200", now), Err(Refused::NetworkTooMany));
        // Another network is not held back by it, and the hour ends.
        assert_eq!(counted(&guard, "198.51.100.7", now), Ok(()));
        assert_eq!(counted(&guard, "203.0.113.200", now + NETWORK_WINDOW), Ok(()));
    }

    #[test]
    fn a_forgiven_post_is_not_counted() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        let first = guard.count(at("203.0.113.9"), now).unwrap();
        for _ in 1..CODES_PER_NETWORK {
            assert_eq!(counted(&guard, "203.0.113.1", now), Ok(()));
        }
        assert_eq!(counted(&guard, "203.0.113.1", now), Err(Refused::NetworkTooMany));
        guard.forgive(&first);
        assert_eq!(counted(&guard, "203.0.113.1", now), Ok(()));
        assert_eq!(counted(&guard, "203.0.113.1", now), Err(Refused::NetworkTooMany));
        // A post without an address has nothing to give back.
        guard.forgive(&guard.count(None, now).unwrap());
        assert_eq!(counted(&guard, "203.0.113.1", now), Err(Refused::NetworkTooMany));
    }

    #[test]
    fn a_post_forgiven_after_its_hour_ended_takes_nothing_from_the_next() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        let old = guard.count(at("203.0.113.1"), now).unwrap();
        let next = now + NETWORK_WINDOW;
        for _ in 0..CODES_PER_NETWORK {
            assert_eq!(counted(&guard, "203.0.113.1", next), Ok(()));
        }
        guard.forgive(&old);
        assert_eq!(counted(&guard, "203.0.113.1", next), Err(Refused::NetworkTooMany));
    }

    #[tokio::test]
    async fn a_post_without_an_address_is_not_counted() {
        let guard = SignInGuard::new(None, "account.example.test");
        for _ in 0..CODES_PER_NETWORK * 2 {
            assert!(guard.check(&HeaderMap::new(), None).await.is_ok());
        }
    }

    #[tokio::test]
    async fn with_turnstile_on_a_post_without_a_token_is_refused_unasked() {
        // Nothing listens on the discard port: asking would fail too, but
        // an empty or oversized token is refused before anything is asked.
        let guard = SignInGuard::new(
            Some(TurnstileConfig {
                site_key: "site".into(),
                secret_key: "secret".into(),
                verify_url: "http://127.0.0.1:9".into(),
            }),
            "account.example.test",
        );
        let long = "x".repeat(MAX_TOKEN_LEN + 1);
        for token in [None, Some(""), Some(long.as_str())] {
            assert_eq!(guard.check(&from("203.0.113.1"), token).await.err(), Some(Refused::NotHuman));
        }
        // None of them used up the network's hour.
        assert_eq!(guard.networks.len(), 0);
    }

    #[test]
    fn the_account_host_is_parsed_from_the_origin() {
        assert_eq!(account_host("https://account.buwiz.com"), "account.buwiz.com");
        assert_eq!(account_host("http://localhost:3008"), "localhost");
        assert_eq!(account_host("http://127.0.0.1:3008"), "127.0.0.1");
        assert_eq!(account_host("http://[::1]:3008"), "[::1]");
        assert_eq!(account_host("not an origin"), "");
    }

    #[test]
    fn an_app_attempt_counts_whatever_comes_of_it_and_only_in_its_own_map() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        for last in 0..APP_ATTEMPTS_PER_NETWORK {
            assert_eq!(guard.count_app_attempt_at(at(&format!("203.0.113.{last}")), now), Ok(()));
        }
        let wait = guard.count_app_attempt_at(at("203.0.113.250"), now).unwrap_err();
        assert!((3599..=3600).contains(&wait), "{wait}");
        // Another network, and the next hour, go on; no address, no count.
        assert_eq!(guard.count_app_attempt_at(at("198.51.100.7"), now), Ok(()));
        assert_eq!(guard.count_app_attempt_at(at("203.0.113.1"), now + NETWORK_WINDOW), Ok(()));
        assert_eq!(guard.count_app_attempt_at(None, now), Ok(()));
        // The web form's count is untouched.
        assert_eq!(guard.networks.len(), 0);
        for _ in 0..CODES_PER_NETWORK {
            assert_eq!(counted(&guard, "203.0.113.9", now), Ok(()));
        }
    }

    #[test]
    fn verify_tries_count_per_network_in_a_map_of_their_own() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        for last in 0..VERIFY_TRIES_PER_NETWORK {
            assert_eq!(guard.count_verify_try_at(at(&format!("203.0.113.{}", last % 250)), now), Ok(()));
        }
        let wait = guard.count_verify_try_at(at("203.0.113.250"), now).unwrap_err();
        assert!((3599..=3600).contains(&wait), "{wait}");
        assert_eq!(guard.count_verify_try_at(at("198.51.100.7"), now), Ok(()));
        assert_eq!(guard.count_verify_try_at(at("203.0.113.1"), now + NETWORK_WINDOW), Ok(()));
        assert_eq!(guard.count_verify_try_at(None, now), Ok(()));
        // Neither the apps' start count nor the web form's is touched.
        assert_eq!(guard.networks.len(), 0);
        assert_eq!(guard.app_attempts.len(), 0);
    }

    /// The `i`th IPv4 /24, for filling a map.
    fn network(i: usize) -> Option<IpAddr> {
        let [_, a, b, c] = u32::try_from(i).unwrap().to_be_bytes();
        Some(IpAddr::from([10 + a, b, c, 1]))
    }

    /// A flood of one try each from as many networks as the map holds does
    /// not lock out a network that comes after it, at start/resend or at
    /// verify; a network already in the map is still limited. The web
    /// form's limiter, which gives refusals back, keeps its overflow.
    #[test]
    fn a_full_map_of_tries_locks_no_new_network_out() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        let max = crate::uptime::keys::MAX_KEYS;
        for i in 0..max {
            guard.count_app_attempt_at(network(i), now).unwrap();
            guard.count_verify_try_at(network(i), now).unwrap();
        }
        assert_eq!(guard.app_attempts.len(), max);
        assert_eq!(guard.verify_tries.len(), max);
        let newcomer = network(max + 1);
        for n in 0..VERIFY_TRIES_PER_NETWORK * 3 {
            assert_eq!(guard.count_app_attempt_at(newcomer, now), Ok(()), "{n}");
            assert_eq!(guard.count_verify_try_at(newcomer, now), Ok(()), "{n}");
            assert_eq!(guard.count_app_attempt_at(network(max + 2 + n as usize), now), Ok(()), "{n}");
        }
        let known = network(7);
        for _ in 1..APP_ATTEMPTS_PER_NETWORK {
            guard.count_app_attempt_at(known, now).unwrap();
        }
        assert!(guard.count_app_attempt_at(known, now).is_err());
        for _ in 1..VERIFY_TRIES_PER_NETWORK {
            guard.count_verify_try_at(known, now).unwrap();
        }
        assert!(guard.count_verify_try_at(known, now).is_err());
        assert_eq!(guard.app_attempts.len(), max);
    }

    fn verdict(action: Option<&str>, hostname: Option<&str>) -> Verdict {
        Verdict { success: true, action: action.map(str::to_owned), hostname: hostname.map(str::to_owned) }
    }

    #[test]
    fn a_missing_hostname_or_action_lets_the_web_through_once_alerted_and_refuses_the_app() {
        let host = "account.example.test";
        let guard = SignInGuard::new(None, host);
        let now = OffsetDateTime::now_utc();
        assert!(guard.passes(&verdict(Some(WEB_ACTION), Some(host)), Widget::Web, now));
        assert!(guard.passes(&verdict(Some(APP_ACTION), Some(host)), Widget::App, now));
        assert!(!guard.take_unbound_alert());
        // There and different: refused for both.
        assert!(!guard.passes(&verdict(Some(APP_ACTION), Some(host)), Widget::Web, now));
        assert!(!guard.passes(&verdict(Some(APP_ACTION), Some("evil.example.test")), Widget::App, now));
        assert!(!guard.passes(&verdict(None, Some("evil.example.test")), Widget::Web, now));
        let failed = Verdict { success: false, ..verdict(Some(WEB_ACTION), Some(host)) };
        assert!(!guard.passes(&failed, Widget::Web, now));
        assert!(!guard.take_unbound_alert());
        // Missing: the app is refused, with no alert.
        for missing in [verdict(None, Some(host)), verdict(Some(APP_ACTION), None), verdict(None, None)] {
            assert!(!guard.passes(&missing, Widget::App, now));
        }
        assert!(!guard.take_unbound_alert());
        // The web goes through, and staff are told once an hour.
        assert!(guard.passes(&verdict(None, Some(host)), Widget::Web, now));
        assert!(guard.passes(&verdict(Some(WEB_ACTION), None), Widget::Web, now + Duration::minutes(5)));
        assert!(guard.take_unbound_alert());
        assert!(!guard.take_unbound_alert());
        assert!(guard.passes(&verdict(None, None), Widget::Web, now + Duration::minutes(59)));
        assert!(!guard.take_unbound_alert());
        assert!(guard.passes(&verdict(None, None), Widget::Web, now + Duration::minutes(60)));
        assert!(guard.take_unbound_alert());
    }
}
