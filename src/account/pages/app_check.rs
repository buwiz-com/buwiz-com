//! `/app-check` and `/app-check/done` on the account host: the app check
//! (Turnstile) for in-app sign-in (docs/accounts.md, "The app-check
//! page"). An app opens `/app-check` in a web view; when the widget
//! passes, the page hands the token to the app (`window.nsh.postMessage`
//! first, then the other ways in `/assets/app-check.js`) and goes on to
//! `/app-check/done#token=…`, where a host that saw none of that reads it
//! from the address. Neither page needs a sign-in or reads anything from
//! the request, and neither links off the account host: the page has no
//! site header.

use http::header;

use super::*;
use crate::sites::canonical::Site;

pub(super) const TITLE: &str = "Checking this app · Buwiz";
pub(super) const DONE_TITLE: &str = "Done · Buwiz";

/// A 404 unless on the account host.
fn account_host(cx: &Cx) -> Result<()> {
    let host = headers(cx).get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if Site::for_host(host) != Site::Account {
        return Err(topcoat::router::error::not_found().into());
    }
    Ok(())
}

#[page("/app-check")]
pub async fn app_check_page(cx: &Cx) -> Result<impl View> {
    account_host(cx)?;
    let site_key = state(cx).sign_in_guard.site_key().map(str::to_owned);
    Ok(view! {
        <main class="home auth">
            <section class="auth-card">
                <span class="auth-brand">brand_wordmark(height: 32)</span>
                <h1>"Checking this app"</h1>
                if let Some(site_key) = site_key {
                    <p class="auth-lead">"This takes a moment. If a box appears, tick it."</p>
                    <div id="app-check" data-state="ready">
                        <div
                            class="cf-turnstile"
                            data-sitekey=(site_key)
                            data-action="app-check"
                            data-callback="buwizTurnstileDone"
                            data-error-callback="buwizTurnstileError"
                            data-expired-callback="buwizTurnstileExpired"
                            data-timeout-callback="buwizTurnstileTimeout"
                        ></div>
                    </div>
                    // Not deferred: the widget calls its functions.
                    <script src="/assets/app-check.js"></script>
                    <script src=(guard::WIDGET_SCRIPT) async=(true) defer=(true)></script>
                    <noscript><p class="auth-notice error">"This check needs JavaScript."</p></noscript>
                } else {
                    <p class="auth-lead">"The app check is not set up on this server (no Turnstile keys)."</p>
                    <div id="app-check" data-state="not-configured"></div>
                    <script src="/assets/app-check.js"></script>
                }
            </section>
        </main>
    })
}

#[page("/app-check/done")]
pub async fn app_check_done_page(cx: &Cx) -> Result<impl View> {
    account_host(cx)?;
    Ok(view! {
        <main class="home auth">
            <section class="auth-card">
                <span class="auth-brand">brand_wordmark(height: 32)</span>
                <h1>"You can close this window"</h1>
                <p class="auth-lead">"Go back to eBIRForms to go on."</p>
            </section>
        </main>
    })
}
