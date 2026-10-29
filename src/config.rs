//! Settings, read once from the environment at start.

use anyhow::{Context, bail};
use base64::Engine;
use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

use crate::bir;
use crate::storage::{DocumentStore, ObjectStore};

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

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let account_origin = origin(&var_or("ACCOUNT_ORIGIN", "http://localhost:3008"))?;
        let secure_cookies = match std::env::var("SECURE_COOKIES") {
            Ok(value) => value == "true",
            Err(_) => account_origin.starts_with("https://"),
        };
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
        Ok(Self {
            host: var_or("HOST", "127.0.0.1"),
            port: var_or("PORT", "3008").parse().context("PORT")?,
            database_url: required("DATABASE_URL")?,
            account_origin,
            extra_origins: list("EXTRA_ORIGINS").iter().map(|raw| origin(raw)).collect::<anyhow::Result<_>>()?,
            secure_cookies,
            document_key: document_key(&required("DOCUMENT_KEY")?)?,
            admin_emails: list("ADMIN_EMAILS").into_iter().map(|email| email.to_lowercase()).collect(),
            mail,
        })
    }

    pub fn session_cookie(&self) -> &'static str {
        if self.secure_cookies { "__Host-buwiz_session" } else { "buwiz_session" }
    }

    pub fn is_admin(&self, email: &str) -> bool {
        self.admin_emails.iter().any(|admin| admin == email)
    }

    pub fn is_own_origin(&self, origin: &str) -> bool {
        origin == self.account_origin || self.extra_origins.iter().any(|extra| extra == origin)
    }
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|value| !value.is_empty()).unwrap_or_else(|| default.to_owned())
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty()).with_context(|| format!("{name} is not set"))
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

/// Relay settings: `BIR_RELAY_MODE` is `dry_run` (default), `lab` or `live`.
pub fn relay_settings() -> anyhow::Result<bir::Settings> {
    let mode = match var_or("BIR_RELAY_MODE", "dry_run").as_str() {
        "dry_run" => bir::Mode::DryRun,
        "lab" => bir::Mode::Lab,
        "live" => bir::Mode::Live,
        other => bail!("BIR_RELAY_MODE must be dry_run, lab or live, not `{other}`"),
    };
    let secret = |name: &str| -> anyhow::Result<Zeroizing<String>> {
        match std::env::var(name).ok().filter(|value| !value.is_empty()) {
            Some(value) => Ok(Zeroizing::new(value)),
            // A dry run encrypts with a placeholder: nothing is sent.
            None if mode == bir::Mode::DryRun => Ok(Zeroizing::new("dry-run".to_owned())),
            None => bail!("{name} is required when BIR_RELAY_MODE={}", mode.as_str()),
        }
    };
    let pinned_keys = list("BIR_SSH_HOST_KEYS");
    if mode != bir::Mode::DryRun && pinned_keys.is_empty() {
        bail!("BIR_SSH_HOST_KEYS must list the upload server's SHA256:… host-key fingerprints");
    }
    let lab = match mode {
        bir::Mode::Lab => Some(lab_server(&required("BIR_LAB_SFTP")?)?),
        _ => None,
    };
    let sftp_hosts = match list("BIR_SFTP_HOSTS") {
        hosts if hosts.is_empty() => vec!["ebf2.bir.gov.ph".to_owned()],
        hosts => hosts,
    };
    Ok(bir::Settings {
        mode,
        iaf_passphrase: secret("BIR_IAF_PASSPHRASE")?,
        dispatcher_passphrase: match mode {
            bir::Mode::Live => secret("BIR_DISPATCHER_PASSPHRASE")?,
            _ => Zeroizing::new(String::new()),
        },
        dispatchers: bir::bir_dispatchers(),
        sftp_hosts,
        pinned_keys,
        lab,
        tls: bir::Settings::tls_config(),
    })
}

/// `user:password@host:port`.
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
    fn lab_servers_parse_with_a_colon_in_the_password() {
        let lab = lab_server("lab:pa:ss@127.0.0.1:2222").unwrap();
        assert_eq!((lab.username.as_str(), lab.password.as_str()), ("lab", "pa:ss"));
        assert_eq!((lab.host.as_str(), lab.port), ("127.0.0.1", 2222));
        assert!(lab_server("127.0.0.1:22").is_err());
    }

    #[test]
    fn origins_drop_the_trailing_slash_and_refuse_paths() {
        assert_eq!(origin("https://account.buwiz.com/").unwrap(), "https://account.buwiz.com");
        assert_eq!(origin("http://localhost:3008").unwrap(), "http://localhost:3008");
        assert!(origin("https://account.buwiz.com/login").is_err());
        assert!(origin("account.buwiz.com").is_err());
    }
}
