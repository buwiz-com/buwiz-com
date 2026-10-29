//! Settings, read once from the environment at start.

use anyhow::{Context, bail};
use base64::Engine;

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
        let mail = match var_or("MAIL_TRANSPORT", "log").as_str() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_drop_the_trailing_slash_and_refuse_paths() {
        assert_eq!(origin("https://account.buwiz.com/").unwrap(), "https://account.buwiz.com");
        assert_eq!(origin("http://localhost:3008").unwrap(), "http://localhost:3008");
        assert!(origin("https://account.buwiz.com/login").is_err());
        assert!(origin("account.buwiz.com").is_err());
    }
}
