//! `/app-sign-in`, where the link in older app sign-in mails goes. Mails
//! no longer carry a link (docs/accounts.md, "In-app sign-in"), but those
//! already sent still point here, so the page stays: it says to type the
//! code from the email into the app. It has no button and no script,
//! approves nothing and calls no endpoint.

use http::header;

use super::*;
use crate::sites::canonical::Site;

pub(super) const TITLE: &str = "Sign in to eBIRForms · Buwiz";

#[page("/app-sign-in")]
pub async fn app_sign_in_page(cx: &Cx) -> Result<impl View> {
    let host = headers(cx).get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if Site::for_host(host) != Site::Account {
        return Err(topcoat::router::error::not_found().into());
    }
    Ok(view! {
        <main class="home auth">
            <section class="auth-card">
                auth_brand()
                <span class="auth-badge">icon(name: "devices")</span>
                <h1>"Sign in to eBIRForms"</h1>
                <p class="auth-lead">"Type the 6-digit code from the email into the app."</p>
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
        </main>
    })
}
