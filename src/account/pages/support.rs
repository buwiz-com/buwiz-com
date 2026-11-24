//! The filing site's help pages for the app stores: `/support` (the
//! listings' support address) and `/delete-account` (their account
//! deletion address). Both are public and need no sign-in; the deletion
//! itself is on the account site, signed in (`account.buwiz.com/delete`).
//! What `/delete-account` says is what that page does
//! (docs/accounts.md, "Deleting an account"): change them together.
//! Styles are the privacy page's (`.policy`, `src/sites/home.css`).

use http::header;

use super::home::footer;
use super::*;
use crate::sites::canonical::Site;

pub(super) const SUPPORT_TITLE: &str = "Support for eBIRForms · Buwiz";
pub(super) const SUPPORT_DESCRIPTION: &str =
    "How to reach Buwiz support for the eBIRForms app, and where to check the BIR's status.";
pub(super) const DELETE_TITLE: &str = "Delete your Buwiz account · Buwiz";
pub(super) const DELETE_DESCRIPTION: &str =
    "How to delete the Buwiz account you use with eBIRForms, and what is removed and what is kept.";

/// The address the app and the listings send people to, to delete their
/// account: the account site's signed-in page.
const DELETE_PAGE: &str = "https://account.buwiz.com/delete";

/// `Err(404)` unless the request is for the filing site: buwiz.com is the
/// Bot site's, and the account and API hosts have no public pages.
fn forms_only(cx: &Cx) -> Result<()> {
    let host = headers(cx).get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if Site::for_host(host) != Site::Forms {
        return Err(topcoat::router::error::not_found().into());
    }
    Ok(())
}

#[page("/support")]
pub async fn support_page(cx: &Cx) -> Result<impl View> {
    forms_only(cx)?;
    let bir_status = state(cx).uptime.level.is_public();
    Ok(view! {
        <main class="home landing">
            <section class="section policy-page">
                <article class="policy">
                    <h1>"Support for eBIRForms"</h1>
                    <p>
                        "Write to "
                        <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                        "."
                    </p>

                    <h2>"What to put in your message"</h2>
                    <ul>
                        <li>"Write from the email address of your Buwiz account, so we can find it."</li>
                        <li>"Your computer's system (macOS, Windows or Linux) and the app's version, if you know it."</li>
                        <li>"What you did, what you expected, and what happened instead. A screenshot helps."</li>
                        <li>"Never send a password, a sign-in code or a recovery key. We never ask for one."</li>
                    </ul>
                    <p>
                        "A TIN, the form and the period are fine to include when the question is about one return."
                    </p>

                    <h2>"Before you write"</h2>
                    <ul>
                        if bir_status {
                            <li>
                                "Returns not going through? The "
                                <a href="/bir-status">"BIR status page"</a>
                                " says whether the BIR's servers are up right now."
                            </li>
                        }
                        <li>
                            "A TIN waiting for review: papers are usually checked within one business day. "
                            <a href="https://account.buwiz.com/tins">"My TINs"</a>
                            " shows where each one stands."
                        </li>
                        <li>
                            "To delete your account, see "
                            <a href="/delete-account">"Delete your Buwiz account"</a>
                            "."
                        </li>
                        <li>
                            "What the app stores and sends: the "
                            <a href="/privacy">"privacy policy"</a>
                            "."
                        </li>
                    </ul>

                    <h2>"Who we are"</h2>
                    <p>
                        "eBIRForms is published by Goldcoders Corp. The app's website is "
                        <a href="https://ebirforms.buwiz.com">"https://ebirforms.buwiz.com"</a>
                        "."
                    </p>
                </article>
            </section>
            footer()
        </main>
    })
}

#[page("/delete-account")]
pub async fn delete_account_page(cx: &Cx) -> Result<impl View> {
    forms_only(cx)?;
    Ok(view! {
        <main class="home landing">
            <section class="section policy-page">
                <article class="policy">
                    <h1>"Delete your Buwiz account"</h1>
                    <p>
                        "You can delete the Buwiz account you use with eBIRForms at any time, yourself. It takes a minute and cannot be undone."
                    </p>

                    <h2>"How"</h2>
                    <ol>
                        <li>
                            "In the app (from version 0.2.1), open Settings > Account > Delete account. Or open "
                            <a href=(DELETE_PAGE)>"account.buwiz.com/delete"</a>
                            " in any browser."
                        </li>
                        <li>"Sign in with your email address, if you are not signed in already."</li>
                        <li>"Read what will be removed, then ask for a confirmation code. We email a six-digit code to your address."</li>
                        <li>"Type the code and confirm. The account is deleted at once, and the app is signed out on every computer."</li>
                    </ol>

                    <h2>"What is removed"</h2>
                    <ul>
                        <li>"Your account and its email address."</li>
                        <li>"Every computer and sign-in connected to it."</li>
                        <li>"Every TIN on the account, with the documents you sent to verify it and what we read from them."</li>
                        <li>"The records of the returns you sent through Buwiz, with their receipts."</li>
                        <li>"Our records of the sign-in codes we emailed you."</li>
                    </ul>

                    <h2>"What is kept, and why"</h2>
                    <ul>
                        <li>"Our staff's audit log keeps its lines about your account and TINs, naming the account by an id only and a TIN only masked (for example ***-***-789-00000), so we can answer for what our staff did."</li>
                        <li>"Records of the BIR's uptime keep, for 90 days, a code that cannot be traced back to your account."</li>
                        <li>"Our server's logs, which name accounts by id only, are kept for 30 days."</li>
                        <li>"Our database provider's backups hold a copy for their limited time, then it is gone."</li>
                    </ul>
                    <p>
                        "Deleting your account does not withdraw returns the BIR has already received, and it does not touch what the app stores on your computer. To remove that, delete the app's folder, as the "
                        <a href="/privacy">"privacy policy"</a>
                        " explains."
                    </p>

                    <h2>"When it cannot be done right away"</h2>
                    <ul>
                        <li>"A return is being sent right now: wait a few minutes, then try again."</li>
                        <li>"The account belongs to a member of our staff: ask an Admin to remove the staff role first."</li>
                        <li>
                            "If the page says to write to us, or you cannot sign in, write to "
                            <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                            " from the account's email address and we will delete it for you."
                        </li>
                    </ul>
                </article>
            </section>
            footer()
        </main>
    })
}
