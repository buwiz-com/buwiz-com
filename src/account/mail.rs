//! Sends sign-in codes (the web's and the apps'), codes for deleting an
//! account, codes for resetting an app's PIN and the notice that one was
//! verified, and short
//! plain-text notices to staff (BIR uptime's alerts, [`crate::uptime::alerts`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// The account site's origin, for links in fixed-word mails.
    site_origin: String,
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
    /// Tests only ([`Mailer::recording`]): new-device mails, kept here, not
    /// sent.
    new_devices: Option<Mutex<Vec<Notice>>>,
    /// Tests only ([`Mailer::recording`]): PIN reset codes, as mailed, kept
    /// here, not sent.
    pin_reset_codes: Option<Mutex<Vec<AppCodeMail>>>,
    /// Tests only ([`Mailer::recording`]): the notices of verified PIN
    /// resets, kept here, not sent.
    pin_reset_notices: Option<Mutex<Vec<Notice>>>,
    /// Tests only: a recording mailer that fails every notice.
    failing: AtomicBool,
    /// Tests only: a recording mailer that fails every new-device mail.
    failing_new_devices: AtomicBool,
    /// Tests only: new-device tasks wait at their start while this is set
    /// ([`Mailer::hold_new_device_mails`]).
    holding_new_devices: AtomicBool,
    /// Tests only: a recording mailer that fails every app's code.
    failing_app_codes: AtomicBool,
    /// Tests only: a recording mailer that fails every PIN reset code.
    failing_pin_reset_codes: AtomicBool,
    /// Tests only: a recording mailer that fails every PIN reset notice.
    failing_pin_reset_notices: AtomicBool,
    /// Tests only: PIN reset notices wait at their start while this is set
    /// ([`Mailer::hold_pin_reset_notices`]).
    holding_pin_reset_notices: AtomicBool,
    /// How many PIN reset notices a [`Mailer::recording`] began to send.
    pin_reset_notices_begun: AtomicUsize,
    /// How many new-device checks have finished, mail or not: tests wait
    /// on it, as the check runs on its own task.
    new_device_checks: AtomicUsize,
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
            site_origin: site_origin.to_owned(),
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            outbox: None,
            codes: None,
            deletion_codes: None,
            app_codes: None,
            new_devices: None,
            pin_reset_codes: None,
            pin_reset_notices: None,
            failing: AtomicBool::new(false),
            failing_app_codes: AtomicBool::new(false),
            failing_pin_reset_codes: AtomicBool::new(false),
            failing_pin_reset_notices: AtomicBool::new(false),
            holding_pin_reset_notices: AtomicBool::new(false),
            pin_reset_notices_begun: AtomicUsize::new(0),
            failing_new_devices: AtomicBool::new(false),
            holding_new_devices: AtomicBool::new(false),
            new_device_checks: AtomicUsize::new(0),
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
            new_devices: Some(Mutex::new(Vec::new())),
            pin_reset_codes: Some(Mutex::new(Vec::new())),
            pin_reset_notices: Some(Mutex::new(Vec::new())),
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

    /// The PIN reset codes a [`Mailer::recording`] kept, oldest first.
    pub fn pin_reset_code_mails(&self) -> Vec<AppCodeMail> {
        self.pin_reset_codes
            .as_ref()
            .map_or_else(Vec::new, |codes| codes.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// The notices of verified PIN resets a [`Mailer::recording`] kept,
    /// oldest first.
    pub fn pin_reset_notices(&self) -> Vec<Notice> {
        self.pin_reset_notices
            .as_ref()
            .map_or_else(Vec::new, |mails| mails.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// Tests only: a [`Mailer::recording`] fails every PIN reset code while
    /// `fail`.
    pub fn fail_pin_reset_codes(&self, fail: bool) {
        self.failing_pin_reset_codes.store(fail, Ordering::SeqCst);
    }

    /// Tests only: a [`Mailer::recording`] fails every PIN reset notice
    /// while `fail`.
    pub fn fail_pin_reset_notices(&self, fail: bool) {
        self.failing_pin_reset_notices.store(fail, Ordering::SeqCst);
    }

    /// Tests only: a [`Mailer::recording`] holds every PIN reset notice at
    /// its start while `hold`, as a mail service that does not answer.
    pub fn hold_pin_reset_notices(&self, hold: bool) {
        self.holding_pin_reset_notices.store(hold, Ordering::SeqCst);
    }

    /// How many PIN reset notices a [`Mailer::recording`] began to send,
    /// held or not.
    pub fn pin_reset_notices_begun(&self) -> usize {
        self.pin_reset_notices_begun.load(Ordering::SeqCst)
    }

    /// The new-device mails a [`Mailer::recording`] kept, oldest first.
    pub fn new_device_mails(&self) -> Vec<Notice> {
        self.new_devices
            .as_ref()
            .map_or_else(Vec::new, |mails| mails.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone())
    }

    /// How many new-device checks have finished, whether they mailed or
    /// not ([`crate::account::agents::mail_new_device`]).
    pub fn new_device_checks(&self) -> usize {
        self.new_device_checks.load(Ordering::SeqCst)
    }

    pub(crate) fn new_device_checked(&self) {
        self.new_device_checks.fetch_add(1, Ordering::SeqCst);
    }

    /// Tests only: a [`Mailer::recording`] holds every new-device task at
    /// its start while `hold`, so a test can act before the task runs.
    pub fn hold_new_device_mails(&self, hold: bool) {
        self.holding_new_devices.store(hold, Ordering::SeqCst);
    }

    /// Where a new-device task starts: it waits while a test holds it
    /// ([`Mailer::hold_new_device_mails`]); otherwise at once.
    pub(crate) async fn new_device_gate(&self) {
        while self.new_devices.is_some() && self.holding_new_devices.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Tests only: a [`Mailer::recording`] fails every new-device mail
    /// while `fail`.
    pub fn fail_new_device_mails(&self, fail: bool) {
        self.failing_new_devices.store(fail, Ordering::SeqCst);
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

    /// Tells an account that a new sign-in on `device` (fixed words, as
    /// [`crate::account::app_sign_in::Platform::device`] gives them)
    /// replaced the sign-in that device had, which was still live. Plain
    /// text with fixed words only; a [`Mailer::recording`] keeps it with
    /// the notices.
    pub async fn send_sign_in_replaced(&self, to: &str, device: &str) -> anyhow::Result<()> {
        let (subject, text) = sign_in_replaced_message(device, &self.site_origin);
        self.send_notice(to, &subject, &text).await
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

    /// Tells `to` that `device` (fixed words, such as "eBIRForms on
    /// iPhone") signed in to the account at `at`, and where its devices
    /// are listed (`devices_page`), as plain text. The third in 24 hours
    /// (`third_in_24_hours`), the most there are, says so and asks the
    /// person to check their devices.
    pub async fn send_new_device(
        &self,
        to: &str,
        device: &'static str,
        at: time::OffsetDateTime,
        devices_page: &str,
        third_in_24_hours: bool,
    ) -> anyhow::Result<()> {
        let (subject, text) = new_device_message(device, at, devices_page, third_in_24_hours);
        if let Some(mails) = &self.new_devices {
            if self.failing_new_devices.load(Ordering::SeqCst) {
                anyhow::bail!("made-up mail failure");
            }
            let mail = Notice { to: to.into(), subject, text };
            mails.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(mail);
            return Ok(());
        }
        match &self.config {
            MailConfig::Log => {
                tracing::info!(device, "new-device mail (MAIL_TRANSPORT=log)");
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

    /// The code for resetting an app's PIN, mailed to the account's own
    /// address `to`: the in-app sign-in code's message, with words for a
    /// PIN reset and the code its only number. `minutes`: how long it
    /// works (`APP_SIGN_IN_MINUTES`).
    pub async fn send_pin_reset_code(&self, to: &str, code: &str, minutes: i64) -> anyhow::Result<()> {
        let message = CodeMessage::pin_reset(code, minutes, &self.logo_url);
        if let Some(codes) = &self.pin_reset_codes {
            if self.failing_pin_reset_codes.load(Ordering::SeqCst) {
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
                tracing::info!(to, code, "PIN reset code (MAIL_TRANSPORT=log)");
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

    /// Tells the account at `to` that a PIN reset was verified on `device`
    /// at `at` ([`pin_reset_notice_message`]): plain text, no code and no
    /// link that acts. `Ok` only once the mail service took it.
    pub async fn send_pin_reset_notice(&self, to: &str, device: &str, at: time::OffsetDateTime) -> anyhow::Result<()> {
        let (subject, text) = pin_reset_notice_message(device, at, &self.site_origin);
        if let Some(mails) = &self.pin_reset_notices {
            self.pin_reset_notices_begun.fetch_add(1, Ordering::SeqCst);
            while self.holding_pin_reset_notices.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if self.failing_pin_reset_notices.load(Ordering::SeqCst) {
                anyhow::bail!("made-up mail failure");
            }
            let mail = Notice { to: to.into(), subject, text };
            mails.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(mail);
            return Ok(());
        }
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, device, "PIN reset notice (MAIL_TRANSPORT=log)");
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

/// The subject and text of the mail about a new device: fixed words, the
/// device's, and the time in the Philippines.
fn new_device_message(
    device: &str,
    at: time::OffsetDateTime,
    devices_page: &str,
    third_in_24_hours: bool,
) -> (String, String) {
    let when = philippine_time(at);
    (
        format!("New device signed in: {device}"),
        format!(
            "Your Buwiz account was signed in to {device} on {when}, Philippine time.\n\n\
             If this was you, there is nothing to do.\n\n\
             If this wasn't you, log it out at {devices_page}\n\n\
             {last}\
             Buwiz - https://buwiz.com",
            last = if third_in_24_hours {
                format!(
                    "We send at most three new-device notices in 24 hours, and this is the third. Check My \
                     devices at {devices_page} \
                     for any other device signed in to your account.\n\n"
                )
            } else {
                String::new()
            }
        ),
    )
}

/// `at` in the Philippines (Asia/Manila, UTC+8 all year), as "November
/// 25, 2026 at 3:04 PM".
fn philippine_time(at: time::OffsetDateTime) -> String {
    let at = at.to_offset(time::UtcOffset::from_hms(8, 0, 0).expect("valid offset"));
    let (hour, half) = match at.hour() {
        0 => (12, "AM"),
        hour @ 1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        hour => (hour - 12, "PM"),
    };
    format!("{} {}, {} at {hour}:{:02} {half}", at.month(), at.day(), at.year(), at.minute())
}

/// The subject and text of [`Mailer::send_pin_reset_notice`]: where and
/// when (Manila time, and UTC), and what to do if it was not the person.
/// The account site is named as plain text, its host only: the mail has no
/// link that acts and no code.
fn pin_reset_notice_message(device: &str, at: time::OffsetDateTime, site_origin: &str) -> (String, String) {
    let when = philippine_time(at);
    let utc = at.to_offset(time::UtcOffset::UTC);
    let utc = format!(
        "{}-{:02}-{:02} {:02}:{:02} UTC",
        utc.year(),
        u8::from(utc.month()),
        utc.day(),
        utc.hour(),
        utc.minute()
    );
    let site = site_origin.split_once("://").map_or(site_origin, |(_, host)| host).trim_end_matches('/');
    (
        "A PIN reset was asked for in eBIRForms".to_owned(),
        format!(
            "A PIN reset was asked for on {device} on {when}, Manila time ({utc}).\n\n\
             If this was you, there is nothing to do.\n\n\
             If it was not you: sign in at {site}, open My devices, and remove {device}. \
             Removing it signs it out and stops it from sending returns.\n\n\
             Buwiz"
        ),
    )
}

/// The subject and text of [`Mailer::send_sign_in_replaced`].
fn sign_in_replaced_message(device: &str, site_origin: &str) -> (String, String) {
    let subject = format!("New sign-in to {device}");
    let text = format!(
        "Your Buwiz account was just signed in to {device}, on a device where it was already signed in. \
         The earlier sign-in on that device was ended.\n\n\
         If this was you, there is nothing to do.\n\n\
         If it wasn't you, open My devices at {site_origin}/tins#devices-h and log that device out, \
         then sign in again on your own devices.\n\n\
         Buwiz"
    );
    (subject, text)
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
        let text = AppCodeText {
            subject: format!("Your Buwiz sign-in code for {device}"),
            heading,
            lead,
            asked,
            safe: "Nobody can get into your account without this code.",
            why: "Sent because this address was entered in the eBIRForms app.",
            minutes: words.minutes,
        };
        Self::app_words(code, text, logo_url)
    }

    /// The mail of a code for resetting an app's PIN: the app's code mail,
    /// with words for a PIN reset. Fixed words only: nothing from the
    /// request, not even the device's name.
    fn pin_reset(code: &str, minutes: i64, logo_url: &str) -> Self {
        let text = AppCodeText {
            subject: "Your eBIRForms PIN reset code".to_owned(),
            heading: "Reset your eBIRForms PIN",
            lead: "Enter this verification code in eBIRForms to reset your PIN.".to_owned(),
            asked: "Someone asked to reset the eBIRForms PIN on a device signed in to your Buwiz account. \
                    If it wasn't you, ignore this mail."
                .to_owned(),
            safe: "Nobody can reset the PIN without this code.",
            why: "Sent because a PIN reset was asked for in the eBIRForms app.",
            minutes,
        };
        Self::app_words(code, text, logo_url)
    }

    /// An app's code mail in `words`. The code is its only number.
    fn app_words(code: &str, words: AppCodeText, logo_url: &str) -> Self {
        let AppCodeText { subject, heading, lead, asked, safe, why, minutes } = words;
        let minutes = minutes_in_words(minutes);
        Self {
            text: format!(
                "Your Buwiz verification code is {code}\n\n\
                 {lead} It expires in {minutes} and works once.\n\n\
                 Never give this code to anyone. Buwiz will never ask for it by phone, chat or email, \
                 not even someone who says they are from Buwiz or the BIR.\n\n\
                 {asked} {safe}\n\n\
                 Buwiz - https://buwiz.com"
            ),
            html: APP_SIGN_IN_CODE_HTML
                .replace("{{subject}}", &subject)
                .replace("{{heading}}", heading)
                .replace("{{lead}}", &lead)
                .replace("{{asked}}", &asked)
                .replace("{{minutes}}", &minutes)
                .replace("{{why}}", why)
                .replace("{{code}}", code)
                .replace("{{logo_url}}", logo_url),
            subject,
        }
    }
}

/// What an app's code mail says around the code ([`CodeMessage::app_words`]).
struct AppCodeText {
    subject: String,
    heading: &'static str,
    lead: String,
    asked: String,
    /// The text's last words before the sign-off.
    safe: &'static str,
    /// The footer's line: why the mail was sent.
    why: &'static str,
    minutes: i64,
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
    fn an_app_code_mail_has_the_code_and_no_link_or_button() {
        let words = AppCodeWords { device: "eBIRForms on Mac", create: false, minutes: 5 };
        let message = CodeMessage::app("042917", words, "https://account.buwiz.com/x.png");
        assert!(
            message.text.contains("Enter this verification code in eBIRForms on Mac to sign in."),
            "{}",
            message.text
        );
        assert!(message.text.contains("Your Buwiz verification code is 042917"), "{}", message.text);
        assert!(message.text.contains("It expires in five minutes and works once."), "{}", message.text);
        assert!(message.text.contains("If it wasn't you, ignore this mail."), "{}", message.text);
        let seen = visible(&message.html);
        assert!(seen.contains("Enter this verification code in eBIRForms on Mac to sign in."), "{seen}");
        assert!(seen.contains("042917") && seen.contains("Expires in five minutes"), "{seen}");
        for part in [message.text.as_str(), message.html.as_str()] {
            assert!(!part.contains("app-sign-in") && !part.contains("ebirforms://"), "{part}");
            assert!(!part.contains("Open in eBIRForms") && !part.contains("open this link"), "{part}");
        }
        // The footer's buwiz.com is the mail's only link.
        assert_eq!(message.html.matches("<a ").count(), 1, "{}", message.html);

        let web = CodeMessage::new("042917", "https://account.buwiz.com/x.png");
        for part in [&web.subject, &web.text, &web.html] {
            assert!(!part.contains("app-sign-in") && !part.contains("eBIRForms"), "{part}");
        }
    }

    #[test]
    fn the_replaced_sign_in_mail_has_fixed_words_and_the_devices_link() {
        let (subject, text) = sign_in_replaced_message("eBIRForms on iPhone", "https://account.buwiz.com");
        assert_eq!(subject, "New sign-in to eBIRForms on iPhone");
        assert!(text.contains("The earlier sign-in on that device was ended."), "{text}");
        assert!(text.contains("https://account.buwiz.com/tins#devices-h"), "{text}");
    }

    #[test]
    fn the_new_device_mail_says_where_and_when_in_philippine_time() {
        let at = time::OffsetDateTime::from_unix_timestamp(1_795_590_240).unwrap();
        let (subject, text) = new_device_message("eBIRForms on iPhone", at, "https://account.buwiz.com/tins", false);
        assert_eq!(subject, "New device signed in: eBIRForms on iPhone");
        let (replaced, _) = sign_in_replaced_message("eBIRForms on iPhone", "https://account.buwiz.com");
        assert_ne!(subject, replaced, "the two mails are told apart by their subjects");
        assert!(
            text.contains("signed in to eBIRForms on iPhone on November 25, 2026 at 3:04 PM, Philippine time"),
            "{text}"
        );
        assert!(text.contains("If this wasn't you, log it out at https://account.buwiz.com/tins\n"), "{text}");
        let (_, midnight) = new_device_message(
            "eBIRForms on Mac",
            time::OffsetDateTime::from_unix_timestamp(1_795_624_200).unwrap(),
            "x",
            false,
        );
        assert!(midnight.contains("November 26, 2026 at 12:30 AM"), "{midnight}");
        assert!(!text.contains("this is the third"), "{text}");
        assert!(!text.contains("today"), "{text}");
        let (_, last) = new_device_message("eBIRForms on iPhone", at, "https://account.buwiz.com/tins", true);
        assert!(
            last.contains(
                "We send at most three new-device notices in 24 hours, and this is the third. Check My devices \
                 at https://account.buwiz.com/tins "
            ),
            "{last}"
        );
        assert!(!last.contains("today"), "{last}");
    }

    #[test]
    fn a_pin_reset_code_mail_has_the_code_only_and_its_own_words() {
        let message = CodeMessage::pin_reset("042917", 5, "https://account.buwiz.com/x.png");
        assert_eq!(message.subject, "Your eBIRForms PIN reset code");
        assert!(
            message.text.contains("Enter this verification code in eBIRForms to reset your PIN."),
            "{}",
            message.text
        );
        assert!(message.text.contains("Nobody can reset the PIN without this code."), "{}", message.text);
        assert!(!message.text.contains("sign in") && !message.html.contains("sign-in code"), "{}", message.text);
        let seen = visible(&message.html);
        assert!(seen.contains("Reset your eBIRForms PIN") && seen.contains("Expires in five minutes"), "{seen}");
        assert!(seen.contains("Sent because a PIN reset was asked for in the eBIRForms app."), "{seen}");
        assert!(message.html.contains("<title>Your eBIRForms PIN reset code</title>"));
        assert!(!message.html.contains("{{"));
        for part in [message.text.as_str(), seen.as_str()] {
            assert!(numbers(part).iter().all(|number| *number == "042917"), "{part}");
        }
        assert_eq!(message.html.matches("<a ").count(), 1, "{}", message.html);

        // The sign-in mail keeps its own words.
        let words = AppCodeWords { device: "eBIRForms on Mac", create: false, minutes: 5 };
        let sign_in = CodeMessage::app("042917", words, "https://account.buwiz.com/x.png");
        assert!(sign_in.html.contains("<title>Your Buwiz sign-in code for eBIRForms on Mac</title>"));
        assert!(visible(&sign_in.html).contains("Sent because this address was entered in the eBIRForms app."));
        assert!(sign_in.text.contains("Nobody can get into your account without this code."), "{}", sign_in.text);
    }

    #[test]
    fn the_pin_reset_notice_says_where_and_when_with_no_link_or_code() {
        let at = time::OffsetDateTime::from_unix_timestamp(1_795_590_240).unwrap();
        let (subject, text) = pin_reset_notice_message("Ana's iPhone", at, "https://account.buwiz.com");
        assert_eq!(subject, "A PIN reset was asked for in eBIRForms");
        assert_eq!(
            text,
            "A PIN reset was asked for on Ana's iPhone on November 25, 2026 at 3:04 PM, Manila time \
             (2026-11-25 07:04 UTC).\n\n\
             If this was you, there is nothing to do.\n\n\
             If it was not you: sign in at account.buwiz.com, open My devices, and remove Ana's iPhone. \
             Removing it signs it out and stops it from sending returns.\n\n\
             Buwiz"
        );
        assert!(numbers(&text).iter().all(|number| number.len() < 6), "{text}");
        assert!(!text.contains("://"), "{text}");
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
