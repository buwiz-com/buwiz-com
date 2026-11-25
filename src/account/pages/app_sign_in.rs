//! `/app-sign-in`, where the link in an app's sign-in mail goes
//! (docs/accounts.md, "The link in the mail"). The request and its link
//! token are in the fragment (`#r=…&t=…`), which never reaches the server.
//! The page's script (`/assets/app-sign-in.js`) turns them into an "Open
//! eBIRForms" button for `ebirforms://signin?r=…&t=…`. It opens nothing on
//! its own, approves nothing and calls no endpoint: only the app that
//! asked holds the secret that makes the token work.

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
                <p class="auth-lead">"Open this on the device where you asked to sign in, or type the code there."</p>
                <div class="auth-actions">
                    <a id="open-app" class="btn btn-solid" href="#" hidden=(true)>"Open eBIRForms"</a>
                </div>
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
            <script src="/assets/app-sign-in.js" defer=(true)></script>
        </main>
    })
}
