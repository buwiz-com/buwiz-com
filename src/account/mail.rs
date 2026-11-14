//! Sends sign-in codes, and short plain-text notices to staff (BIR uptime's
//! alerts, [`crate::uptime::alerts`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use serde_json::json;

use crate::base::config::MailConfig;

/// How long connecting to the mail provider may take ...
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// ... and one whole request, so a provider that hangs never holds BIR
/// uptime's ticker for longer ...
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// ... but a sign-in code gets longer: a slow provider that still delivers
/// it must not show the person an error page for a code that arrives.
const CODE_TIMEOUT: Duration = Duration::from_secs(60);

/// The HTML sign-in code email; `{{code}}` and `{{logo_url}}` are filled in.
const SIGN_IN_CODE_HTML: &str = include_str!("email/sign_in_code.html");

pub struct Mailer {
    config: MailConfig,
    /// Absolute URL of the PNG wordmark in the email header (mail apps do
    /// not show WebP or SVG reliably).
    logo_url: String,
    http: reqwest::Client,
    /// Tests only ([`Mailer::recording`]): notices are kept here, not sent.
    outbox: Option<Mutex<Vec<Notice>>>,
    /// Tests only: a recording mailer that fails every notice.
    failing: AtomicBool,
}

/// A staff notice as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub to: String,
    pub subject: String,
    pub text: String,
}

impl Mailer {
    pub fn new(config: MailConfig, site_origin: &str) -> Self {
        Self {
            config,
            logo_url: format!("{site_origin}/assets/brand/buwiz_wordmark_email.png"),
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            outbox: None,
            failing: AtomicBool::new(false),
        }
    }

    /// A mailer for tests: notices are kept in memory ([`Mailer::notices`])
    /// and nothing goes out; sign-in codes are logged.
    pub fn recording(site_origin: &str) -> Self {
        Self { outbox: Some(Mutex::new(Vec::new())), ..Self::new(MailConfig::Log, site_origin) }
    }

    /// Tests only: a [`Mailer::recording`] fails every notice while `fail`.
    pub fn fail_notices(&self, fail: bool) {
        self.failing.store(fail, Ordering::SeqCst);
    }

    /// The notices a [`Mailer::recording`] kept, oldest first.
    pub fn notices(&self) -> Vec<Notice> {
        self.outbox
            .as_ref()
            .map_or_else(Vec::new, |outbox| outbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// A short plain-text notice to one staff address.
    pub async fn send_notice(&self, to: &str, subject: &str, text: &str) -> anyhow::Result<()> {
        if let Some(outbox) = &self.outbox {
            if self.failing.load(Ordering::SeqCst) {
                anyhow::bail!("made-up mail failure");
            }
            let notice = Notice { to: to.into(), subject: subject.into(), text: text.into() };
            outbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(notice);
            return Ok(());
        }
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, subject, text, "staff notice (MAIL_TRANSPORT=log)");
                Ok(())
            }
            MailConfig::Resend { api_key, from } => {
                self.http
                    .post("https://api.resend.com/emails")
                    .bearer_auth(api_key)
                    .json(&json!({ "from": from, "to": [to], "subject": subject, "text": text }))
                    .send()
                    .await
                    .context("mail request failed")?
                    .error_for_status()
                    .context("mail provider refused the message")?;
                Ok(())
            }
        }
    }

    pub async fn send_sign_in_code(&self, to: &str, code: &str) -> anyhow::Result<()> {
        let message = CodeMessage::new(code, &self.logo_url);
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, code, "sign-in code (MAIL_TRANSPORT=log)");
                Ok(())
            }
            MailConfig::Resend { api_key, from } => {
                self.http
                    .post("https://api.resend.com/emails")
                    .timeout(CODE_TIMEOUT)
                    .bearer_auth(api_key)
                    .json(&json!({
                        "from": from,
                        "to": [to],
                        "subject": message.subject,
                        "text": message.text,
                        "html": message.html,
                    }))
                    .send()
                    .await
                    .context("mail request failed")?
                    .error_for_status()
                    .context("mail provider refused the message")?;
                Ok(())
            }
        }
    }
}

/// A sign-in code email. Mail apps (Gmail's "Copy code", Apple's code
/// autofill) find the code on their own when it is the only number, sits
/// next to the words "verification code", and leads the subject.
struct CodeMessage {
    subject: String,
    text: String,
    html: String,
}

impl CodeMessage {
    fn new(code: &str, logo_url: &str) -> Self {
        Self {
            subject: format!("{code} is your Buwiz verification code"),
            text: format!(
                "Your Buwiz verification code is {code}\n\n\
                 Enter it on the sign-in page to continue. It expires in ten minutes and works once.\n\n\
                 Keep this code to yourself. Buwiz will never ask for it by phone, chat or email, \
                 not even someone who says they are from Buwiz or the BIR.\n\n\
                 Didn't try to sign in? You can ignore this email. Nobody can get into your account without this code.\n\n\
                 Buwiz - https://buwiz.com"
            ),
            html: SIGN_IN_CODE_HTML.replace("{{code}}", code).replace("{{logo_url}}", logo_url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs of digits in `text`.
    fn numbers(text: &str) -> Vec<&str> {
        text.split(|c: char| !c.is_ascii_digit()).filter(|run| !run.is_empty()).collect()
    }

    /// Text a reader sees: everything outside tags, without the `<style>` block.
    fn visible(html: &str) -> String {
        let body = html.split("</style>").nth(1).expect("the template has a style block");
        body.split('>').filter_map(|chunk| chunk.split('<').next()).collect()
    }

    #[test]
    fn the_code_is_the_only_number_and_leads_the_subject() {
        let message = CodeMessage::new("042917", "https://account.buwiz.com/assets/brand/buwiz_wordmark_email.png");
        assert!(message.subject.starts_with("042917 "));
        assert!(message.text.contains("verification code is 042917"));
        for part in [message.subject.as_str(), message.text.as_str(), visible(&message.html).as_str()] {
            assert!(numbers(part).iter().all(|number| *number == "042917"), "{part}");
        }
    }

    #[test]
    fn the_template_is_fully_filled_in() {
        let message = CodeMessage::new("042917", "https://account.buwiz.com/assets/brand/buwiz_wordmark_email.png");
        assert!(!message.html.contains("{{"));
        assert!(message.html.contains(r#"src="https://account.buwiz.com/assets/brand/buwiz_wordmark_email.png""#));
    }
}
