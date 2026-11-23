//! Keeps scripts from mailing sign-in codes to strangers.
//!
//! Anyone can type any address into the sign-in form, and that address is
//! mailed a code. A script that types a new address each time gets past the
//! limit per address ([`crate::account::accounts::issue_code`]), so two
//! more checks come before a code is made: Cloudflare Turnstile on the form
//! (when `TURNSTILE_SITE_KEY` and `TURNSTILE_SECRET_KEY` are set) and a
//! limit per network. The limit on codes for addresses without an account
//! is the last one, in `issue_code`.

use std::net::IpAddr;

use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};

use crate::base::config::TurnstileConfig;
use crate::uptime::keys::{Network, RateLimiter, client_ip};

/// Codes one network (an IPv4 /24, an IPv6 /48) may ask for in an hour. An
/// office or a phone carrier shares one, so it is well above what one
/// person needs.
const CODES_PER_NETWORK: u32 = 10;
const NETWORK_WINDOW: Duration = Duration::hours(1);

/// How long Turnstile may take to answer before the sign-in is refused.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Turnstile's tokens are at most this long; a longer one is not sent on.
const MAX_TOKEN_LEN: usize = 2048;

/// The script that draws Turnstile's widget.
pub const WIDGET_SCRIPT: &str = "https://challenges.cloudflare.com/turnstile/v0/api.js";

pub struct SignInGuard {
    turnstile: Option<TurnstileConfig>,
    http: reqwest::Client,
    networks: RateLimiter<Network>,
}

/// Why a request for a code was refused before one was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Turnstile did not vouch for the form post.
    NotHuman,
    /// The caller's network asked for too many codes this hour.
    NetworkTooMany,
}

/// Turnstile's answer; its other fields are not used.
#[derive(Deserialize)]
struct Verdict {
    success: bool,
}

impl SignInGuard {
    pub fn new(turnstile: Option<TurnstileConfig>) -> Self {
        Self {
            turnstile,
            http: reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build().unwrap_or_default(),
            networks: RateLimiter::new(CODES_PER_NETWORK, NETWORK_WINDOW),
        }
    }

    /// The key of the widget the sign-in forms show, when Turnstile is on.
    pub fn site_key(&self) -> Option<&str> {
        self.turnstile.as_ref().map(|turnstile| turnstile.site_key.as_str())
    }

    /// Whether this form post may be mailed a code. `token` is what the
    /// widget put in the form's `cf-turnstile-response` field. The
    /// network's hour is counted only for posts Turnstile let through.
    pub async fn check(&self, headers: &HeaderMap, token: Option<&str>) -> Result<(), Refused> {
        let address = client_ip(headers);
        if let Some(turnstile) = &self.turnstile
            && !self.is_human(turnstile, token.unwrap_or_default(), address).await
        {
            return Err(Refused::NotHuman);
        }
        // The time is read after Turnstile answered: a slow answer must
        // not come to the limiter with a time from before its window.
        self.count(address, OffsetDateTime::now_utc())
    }

    /// Counts a post from `address` at `now` against its network's hour.
    fn count(&self, address: Option<IpAddr>, now: OffsetDateTime) -> Result<(), Refused> {
        // Without `CF-Connecting-IP` (development, or a tunnel that lost
        // it) there is no network to count, and counting them all as one
        // would let ten posts lock everyone out.
        match address {
            Some(address) => self.networks.check(Network::of(address), now).map_err(|_| Refused::NetworkTooMany),
            None => Ok(()),
        }
    }

    /// Gives a network back the post [`Self::check`] counted, when no code
    /// was mailed for it after all (the address had its own limit). Others
    /// on that network are not held back by posts that sent nothing.
    pub fn forgive(&self, headers: &HeaderMap) {
        if let Some(address) = client_ip(headers) {
            self.networks.release(&Network::of(address));
        }
    }

    /// Asks Turnstile whether `token` is from a person on our form. No
    /// answer counts as no: a code is not worth mailing on a guess.
    async fn is_human(&self, turnstile: &TurnstileConfig, token: &str, address: Option<IpAddr>) -> bool {
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
                Ok(verdict) => verdict.success,
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

    #[test]
    fn a_network_gets_ten_codes_an_hour_whatever_address_it_posts_from() {
        let guard = SignInGuard::new(None);
        let now = OffsetDateTime::now_utc();
        for last in 0..CODES_PER_NETWORK {
            assert_eq!(guard.count(at(&format!("203.0.113.{last}")), now), Ok(()));
        }
        assert_eq!(guard.count(at("203.0.113.200"), now), Err(Refused::NetworkTooMany));
        // Another network is not held back by it, and the hour ends.
        assert_eq!(guard.count(at("198.51.100.7"), now), Ok(()));
        assert_eq!(guard.count(at("203.0.113.200"), now + NETWORK_WINDOW), Ok(()));
    }

    #[test]
    fn a_forgiven_post_is_not_counted() {
        let guard = SignInGuard::new(None);
        let now = OffsetDateTime::now_utc();
        for _ in 0..CODES_PER_NETWORK {
            assert_eq!(guard.count(at("203.0.113.1"), now), Ok(()));
        }
        assert_eq!(guard.count(at("203.0.113.1"), now), Err(Refused::NetworkTooMany));
        guard.forgive(&from("203.0.113.9"));
        assert_eq!(guard.count(at("203.0.113.1"), now), Ok(()));
        assert_eq!(guard.count(at("203.0.113.1"), now), Err(Refused::NetworkTooMany));
        // Forgiving a network that has nothing counted changes nothing.
        guard.forgive(&from("198.51.100.7"));
        guard.forgive(&HeaderMap::new());
        assert_eq!(guard.count(at("203.0.113.1"), now), Err(Refused::NetworkTooMany));
    }

    #[tokio::test]
    async fn a_post_without_an_address_is_not_counted() {
        let guard = SignInGuard::new(None);
        for _ in 0..CODES_PER_NETWORK * 2 {
            assert_eq!(guard.check(&HeaderMap::new(), None).await, Ok(()));
        }
    }

    #[tokio::test]
    async fn with_turnstile_on_a_post_without_a_token_is_refused_unasked() {
        // Nothing listens on the discard port: asking would fail too, but
        // an empty or oversized token is refused before anything is asked.
        let guard = SignInGuard::new(Some(TurnstileConfig {
            site_key: "site".into(),
            secret_key: "secret".into(),
            verify_url: "http://127.0.0.1:9".into(),
        }));
        let long = "x".repeat(MAX_TOKEN_LEN + 1);
        for token in [None, Some(""), Some(long.as_str())] {
            assert_eq!(guard.check(&from("203.0.113.1"), token).await, Err(Refused::NotHuman));
        }
        // None of them used up the network's hour.
        assert_eq!(guard.networks.len(), 0);
    }
}
