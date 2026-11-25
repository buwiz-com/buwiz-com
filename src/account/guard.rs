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
//! same way. When their map is full, the networks new that hour share one
//! budget of tries for the hour, not counted by network
//! ([`RateLimiter::with_overflow_budget`]), so a flood of networks does not
//! lock the others out at once; when that is spent too, new networks are
//! refused until the hour ends.

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
/// carrier's or office's network: a carrier's NAT puts many phones behind
/// one /24, and many of them sign in at once near a filing deadline. A
/// person needs a start or two; what bounds one address is its five codes
/// an hour, whatever the network.
pub const APP_ATTEMPTS_PER_NETWORK: u32 = 200;

/// Tries at a code, `POST /auth/email/verify`, the apps' JSON and the web
/// form's together, one network may make in an hour, whatever comes of
/// them. Counted before the body is read, and never given back. As wide
/// as start's, for the same carrier networks; guessing is bounded per code
/// (five tries) and per address (five codes an hour), not by this.
pub const VERIFY_TRIES_PER_NETWORK: u32 = 300;

/// The tries at start and resend, across every network new in an hour
/// once the map of networks is full (100,000 networks this hour): one
/// budget for them all, not counted by network, refused once spent. Real
/// use does not reach it: it is used only after 100,000 networks have
/// tried this hour, and it is the tries of 100 more full networks. What it
/// costs is bounded: about 6 tries a second over the hour against start's
/// 4 places, where a try that is refused holds a place for a few short
/// queries, and at most this many Turnstile calls. Mail is bounded apart
/// from it (five codes an hour per address, the new-address budgets, the
/// testing cap).
pub const APP_ATTEMPTS_PAST_FULL: u32 = 20_000;

/// The same for tries at a code: about 8 a second over the hour against
/// verify's 3 places, and again the tries of 100 full networks. Guessing
/// stays bounded per code (five tries) and per address (five codes an
/// hour).
pub const VERIFY_TRIES_PAST_FULL: u32 = 30_000;

/// Failed tries at a token one network may make in an hour: an unknown or
/// wrong code, refresh token or device code on `/oauth/token` and
/// `/auth/device/poll`. Only failures count (a try that did not fail is
/// given back), and a network over the limit is refused before the
/// database. A working app fails rarely: a refresh token that a sign-out
/// ended, once, or a device code it polls after its sign-in was used. So
/// 300 is room for hundreds of apps behind one carrier's or office's
/// address failing in the same hour, and still holds a script to 300
/// lookups an hour, under one every ten seconds.
pub const TOKEN_FAILURES_PER_NETWORK: u32 = 300;

/// The same for networks new in an hour once the map is full, as
/// [`VERIFY_TRIES_PAST_FULL`].
pub const TOKEN_FAILURES_PAST_FULL: u32 = 30_000;

/// Refusals by those limits in an hour (start/resend's and verify's, by a
/// network's own limit or by the shared budget) at which staff are mailed:
/// [`SignInGuard::take_refusal_alert`]. Any refusal already means one
/// network used 200 starts or 300 tries at a code in the hour, more than a
/// few people do; a person who is refused tries again a few times at most,
/// so 100 is some tens of people turned away, not one network at the edge
/// of its limit.
pub const REFUSALS_ALERT_AT: u32 = 100;

/// The limits a deployment may set (`APP_SIGN_IN_STARTS_PER_NETWORK` and
/// the others, docs/deploy.md), today's values when unset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignInLimits {
    /// [`APP_ATTEMPTS_PER_NETWORK`].
    pub starts_per_network: u32,
    /// [`VERIFY_TRIES_PER_NETWORK`].
    pub verifies_per_network: u32,
    /// [`APP_ATTEMPTS_PAST_FULL`].
    pub starts_past_full: u32,
    /// [`VERIFY_TRIES_PAST_FULL`].
    pub verifies_past_full: u32,
    /// [`REFUSALS_ALERT_AT`].
    pub refusals_alert_at: u32,
    /// [`TOKEN_FAILURES_PER_NETWORK`].
    pub token_failures_per_network: u32,
    /// [`TOKEN_FAILURES_PAST_FULL`].
    pub token_failures_past_full: u32,
}

impl Default for SignInLimits {
    fn default() -> Self {
        Self {
            starts_per_network: APP_ATTEMPTS_PER_NETWORK,
            verifies_per_network: VERIFY_TRIES_PER_NETWORK,
            starts_past_full: APP_ATTEMPTS_PAST_FULL,
            verifies_past_full: VERIFY_TRIES_PAST_FULL,
            refusals_alert_at: REFUSALS_ALERT_AT,
            token_failures_per_network: TOKEN_FAILURES_PER_NETWORK,
            token_failures_past_full: TOKEN_FAILURES_PAST_FULL,
        }
    }
}

/// The tries refused in an hour, by which limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefusalCounts {
    /// Starts and resends refused by their network's own limit.
    pub starts_by_network: u64,
    /// Starts and resends of new networks refused by the shared budget.
    pub starts_by_budget: u64,
    /// Tries at a code refused by their network's own limit.
    pub verifies_by_network: u64,
    /// Tries at a code of new networks refused by the shared budget.
    pub verifies_by_budget: u64,
    /// Tries at a token refused by their network's own failures.
    pub tokens_by_network: u64,
    /// Tries at a token of new networks refused by the shared budget.
    pub tokens_by_budget: u64,
}

impl RefusalCounts {
    pub fn total(&self) -> u64 {
        self.starts_by_network
            + self.starts_by_budget
            + self.verifies_by_network
            + self.verifies_by_budget
            + self.tokens_by_network
            + self.tokens_by_budget
    }
}

/// Which limiter a try was counted in.
#[derive(Clone, Copy)]
enum Tries {
    Starts,
    Verifies,
    Tokens,
}

/// The current hour's refusals.
#[derive(Default)]
struct RefusalHour {
    since: Option<OffsetDateTime>,
    counts: RefusalCounts,
}

/// How often, at most, staff are mailed that Turnstile's answers lack the
/// hostname or action ([`SignInGuard::take_unbound_alert`]), that an
/// in-app request was stuck ([`SignInGuard::take_stuck_alert`]), or that
/// many tries were refused ([`SignInGuard::take_refusal_alert`]).
const ALERT_EVERY: Duration = Duration::hours(1);

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
    /// Failed tries at a token per network ([`TOKEN_FAILURES_PER_NETWORK`]).
    token_failures: RateLimiter<Network>,
    /// Turnstile's answers without a hostname or action, for the staff alert.
    unbound: HourlyAlert,
    /// An in-app request that could be neither taken back nor ended after
    /// its mail failed, for the staff alert.
    stuck: HourlyAlert,
    /// The hour's refusals by the two limiters above, for the hourly log
    /// line and the staff alert.
    refusals: Mutex<RefusalHour>,
    refused_many: HourlyAlert,
    refusals_alert_at: u64,
}

/// A staff alert raised at most once an hour, and taken once per raise.
#[derive(Default)]
struct HourlyAlert(Mutex<Raised>);

#[derive(Default)]
struct Raised {
    /// When it was last raised.
    at: Option<OffsetDateTime>,
    /// Raised and not yet taken for the staff mail.
    alert: bool,
}

impl HourlyAlert {
    /// Raises it at `now`, unless it was raised less than an hour before.
    fn raise(&self, now: OffsetDateTime) {
        let mut raised = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if raised.at.is_none_or(|at| now - at >= ALERT_EVERY || now < at) {
            raised.at = Some(now);
            raised.alert = true;
        }
    }

    /// Whether staff are to be told now: once per raise.
    fn take(&self) -> bool {
        std::mem::take(&mut self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).alert)
    }
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

/// A try at a token as [`SignInGuard::count_token_try`] counted it.
#[derive(Debug)]
pub struct TokenTry {
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
    /// With today's limits ([`SignInLimits::default`]).
    pub fn new(turnstile: Option<TurnstileConfig>, hostname: &str) -> Self {
        Self::with_limits(turnstile, hostname, SignInLimits::default())
    }

    /// [`Self::new`] with the limits the deployment set.
    pub fn with_limits(turnstile: Option<TurnstileConfig>, hostname: &str, limits: SignInLimits) -> Self {
        Self {
            turnstile,
            hostname: hostname.to_owned(),
            http: reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build().unwrap_or_default(),
            networks: RateLimiter::new(CODES_PER_NETWORK, NETWORK_WINDOW),
            // Nothing is given back to these two, so a full map must not
            // lock new networks out at once (it would, sharing an overflow
            // of one network's size): they share a budget of their own.
            app_attempts: RateLimiter::with_overflow_budget(
                limits.starts_per_network,
                NETWORK_WINDOW,
                limits.starts_past_full,
            ),
            verify_tries: RateLimiter::with_overflow_budget(
                limits.verifies_per_network,
                NETWORK_WINDOW,
                limits.verifies_past_full,
            ),
            token_failures: RateLimiter::with_overflow_budget(
                limits.token_failures_per_network,
                NETWORK_WINDOW,
                limits.token_failures_past_full,
            ),
            unbound: HourlyAlert::default(),
            stuck: HourlyAlert::default(),
            refusals: Mutex::new(RefusalHour::default()),
            refused_many: HourlyAlert::default(),
            refusals_alert_at: u64::from(limits.refusals_alert_at),
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
        self.count_try(Tries::Starts, address, now)
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
        self.count_try(Tries::Verifies, address, now)
    }

    /// Counts a try from `address` at `now` in the limiter of `tries`:
    /// `Err` with the whole seconds to wait. No address, no count. Every
    /// counted try notes the hour's refusals ([`Self::note`]).
    fn count_try(&self, tries: Tries, address: Option<IpAddr>, now: OffsetDateTime) -> Result<(), i64> {
        let Some(address) = address else {
            return Ok(());
        };
        let limiter = match tries {
            Tries::Starts => &self.app_attempts,
            Tries::Verifies => &self.verify_tries,
            Tries::Tokens => &self.token_failures,
        };
        let verdict = limiter.hit_or_refused(Network::of(address), now);
        self.note(tries, verdict.as_ref().err().map(|(bucket, _)| *bucket), now);
        verdict.map(|_| ()).map_err(|(_, wait)| {
            let seconds = wait.whole_seconds();
            if wait > Duration::seconds(seconds) { seconds + 1 } else { seconds }
        })
    }

    /// Notes a try at `now`, `refused` by a network's own count
    /// ([`Bucket::Key`]) or the shared budget ([`Bucket::Overflow`]). When
    /// an hour with refusals has ended, logs one line of its counts (no
    /// network, no address). When the hour's refusals reach
    /// `refusals_alert_at`, raises the staff alert.
    fn note(&self, tries: Tries, refused: Option<Bucket>, now: OffsetDateTime) {
        let (ended, total) = {
            let mut hour = self.refusals.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut ended = None;
            if hour.since.is_none_or(|since| now - since >= NETWORK_WINDOW || now < since) {
                ended = Some(std::mem::take(&mut hour.counts)).filter(|counts| counts.total() > 0);
                hour.since = Some(now);
            }
            let counts = &mut hour.counts;
            match (tries, refused) {
                (_, None) => {}
                (Tries::Starts, Some(Bucket::Key)) => counts.starts_by_network += 1,
                (Tries::Starts, Some(Bucket::Overflow)) => counts.starts_by_budget += 1,
                (Tries::Verifies, Some(Bucket::Key)) => counts.verifies_by_network += 1,
                (Tries::Verifies, Some(Bucket::Overflow)) => counts.verifies_by_budget += 1,
                (Tries::Tokens, Some(Bucket::Key)) => counts.tokens_by_network += 1,
                (Tries::Tokens, Some(Bucket::Overflow)) => counts.tokens_by_budget += 1,
            }
            (ended, counts.total())
        };
        if let Some(counts) = ended {
            tracing::warn!(
                starts_by_network = counts.starts_by_network,
                starts_by_budget = counts.starts_by_budget,
                verifies_by_network = counts.verifies_by_network,
                verifies_by_budget = counts.verifies_by_budget,
                tokens_by_network = counts.tokens_by_network,
                tokens_by_budget = counts.tokens_by_budget,
                "sign-in: tries refused in the last hour by the per-network limits and the shared budgets"
            );
        }
        if refused.is_some() && total >= self.refusals_alert_at {
            self.refused_many.raise(now);
        }
    }

    /// The refusals of the hour so far.
    pub fn refusals_this_hour(&self) -> RefusalCounts {
        self.refusals.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).counts
    }

    /// When staff are to be told now that many tries were refused this
    /// hour (once per raise, at most once an hour): the hour's counts.
    pub fn take_refusal_alert(&self) -> Option<RefusalCounts> {
        self.refused_many.take().then(|| self.refusals_this_hour())
    }

    /// Whether staff are to be told now that Turnstile answered without the
    /// hostname or action: once per raise, at most once an hour.
    pub fn take_unbound_alert(&self) -> bool {
        self.unbound.take()
    }

    /// Notes an in-app request that could not be taken back after its mail
    /// failed ([`crate::account::app_sign_in`]).
    pub fn raise_stuck(&self) {
        self.stuck.raise(OffsetDateTime::now_utc());
    }

    /// Whether staff are to be told now of a stuck request: once per raise,
    /// at most once an hour.
    pub fn take_stuck_alert(&self) -> bool {
        self.stuck.take()
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

    /// Counts a try at a token (`/oauth/token`, `/auth/device/poll`)
    /// against its network's failures this hour, before the database:
    /// `Err` with the seconds to wait when the network failed
    /// [`TOKEN_FAILURES_PER_NETWORK`] times. A try that did not fail is
    /// given back ([`Self::forgive_token_try`]), so only failures stay
    /// counted. Without `CF-Connecting-IP` there is no network to count.
    pub fn count_token_try(&self, headers: &HeaderMap) -> Result<TokenTry, i64> {
        self.count_token_try_at(client_ip(headers), OffsetDateTime::now_utc())
    }

    fn count_token_try_at(&self, address: Option<IpAddr>, now: OffsetDateTime) -> Result<TokenTry, i64> {
        let Some(address) = address else {
            return Ok(TokenTry { network: None, at: now });
        };
        let network = Network::of(address);
        let verdict = self.token_failures.hit_or_refused(network.clone(), now);
        self.note(Tries::Tokens, verdict.as_ref().err().map(|(bucket, _)| *bucket), now);
        match verdict {
            Ok(bucket) => Ok(TokenTry { network: Some((network, bucket)), at: now }),
            Err((_, wait)) => {
                let seconds = wait.whole_seconds();
                Err(if wait > Duration::seconds(seconds) { seconds + 1 } else { seconds })
            }
        }
    }

    /// Gives back a try at a token that did not fail.
    pub fn forgive_token_try(&self, tried: &TokenTry) {
        if let Some((network, bucket)) = &tried.network {
            self.token_failures.release(network, tried.at, *bucket);
        }
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
                self.unbound.raise(now);
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
    /// not lock out a network that comes after it, at once: the new
    /// networks share one budget for the hour, at start/resend and at
    /// verify apart, and are refused once it is spent, until the next hour.
    /// A network already in the map is counted on its own as before.
    #[test]
    fn a_full_map_gives_new_networks_one_budget_then_refuses_them() {
        let guard = SignInGuard::new(None, "account.example.test");
        let now = OffsetDateTime::now_utc();
        let max = crate::uptime::keys::MAX_KEYS;
        for i in 0..max {
            guard.count_app_attempt_at(network(i), now).unwrap();
            guard.count_verify_try_at(network(i), now).unwrap();
        }
        assert_eq!(guard.app_attempts.len(), max);
        assert_eq!(guard.verify_tries.len(), max);
        // One newcomer may take more than a network's own limit, and many
        // newcomers share the rest.
        let newcomer = network(max + 1);
        for n in 0..APP_ATTEMPTS_PER_NETWORK * 2 {
            assert_eq!(guard.count_app_attempt_at(newcomer, now), Ok(()), "{n}");
        }
        for n in APP_ATTEMPTS_PER_NETWORK * 2..APP_ATTEMPTS_PAST_FULL {
            assert_eq!(guard.count_app_attempt_at(network(max + 2 + n as usize), now), Ok(()), "{n}");
        }
        for n in 0..VERIFY_TRIES_PAST_FULL {
            assert_eq!(guard.count_verify_try_at(network(max + 1 + n as usize), now), Ok(()), "{n}");
        }
        // Spent: every new network is refused for the rest of the hour,
        // with the wait the per-network limit gives.
        for late in [newcomer, network(max * 3)] {
            let wait = guard.count_app_attempt_at(late, now).unwrap_err();
            assert!((3599..=3600).contains(&wait), "{wait}");
            let wait = guard.count_verify_try_at(late, now).unwrap_err();
            assert!((3599..=3600).contains(&wait), "{wait}");
        }
        // A network in the map goes on, up to its own limit.
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
        // Each refusal is counted by the limit that made it.
        let counts = RefusalCounts {
            starts_by_network: 1,
            starts_by_budget: 2,
            verifies_by_network: 1,
            verifies_by_budget: 2,
            ..RefusalCounts::default()
        };
        assert_eq!(guard.refusals_this_hour(), counts);
        // The next hour starts again: the newcomer is counted as a network.
        let next = now + NETWORK_WINDOW;
        assert_eq!(guard.count_app_attempt_at(newcomer, next), Ok(()));
        assert_eq!(guard.count_verify_try_at(newcomer, next), Ok(()));
        assert_eq!(guard.app_attempts.len(), 1);
    }

    /// The hour's refusals are counted by limit, and the staff alert is
    /// raised once they reach the threshold, once in the hour; the next
    /// hour counts from nothing.
    #[test]
    fn refusals_are_counted_and_alert_staff_once_past_the_threshold() {
        let limits = SignInLimits {
            starts_per_network: 2,
            verifies_per_network: 3,
            refusals_alert_at: 5,
            ..SignInLimits::default()
        };
        let guard = SignInGuard::with_limits(None, "account.example.test", limits);
        let now = OffsetDateTime::now_utc();
        let home = at("203.0.113.1");
        for _ in 0..2 {
            guard.count_app_attempt_at(home, now).unwrap();
        }
        for _ in 0..3 {
            guard.count_verify_try_at(home, now).unwrap();
        }
        assert_eq!(guard.refusals_this_hour(), RefusalCounts::default());
        for _ in 0..2 {
            assert!(guard.count_app_attempt_at(home, now).is_err());
            assert!(guard.count_verify_try_at(home, now).is_err());
        }
        // No address, no count; no refusal either.
        guard.count_app_attempt_at(None, now).unwrap();
        let four = RefusalCounts { starts_by_network: 2, verifies_by_network: 2, ..RefusalCounts::default() };
        assert_eq!(guard.refusals_this_hour(), four);
        assert_eq!(guard.take_refusal_alert(), None, "4 is below 5");
        assert!(guard.count_verify_try_at(home, now + Duration::minutes(1)).is_err());
        let five = RefusalCounts { verifies_by_network: 3, ..four };
        assert_eq!(guard.take_refusal_alert(), Some(five));
        assert_eq!(guard.take_refusal_alert(), None);
        assert!(guard.count_app_attempt_at(home, now + Duration::minutes(2)).is_err());
        assert_eq!(guard.take_refusal_alert(), None, "once an hour");
        // The next hour: the counts start again.
        let next = now + NETWORK_WINDOW + Duration::minutes(1);
        guard.count_app_attempt_at(home, next).unwrap();
        assert_eq!(guard.refusals_this_hour(), RefusalCounts::default());
    }

    #[test]
    fn a_stuck_request_raises_an_alert_taken_once_an_hour_at_most() {
        let guard = SignInGuard::new(None, "account.example.test");
        assert!(!guard.take_stuck_alert());
        let now = OffsetDateTime::now_utc();
        guard.stuck.raise(now);
        guard.stuck.raise(now + Duration::minutes(5));
        assert!(guard.take_stuck_alert());
        assert!(!guard.take_stuck_alert());
        guard.stuck.raise(now + Duration::minutes(59));
        assert!(!guard.take_stuck_alert());
        guard.stuck.raise(now + Duration::minutes(60));
        assert!(guard.take_stuck_alert());
        // Apart from the Turnstile alert.
        assert!(!guard.take_unbound_alert());
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
