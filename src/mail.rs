//! Sends sign-in codes.

use anyhow::Context;
use serde_json::json;

use crate::config::MailConfig;

pub struct Mailer {
    config: MailConfig,
    http: reqwest::Client,
}

impl Mailer {
    pub fn new(config: MailConfig) -> Self {
        Self { config, http: reqwest::Client::new() }
    }

    pub async fn send_sign_in_code(&self, to: &str, code: &str) -> anyhow::Result<()> {
        let subject = format!("Your Buwiz sign-in code: {code}");
        let text = format!(
            "Your Buwiz sign-in code is {code}.\n\nIt works for 10 minutes. If you did not try to sign in, ignore this email."
        );
        match &self.config {
            MailConfig::Log => {
                tracing::info!(to, code, "sign-in code (MAIL_TRANSPORT=log)");
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
}
