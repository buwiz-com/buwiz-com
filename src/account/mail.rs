//! Sends sign-in codes (the web's and the apps'), codes for deleting an
//! account, and short
//! plain-text notices to staff (BIR uptime's alerts, [`crate::uptime::alerts`]).

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

/// The HTML of the code an app asked for ([`crate::account::app_sign_in`]);
/// `{{code}}`, `{{logo_url}}` and fixed words are filled in.
const APP_SIGN_IN_CODE_HTML: &str = include_str!("email/app_sign_in_code.html");

pub struct Mailer {
    config: MailConfig,
    /// Absolute URL of the PNG wordmark in the email header (mail apps do
    /// not show WebP or SVG reliably).
    logo_url: String,
    http: reqwest::Client,
    /// Tests only ([`Mailer::recording`]): notices are kept here, not sent.
    outbox: Option<Mutex<Vec<Notice>>>,
    /// Tests only ([`Mailer::recording`]): sign-in codes as (to, code),
    /// kept here, not sent.
    codes: Option<Mutex<Vec<(String, String)>>>,
    /// Tests only ([`Mailer::recording`]): codes for deleting an account as
    /// (to, code), kept here, not sent.
    deletion_codes: Option<Mutex<Vec<(String, String)>>>,
    /// Tests only ([`Mailer::recording`]): codes the apps asked for, as
    /// mailed, kept here, not sent.
    app_codes: Option<Mutex<Vec<AppCodeMail>>>,
    /// Tests only: a recording mailer that fails every notice.
    failing: AtomicBool,
    /// Tests only: a recording mailer that fails every app's code.
    failing_app_codes: AtomicBool,
}

/// A sign-in code an app asked for, as mailed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppCodeMail {
    pub to: String,
    pub code: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// What an app's code mail says besides the code. Only fixed words: no
/// text from the request goes into a mail.
#[derive(Clone, Copy, Debug)]
pub struct AppCodeWords {
    /// What asked, such as "eBIRForms on iPhone".
    pub device: &'static str,
    /// The person asked to create an account rather than sign in.
    pub create: bool,
    /// How long the code works (`APP_SIGN_IN_MINUTES`).
    pub minutes: i64,
}

/// `minutes` in words ("five minutes"), so the code stays the mail's only
/// number. 1 to 30, as `APP_SIGN_IN_MINUTES` allows.
fn minutes_in_words(minutes: i64) -> String {
    const WORDS: [&str; 31] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
        "twenty-one",
        "twenty-two",
        "twenty-three",
        "twenty-four",
        "twenty-five",
        "twenty-six",
        "twenty-seven",
        "twenty-eight",
        "twenty-nine",
        "thirty",
    ];
    let word = usize::try_from(minutes).ok().and_then(|index| WORDS.get(index)).copied().unwrap_or("a few");
    if minutes == 1 { format!("{word} minute") } else { format!("{word} minutes") }
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
            codes: None,
            deletion_codes: None,
            app_codes: None,
            failing: AtomicBool::new(false),
            failing_app_codes: AtomicBool::new(false),
        }
    }

    /// A mailer for tests: notices and sign-in codes are kept in memory
    /// ([`Mailer::notices`], [`Mailer::sign_in_codes`]) and nothing goes out.
    pub fn recording(site_origin: &str) -> Self {
        Self {
            outbox: Some(Mutex::new(Vec::new())),
            codes: Some(Mutex::new(Vec::new())),
            deletion_codes: Some(Mutex::new(Vec::new())),
            app_codes: Some(Mutex::new(Vec::new())),
            ..Self::new(MailConfig::Log, site_origin)
        }
    }

    /// The sign-in codes a [`Mailer::recording`] kept, as (to, code), oldest
    /// first.
    pub fn sign_in_codes(&self) -> Vec<(String, String)> {
        self.codes
            .as_ref()
            .map_or_else(Vec::new, |codes| codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// The codes for deleting an account a [`Mailer::recording`] kept, as
    /// (to, code), oldest first.
    pub fn deletion_codes(&self) -> Vec<(String, String)> {
        self.deletion_codes
            .as_ref()
            .map_or_else(Vec::new, |codes| codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// The codes apps asked for that a [`Mailer::recording`] kept, oldest
    /// first.
    pub fn app_sign_in_mails(&self) -> Vec<AppCodeMail> {
        self.app_codes
            .as_ref()
            .map_or_else(Vec::new, |codes| codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// Tests only: a [`Mailer::recording`] fails every app's code while
    /// `fail`.
    pub fn fail_app_codes(&self, fail: bool) {
        self.failing_app_codes.store(fail, Ordering::SeqCst);
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
        if let Some(codes) = &self.codes {
            codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((to.into(), code.into()));
            return Ok(());
        }
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

    /// A sign-in code an app asked for: its own message, which names the
    /// app and the device ([`AppCodeWords`]). The web's sign-in mail is
    /// [`Self::send_sign_in_code`].
    pub async fn send_app_sign_in_code(&self, to: &str, code: &str, words: AppCodeWords) -> anyhow::Result<()> {
        let message = CodeMessage::app(code, words, &self.logo_url);
        if let Some(codes) = &self.app_codes {
            if self.failing_app_codes.load(Ordering::SeqCst) {
                anyhow::bail!("made-up mail failure");
            }
            let mail = AppCodeMail {
                to: to.into(),
                code: code.into(),
                subject: message.subject,
                text: message.text,
                html: message.html,
            };
            codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(mail);
            return Ok(());
        }
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, code, device = words.device, "app sign-in code (MAIL_TRANSPORT=log)");
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

    /// The code for deleting the account at `to`, as plain text: a short
    /// message of its own, so it is never taken for a sign-in code.
    pub async fn send_deletion_code(&self, to: &str, code: &str) -> anyhow::Result<()> {
        if let Some(codes) = &self.deletion_codes {
            codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((to.into(), code.into()));
            return Ok(());
        }
        let (subject, text) = deletion_message(code);
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, code, "account deletion code (MAIL_TRANSPORT=log)");
                Ok(())
            }
            MailConfig::Resend { api_key, from } => {
                self.http
                    .post("https://api.resend.com/emails")
                    .timeout(CODE_TIMEOUT)
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
}

/// The subject and text of the email with a code for deleting an account.
/// As in the sign-in email, the code is the only number.
fn deletion_message(code: &str) -> (String, String) {
    (
        format!("{code} is your code to delete your Buwiz account"),
        format!(
            "Your code to delete your Buwiz account is {code}\n\n\
             Type it on the Delete your account page to delete the account for good. \
             It expires in ten minutes and works once.\n\n\
             Didn't ask to delete your account? Don't type this code anywhere, and sign in to check your account. \
             Nobody can delete it without this code.\n\n\
             Buwiz - https://buwiz.com"
        ),
    )
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

    /// The mail of a code an app asked for. The code is still the only
    /// number; the subject names the device instead.
    fn app(code: &str, words: AppCodeWords, logo_url: &str) -> Self {
        let device = words.device;
        let minutes = minutes_in_words(words.minutes);
        let (heading, lead, asked) = if words.create {
            (
                "Create your Buwiz account",
                format!("Enter this verification code in {device} to create your Buwiz account."),
                format!(
                    "Someone asked to create a Buwiz account in {device} with this address. \
                     If it wasn't you, ignore this mail."
                ),
            )
        } else {
            (
                "Sign in to eBIRForms",
                format!("Enter this verification code in {device} to sign in."),
                format!("Someone asked to sign in to {device} with this address. If it wasn't you, ignore this mail."),
            )
        };
        Self {
            subject: format!("Your Buwiz sign-in code for {device}"),
            text: format!(
                "Your Buwiz verification code is {code}\n\n\
                 {lead} It expires in {minutes} and works once.\n\n\
                 Never give this code to anyone. Buwiz will never ask for it by phone, chat or email, \
                 not even someone who says they are from Buwiz or the BIR.\n\n\
                 {asked} Nobody can get into your account without this code.\n\n\
                 Buwiz - https://buwiz.com"
            ),
            html: APP_SIGN_IN_CODE_HTML
                .replace("{{device}}", device)
                .replace("{{heading}}", heading)
                .replace("{{lead}}", &lead)
                .replace("{{asked}}", &asked)
                .replace("{{minutes}}", &minutes)
                .replace("{{code}}", code)
                .replace("{{logo_url}}", logo_url),
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
    fn the_deletion_code_is_the_only_number_and_leads_the_subject() {
        let (subject, text) = deletion_message("042917");
        assert!(subject.starts_with("042917 "));
        assert!(text.contains("delete your Buwiz account is 042917"));
        for part in [subject.as_str(), text.as_str()] {
            assert!(numbers(part).iter().all(|number| *number == "042917"), "{part}");
        }
    }

    #[test]
    fn an_app_code_names_the_device_and_is_still_the_only_number() {
        for create in [false, true] {
            let words = AppCodeWords { device: "eBIRForms on iPhone", create, minutes: 5 };
            let message = CodeMessage::app("042917", words, "https://account.buwiz.com/assets/brand/x.png");
            assert_eq!(message.subject, "Your Buwiz sign-in code for eBIRForms on iPhone");
            assert!(message.text.contains("Never give this code to anyone"));
            assert!(message.text.contains("It expires in five minutes"), "{}", message.text);
            assert!(message.html.contains("Expires in five minutes"));
            assert!(!message.html.contains("{{"));
            for part in [message.text.as_str(), visible(&message.html).as_str()] {
                assert!(numbers(part).iter().all(|number| *number == "042917"), "{part}");
                assert!(part.contains("042917"));
            }
        }
    }

    #[test]
    fn minutes_are_in_words() {
        assert_eq!(minutes_in_words(1), "one minute");
        assert_eq!(minutes_in_words(5), "five minutes");
        assert_eq!(minutes_in_words(30), "thirty minutes");
    }

    #[test]
    fn the_template_is_fully_filled_in() {
        let message = CodeMessage::new("042917", "https://account.buwiz.com/assets/brand/buwiz_wordmark_email.png");
        assert!(!message.html.contains("{{"));
        assert!(message.html.contains(r#"src="https://account.buwiz.com/assets/brand/buwiz_wordmark_email.png""#));
    }
}
