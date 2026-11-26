//! Settings, read once from the environment at start.

use anyhow::{Context, bail};
use base64::Engine;
use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

pub use crate::account::guard::SignInLimits;
use crate::base::storage::{DocumentStore, ObjectStore};
#[cfg(feature = "bir")]
use crate::bir;
use crate::sender::{Mode, RelaySettings};

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    /// Origin of the account site, e.g. `https://account.buwiz.com`. Device
    /// sign-in links and form origin checks use it.
    pub account_origin: String,
    /// Other origins that serve this app (e.g. `https://api.buwiz.com`).
    pub extra_origins: Vec<String>,
    /// Sets `Secure` and the `__Host-` prefix on the session cookie.
    pub secure_cookies: bool,
    /// AES-256-GCM key for claim documents at rest.
    pub document_key: [u8; 32],
    /// Accounts allowed to review TIN claims.
    pub admin_emails: Vec<String>,
    pub mail: MailConfig,
    /// Cloudflare Turnstile on the sign-in forms
    /// ([`crate::account::guard`]); `None` leaves them without it.
    pub turnstile: Option<TurnstileConfig>,
    /// The offline-signed relay trust list apps fetch (`TRUST_LIST_JWS`).
    pub trust_list: Option<String>,
    /// Where the app's installers are, e.g. `https://downloads.buwiz.com`
    /// (see [`crate::sites::downloads`]).
    pub downloads_url: String,
    /// Origin of the filing site, e.g. `https://ebirforms.buwiz.com`. Its pages'
    /// canonical addresses, the sitemap and the install command use it.
    pub forms_origin: String,
    /// The GitHub API, `https://api.github.com`, read for Buwiz Bot's star
    /// count (see [`crate::sites::github`]).
    pub github_api_url: String,
    /// `APP_SIGN_IN_MINUTES` (1 to 30, 5 when unset): how long an in-app
    /// sign-in code works, which is also how long before the app may ask
    /// for a new one ([`crate::account::app_sign_in`]).
    pub app_sign_in_minutes: i64,
    /// The per-network limits of sign-in tries and the shared budgets past
    /// a full map, and when staff are told of refusals
    /// ([`sign_in_limits`]).
    pub sign_in_limits: SignInLimits,
}

#[derive(Clone, Debug)]
pub enum MailConfig {
    /// Writes sign-in codes to the log. Development only.
    Log,
    Resend {
        api_key: String,
        from: String,
    },
}

/// A Turnstile widget's keys (`TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET_KEY`).
#[derive(Clone, Debug)]
pub struct TurnstileConfig {
    /// Public: the sign-in pages carry it.
    pub site_key: String,
    pub secret_key: String,
    /// Where a form post's token is checked: [`TURNSTILE_VERIFY_URL`],
    /// or a test's own.
    pub verify_url: String,
}

/// Cloudflare's endpoint that checks a Turnstile token.
pub const TURNSTILE_VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let account_origin = origin(&var_or("ACCOUNT_ORIGIN", "http://localhost:3008"))?;
        let secure_cookies = secure_cookies(&account_origin, std::env::var("SECURE_COOKIES").ok().as_deref())?;
        // Without an explicit MAIL_TRANSPORT, a Resend key means send email;
        // no key means print codes to the log.
        let has_resend_key = std::env::var("RESEND_API_KEY").is_ok_and(|key| !key.trim().is_empty());
        let default_transport = if has_resend_key { "resend" } else { "log" };
        let mail = match var_or("MAIL_TRANSPORT", default_transport).as_str() {
            "log" => MailConfig::Log,
            "resend" => MailConfig::Resend { api_key: required("RESEND_API_KEY")?, from: required("MAIL_FROM")? },
            other => bail!("MAIL_TRANSPORT must be `log` or `resend`, not `{other}`"),
        };
        if secure_cookies && matches!(mail, MailConfig::Log) {
            bail!("MAIL_TRANSPORT=log would print sign-in codes on a public deployment");
        }
        let turnstile = match (optional("TURNSTILE_SITE_KEY"), optional("TURNSTILE_SECRET_KEY")) {
            (Some(site_key), Some(secret_key)) => {
                Some(TurnstileConfig { site_key, secret_key, verify_url: TURNSTILE_VERIFY_URL.into() })
            }
            (None, None) => {
                if secure_cookies {
                    tracing::warn!(
                        "TURNSTILE_SITE_KEY and TURNSTILE_SECRET_KEY are not set: the sign-in forms have no check against scripts"
                    );
                }
                None
            }
            // Not a reason to stay down: a key that went missing would
            // take the whole site with it. The other limits still apply.
            _ => {
                tracing::error!(
                    "only one of TURNSTILE_SITE_KEY and TURNSTILE_SECRET_KEY is set: the sign-in forms go without Turnstile until both are"
                );
                None
            }
        };
        Ok(Self {
            host: var_or("HOST", "127.0.0.1"),
            port: var_or("PORT", "3008").parse().context("PORT")?,
            database_url: required("DATABASE_URL")?,
            account_origin,
            extra_origins: list("EXTRA_ORIGINS").iter().map(|raw| origin(raw)).collect::<anyhow::Result<_>>()?,
            secure_cookies,
            document_key: document_key(&required("DOCUMENT_KEY")?)?,
            admin_emails: admin_emails(),
            mail,
            turnstile,
            trust_list: std::env::var("TRUST_LIST_JWS")
                .ok()
                .map(|raw| raw.trim().to_owned())
                .filter(|raw| !raw.is_empty()),
            downloads_url: origin(&var_or("DOWNLOADS_URL", "https://downloads.buwiz.com")).context("DOWNLOADS_URL")?,
            forms_origin: origin(&var_or("FORMS_ORIGIN", "https://ebirforms.buwiz.com")).context("FORMS_ORIGIN")?,
            github_api_url: origin(&var_or("GITHUB_API_URL", "https://api.github.com")).context("GITHUB_API_URL")?,
            app_sign_in_minutes: app_sign_in_minutes(optional("APP_SIGN_IN_MINUTES").as_deref())?,
            sign_in_limits: sign_in_limits(optional)?,
        })
    }

    pub fn session_cookie(&self) -> &'static str {
        if self.secure_cookies { "__Host-buwiz_session" } else { "buwiz_session" }
    }

    pub fn is_admin(&self, email: &str) -> bool {
        crate::account::roles::in_list(&self.admin_emails, email)
    }

    pub fn is_own_origin(&self, origin: &str) -> bool {
        origin == self.account_origin || self.extra_origins.iter().any(|extra| extra == origin)
    }
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|value| !value.is_empty()).unwrap_or_else(|| default.to_owned())
}

/// A setting that may be left out; surrounding spaces are dropped.
fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty()).with_context(|| format!("{name} is not set"))
}

/// `ADMIN_EMAILS`, split on commas, trimmed and lowercased. An entry that
/// is not an email address (a list separated by spaces or semicolons, a
/// quoted value) is logged, so nobody is left believing it makes an Admin;
/// the server still starts.
pub fn admin_emails() -> Vec<String> {
    let (list, bad) = parse_admin_emails(&std::env::var("ADMIN_EMAILS").unwrap_or_default());
    for entry in &bad {
        tracing::warn!(%entry, "ADMIN_EMAILS has an entry that is not an email address; separate addresses with commas");
    }
    list
}

/// `raw` as `ADMIN_EMAILS` reads it: (every entry, those that are not an
/// email address).
pub fn parse_admin_emails(raw: &str) -> (Vec<String>, Vec<String>) {
    let list: Vec<String> =
        raw.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_lowercase).collect();
    // A sign-in address as Buwiz keeps it, in the characters addresses
    // use: a quote, a space or a semicolon means the list was written in
    // another way.
    let address = |entry: &str| {
        entry.chars().all(|ch| ch.is_ascii_alphanumeric() || "._+-@".contains(ch))
            && crate::account::accounts::normalize_email(entry).ok().as_deref() == Some(entry)
    };
    let bad = list.iter().filter(|entry| !address(entry)).cloned().collect();
    (list, bad)
}

/// `APP_SIGN_IN_MINUTES` as read: 5 when unset, else 1 to 30. Only the
/// apps' codes: the web's last ten minutes whatever this says.
pub fn app_sign_in_minutes(raw: Option<&str>) -> anyhow::Result<i64> {
    let Some(raw) = raw else {
        return Ok(5);
    };
    match raw.parse::<i64>() {
        Ok(minutes) if (1..=30).contains(&minutes) => Ok(minutes),
        _ => bail!("APP_SIGN_IN_MINUTES must be a whole number of minutes from 1 to 30, not `{raw}`"),
    }
}

/// `SECURE_COOKIES` as read: when unset, on for an https account origin.
/// Off with an https origin stops the server at start-up: off means "not a
/// public deployment", which also lets a loopback host take sign-in.
pub fn secure_cookies(account_origin: &str, raw: Option<&str>) -> anyhow::Result<bool> {
    let public = account_origin.starts_with("https://");
    let on = raw.map_or(public, |value| value == "true");
    if public && !on {
        bail!("SECURE_COOKIES must be true with an https ACCOUNT_ORIGIN; leave it unset");
    }
    Ok(on)
}

/// The sign-in limits as set, each a whole number in its range, else
/// today's value ([`SignInLimits::default`]). Any other value stops the
/// server at start-up. `var` reads a setting (trimmed, `None` when unset).
pub fn sign_in_limits(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<SignInLimits> {
    let today = SignInLimits::default();
    let read = |name: &str, default: u32, low: u32, high: u32| -> anyhow::Result<u32> {
        let Some(raw) = var(name) else {
            return Ok(default);
        };
        match raw.parse::<u32>() {
            Ok(value) if (low..=high).contains(&value) => Ok(value),
            _ => bail!("{name} must be a whole number from {low} to {high}, not `{raw}`"),
        }
    };
    Ok(SignInLimits {
        starts_per_network: read("APP_SIGN_IN_STARTS_PER_NETWORK", today.starts_per_network, 1, 1_000_000)?,
        verifies_per_network: read("SIGN_IN_VERIFIES_PER_NETWORK", today.verifies_per_network, 1, 1_000_000)?,
        starts_past_full: read("APP_SIGN_IN_STARTS_PAST_FULL", today.starts_past_full, 0, 10_000_000)?,
        verifies_past_full: read("SIGN_IN_VERIFIES_PAST_FULL", today.verifies_past_full, 0, 10_000_000)?,
        refusals_alert_at: read("SIGN_IN_REFUSALS_ALERT_AT", today.refusals_alert_at, 1, 10_000_000)?,
        token_failures_per_network: read("TOKEN_FAILURES_PER_NETWORK", today.token_failures_per_network, 1, 1_000_000)?,
        token_failures_past_full: read("TOKEN_FAILURES_PAST_FULL", today.token_failures_past_full, 0, 10_000_000)?,
    })
}

fn list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `scheme://host[:port]` without a trailing slash or path.
fn origin(raw: &str) -> anyhow::Result<String> {
    let uri: http::Uri = raw.parse().with_context(|| format!("not a URL: {raw}"))?;
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        bail!("not an absolute URL: {raw}");
    };
    if uri.path() != "/" && !uri.path().is_empty() {
        bail!("an origin has no path: {raw}");
    }
    Ok(format!("{scheme}://{authority}"))
}

fn document_key(raw: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw.trim()).context("DOCUMENT_KEY must be base64")?;
    bytes.try_into().map_err(|_| anyhow::anyhow!("DOCUMENT_KEY must be 32 bytes"))
}

/// `BIR_RELAY_MODE`: `dry_run` (default), `lab` or `live`.
pub fn relay_mode() -> anyhow::Result<Mode> {
    Ok(match var_or("BIR_RELAY_MODE", "dry_run").as_str() {
        "dry_run" => Mode::DryRun,
        "lab" => Mode::Lab,
        "live" => Mode::Live,
        other => bail!("BIR_RELAY_MODE must be dry_run, lab or live, not `{other}`"),
    })
}

/// Built without the `bir` feature: only `BIR_RELAY_MODE` is read, and
/// every send fails with [`crate::sender::NOT_BUILT_IN`].
#[cfg(not(feature = "bir"))]
pub fn relay_settings() -> anyhow::Result<RelaySettings> {
    let mode = relay_mode()?;
    tracing::warn!(mode = mode.as_str(), "built without the `bir` feature: every send fails, nothing reaches BIR");
    Ok(RelaySettings { mode })
}

/// Relay settings: [`relay_mode`] and the other `BIR_*` settings.
/// `BIR_SFTP_HOSTS` has no default and is required in `live`.
#[cfg(feature = "bir")]
pub fn relay_settings() -> anyhow::Result<RelaySettings> {
    let mode = relay_mode()?;
    let secret = |name: &str| -> anyhow::Result<Zeroizing<String>> {
        match std::env::var(name).ok().filter(|value| !value.is_empty()) {
            Some(value) => Ok(Zeroizing::new(value)),
            // A dry run encrypts with a placeholder: nothing is sent.
            None if mode == Mode::DryRun => Ok(Zeroizing::new("dry-run".to_owned())),
            None => bail!("{name} is required when BIR_RELAY_MODE={}", mode.as_str()),
        }
    };
    let pinned_keys = list("BIR_SSH_HOST_KEYS");
    if mode != Mode::DryRun && pinned_keys.is_empty() {
        bail!("BIR_SSH_HOST_KEYS must list the upload server's SHA256:… host-key fingerprints");
    }
    let lab = match mode {
        Mode::Lab => Some(lab_server(&required("BIR_LAB_SFTP")?)?),
        _ => None,
    };
    let sftp_hosts = list("BIR_SFTP_HOSTS");
    if mode == Mode::Live && sftp_hosts.is_empty() {
        bail!("BIR_SFTP_HOSTS must list the upload hosts BIR's dispatcher may name (see the BIR relay's docs)");
    }
    Ok(bir::Settings {
        mode,
        iaf_passphrase: secret("BIR_IAF_PASSPHRASE")?,
        dispatcher_passphrase: match mode {
            Mode::Live => secret("BIR_DISPATCHER_PASSPHRASE")?,
            _ => Zeroizing::new(String::new()),
        },
        dispatchers: bir::bir_dispatchers(),
        sftp_hosts,
        pinned_keys,
        lab,
        tls: bir::Settings::tls_config(),
        sftp_timeout: bir::sftp::TIMEOUT,
    })
}

/// `user:password@host:port`.
#[cfg(feature = "bir")]
fn lab_server(raw: &str) -> anyhow::Result<bir::LabServer> {
    let (login, address) = raw.rsplit_once('@').context("BIR_LAB_SFTP is user:password@host:port")?;
    let (username, password) = login.split_once(':').context("BIR_LAB_SFTP is user:password@host:port")?;
    let (host, port) = address.rsplit_once(':').context("BIR_LAB_SFTP is user:password@host:port")?;
    Ok(bir::LabServer {
        host: host.to_owned(),
        port: port.parse().context("BIR_LAB_SFTP port")?,
        username: username.to_owned(),
        password: Zeroizing::new(password.to_owned()),
    })
}

/// Where claim documents go: object storage when `S3_ENDPOINT` is set
/// (required on a public deployment), otherwise Postgres.
pub fn document_store(public: bool) -> anyhow::Result<DocumentStore> {
    let Some(endpoint) = std::env::var("S3_ENDPOINT").ok().filter(|value| !value.is_empty()) else {
        if public {
            bail!("S3_ENDPOINT and the other S3_* settings are required on a public deployment");
        }
        return Ok(DocumentStore::Database);
    };
    let store = ObjectStore::new(
        &endpoint,
        &var_or("S3_REGION", "auto"),
        &required("S3_BUCKET")?,
        &required("S3_ACCESS_KEY_ID")?,
        &required("S3_SECRET_ACCESS_KEY")?,
    )?;
    Ok(DocumentStore::Objects(Box::new(store)))
}

/// Ed25519 key that signs receipts (`RECEIPT_SIGNING_KEY`, a base64 32-byte
/// seed). Without one, development servers make a throwaway key.
pub fn receipt_key(public: bool) -> anyhow::Result<SigningKey> {
    signing_key("RECEIPT_SIGNING_KEY", public)
}

/// Ed25519 key that signs tickets (`TICKET_SIGNING_KEY`). The Sender trusts
/// its public half (`TICKET_PUBLIC_KEYS`).
pub fn ticket_key(public: bool) -> anyhow::Result<SigningKey> {
    signing_key("TICKET_SIGNING_KEY", public)
}

fn signing_key(name: &str, required: bool) -> anyhow::Result<SigningKey> {
    match std::env::var(name).ok().filter(|value| !value.is_empty()) {
        Some(raw) => {
            let seed: Zeroizing<Vec<u8>> = Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(raw.trim())
                    .with_context(|| format!("{name} must be base64"))?,
            );
            let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| anyhow::anyhow!("{name} must be 32 bytes"))?;
            Ok(SigningKey::from_bytes(&seed))
        }
        None if required => bail!("{name} is required on a public deployment"),
        None => {
            let mut seed = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rng(), &mut seed);
            tracing::warn!("{name} is not set; signing with a throwaway key");
            Ok(SigningKey::from_bytes(&seed))
        }
    }
}

/// Where the Sender is: `SENDER_URL` and `SENDER_SECRET`. `None` when
/// unset, which only a development server accepts.
pub fn sender_url(public: bool) -> anyhow::Result<Option<(String, Zeroizing<String>)>> {
    match std::env::var("SENDER_URL").ok().filter(|value| !value.is_empty()) {
        Some(url) => {
            let secret = Zeroizing::new(required("SENDER_SECRET")?);
            if secret.len() < 32 {
                bail!("SENDER_SECRET must be at least 32 characters");
            }
            Ok(Some((url, secret)))
        }
        None if public => bail!("SENDER_URL and SENDER_SECRET are required on a public deployment"),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_in_app_code_lasts_five_minutes_unless_set_from_one_to_thirty() {
        assert_eq!(app_sign_in_minutes(None).unwrap(), 5);
        assert_eq!(app_sign_in_minutes(Some("1")).unwrap(), 1);
        assert_eq!(app_sign_in_minutes(Some("30")).unwrap(), 30);
        for bad in ["0", "31", "-5", "ten", "2.5"] {
            assert!(app_sign_in_minutes(Some(bad)).is_err(), "{bad}");
        }
    }

    #[cfg(feature = "bir")]
    #[test]
    fn lab_servers_parse_with_a_colon_in_the_password() {
        let lab = lab_server("lab:pa:ss@127.0.0.1:2222").unwrap();
        assert_eq!((lab.username.as_str(), lab.password.as_str()), ("lab", "pa:ss"));
        assert_eq!((lab.host.as_str(), lab.port), ("127.0.0.1", 2222));
        assert!(lab_server("127.0.0.1:22").is_err());
    }

    #[test]
    fn admin_emails_split_on_commas_and_name_what_is_not_an_address() {
        let (list, bad) = parse_admin_emails(" Root@Example.test ,ops@example.test,, ");
        assert_eq!((list, bad), (vec!["root@example.test".to_owned(), "ops@example.test".to_owned()], Vec::new()));
        for raw in ["a@example.test; b@example.test", "a@example.test b@example.test", "\"a@example.test\"", "admin"] {
            let (list, bad) = parse_admin_emails(raw);
            assert_eq!(bad, list, "{raw}");
            assert_eq!(bad.len(), 1, "{raw}");
        }
    }

    #[test]
    fn sign_in_limits_are_todays_unless_set_to_a_whole_number_in_range() {
        let from = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
            sign_in_limits(move |name| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()))
        };
        assert_eq!(from(&[]).unwrap(), SignInLimits::default());
        assert_eq!(
            from(&[]).unwrap(),
            SignInLimits {
                starts_per_network: 200,
                verifies_per_network: 300,
                starts_past_full: 20_000,
                verifies_past_full: 30_000,
                refusals_alert_at: 100,
                token_failures_per_network: 300,
                token_failures_past_full: 30_000,
            }
        );
        let set = from(&[
            ("APP_SIGN_IN_STARTS_PER_NETWORK", "1000"),
            ("SIGN_IN_VERIFIES_PER_NETWORK", "1500"),
            ("APP_SIGN_IN_STARTS_PAST_FULL", "0"),
            ("SIGN_IN_VERIFIES_PAST_FULL", "10000000"),
            ("SIGN_IN_REFUSALS_ALERT_AT", "1"),
            ("TOKEN_FAILURES_PER_NETWORK", "5000"),
            ("TOKEN_FAILURES_PAST_FULL", "0"),
        ])
        .unwrap();
        assert_eq!(
            set,
            SignInLimits {
                starts_per_network: 1000,
                verifies_per_network: 1500,
                starts_past_full: 0,
                verifies_past_full: 10_000_000,
                refusals_alert_at: 1,
                token_failures_per_network: 5000,
                token_failures_past_full: 0,
            }
        );
        for (name, bad) in [
            ("APP_SIGN_IN_STARTS_PER_NETWORK", "0"),
            ("APP_SIGN_IN_STARTS_PER_NETWORK", "1000001"),
            ("SIGN_IN_VERIFIES_PER_NETWORK", "-1"),
            ("SIGN_IN_VERIFIES_PER_NETWORK", "2.5"),
            ("APP_SIGN_IN_STARTS_PAST_FULL", "20,000"),
            ("SIGN_IN_VERIFIES_PAST_FULL", "10000001"),
            ("SIGN_IN_REFUSALS_ALERT_AT", "0"),
            ("SIGN_IN_REFUSALS_ALERT_AT", "many"),
            ("TOKEN_FAILURES_PER_NETWORK", "0"),
            ("TOKEN_FAILURES_PER_NETWORK", " 300"),
            ("TOKEN_FAILURES_PAST_FULL", "10000001"),
        ] {
            let error = from(&[(name, bad)]).unwrap_err().to_string();
            assert!(error.starts_with(name) && error.contains(bad), "{name}={bad}: {error}");
        }
    }

    #[test]
    fn an_https_origin_needs_secure_cookies() {
        assert!(secure_cookies("https://account.buwiz.com", None).unwrap());
        assert!(secure_cookies("https://account.buwiz.com", Some("true")).unwrap());
        for off in ["false", "", "1", "TRUE"] {
            assert!(secure_cookies("https://account.buwiz.com", Some(off)).is_err(), "{off}");
        }
        assert!(!secure_cookies("http://localhost:3008", None).unwrap());
        assert!(!secure_cookies("http://localhost:3008", Some("false")).unwrap());
        assert!(secure_cookies("http://localhost:3008", Some("true")).unwrap());
    }

    #[test]
    fn origins_drop_the_trailing_slash_and_refuse_paths() {
        assert_eq!(origin("https://account.buwiz.com/").unwrap(), "https://account.buwiz.com");
        assert_eq!(origin("http://localhost:3008").unwrap(), "http://localhost:3008");
        assert!(origin("https://account.buwiz.com/login").is_err());
        assert!(origin("account.buwiz.com").is_err());
    }
}
