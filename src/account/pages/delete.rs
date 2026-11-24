//! Deleting one's own account (`/delete`, docs/accounts.md, "Deleting an
//! account"); the eBIRForms app's Settings > Account > Delete account opens
//! it. It says what goes and what stays, then mails a code; the posts are
//! [`crate::account::deletion`]'s. A banned account gets here too, so the
//! session is read banned or not ([`deletion_account`]), and is told to
//! write to support.

use super::*;
use crate::account::deletion;
use crate::account::web::{DELETE_PAGE, deletion_account};

pub(super) const TITLE: &str = "Delete your account · Buwiz";

#[page("/delete")]
pub async fn delete_page(cx: &Cx) -> Result<impl View> {
    let state = state(cx);
    let Some(account) = deletion_account(state, headers(cx)).await.map_err(fail)? else {
        return Err(redirect(link("/login", Some(Notice::SignInFirst), &[("next", DELETE_PAGE)])).into());
    };
    let mut db = state.db.acquire().await.map_err(fail)?;
    let refused =
        deletion::refusal(&mut db, &state.config.admin_emails, account.id, &account.email).await.map_err(fail)?;
    let shown = notice(cx).filter(|shown| Some(*shown) != refused);
    // After a code was sent (or a wrong one typed), the page asks for it.
    let asking = matches!(shown, Some(Notice::DeleteCodeSent | Notice::DeleteCodeInvalid));
    Ok(view! {
        <main class="home auth">
            <section class="auth-card consent delete-account">
                auth_brand()
                <h1>"Delete your account"</h1>
                <p class="consent-who">"Signed in as " <strong>(account.email.clone())</strong></p>
                auth_notice(notice: shown)
                <p class="auth-lead">"Deleting your Buwiz account can't be undone. You can make a new account with the same address later, but nothing below comes back."</p>
                <div class="consent-lists delete-lists">
                    <div class="consent-cannot">
                        <p class="consent-head">icon(name: "remove_circle") "Deleted now"</p>
                        <ul>
                            <li>"Your email address and your account."</li>
                            <li>"Every device and sign-in. The Buwiz apps and any AI agent are signed out."</li>
                            <li>"Every TIN on your account, with the files you sent for it and what was read from them."</li>
                            <li>"The records of every return you sent through Buwiz, with their receipts and the send permissions (tickets) issued for them."</li>
                            <li>"Your place in the BIR queue, and the records of BIR outages kept for you, proofs of outage included."</li>
                            <li>"The records of the sign-in codes mailed to you."</li>
                        </ul>
                    </div>
                    <div class="consent-can">
                        <p class="consent-head">icon(name: "lock") "Kept, and why"</p>
                        <ul>
                            <li>"Staff audit lines, so we can answer for what staff did: they name a TIN only with its middle digits hidden, and your account only by an id number, with a line that it was deleted. If staff made your login with their setup tool, its record of that run keeps your address."</li>
                            <li>"BIR uptime records for 90 days. They hold only scrambled codes that change every day, never your account or TIN."</li>
                            <li>"Our server logs, which carry ids only."</li>
                            <li>"Our database provider's backups, until they expire after their short window."</li>
                        </ul>
                    </div>
                </div>
                <p class="auth-hint">icon(name: "receipt_long") "Returns the BIR already received stay with the BIR. Deleting your account does not withdraw them."</p>
                <p class="auth-hint">icon(name: "desktop_windows") "Nothing stored in the app on your own computer is touched. Remove the app's data there if you want it gone."</p>
                if let Some(refused) = refused {
                    <p class="auth-notice error" role="status">icon(name: "error") <span>(refused.message())</span></p>
                    <a class="btn btn-ghost auth-submit" href="/tins">"Back to My TINs"</a>
                } else if asking {
                    <form
                        class="auth-form"
                        method="post"
                        action="/delete/confirm"
                        data-confirm="Delete your account for good?"
                        data-confirm-detail="Your TINs, files, returns and sign-ins are deleted now. This can't be undone."
                        data-confirm-label="Delete my account"
                        data-confirm-tone="danger"
                    >
                        <label class="auth-label" for="code">"Code from the email"</label>
                        <input
                            id="code"
                            class="auth-code"
                            type="text"
                            name="code"
                            inputmode="numeric"
                            autocomplete="one-time-code"
                            maxlength="7"
                            placeholder="000000"
                            required=(true)
                            autofocus=(true)
                        >
                        <p class="auth-hint">icon(name: "timer") "The code works for ten minutes, once. Only the newest one works."</p>
                        <button class="btn btn-danger auth-submit" type="submit">"Delete my account"</button>
                    </form>
                    <form class="auth-resend" method="post" action="/delete/code">
                        <span>"No code? Check spam, or"</span>
                        <button class="auth-link" type="submit">"send a new one"</button>
                    </form>
                } else {
                    <form class="auth-form" method="post" action="/delete/code">
                        <p class="auth-hint">icon(name: "mail") "To be sure it is you, we email a 6-digit code to " (account.email) ". A code you got to sign in does not work here."</p>
                        <button class="btn btn-danger auth-submit" type="submit">"Email me a code"</button>
                    </form>
                }
                <p class="auth-switch"><a href="/tins">"Keep my account"</a></p>
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
        </main>
    })
}
