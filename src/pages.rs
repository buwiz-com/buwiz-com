//! The account site, rendered with Topcoat. Pages only read; every change
//! goes through a form post handled in [`crate::web`].

use std::fmt::Display;

use topcoat::Result;
use topcoat::context::{Cx, app_context};
use topcoat::router::error::redirect;
use topcoat::router::request::{headers, uri};
use topcoat::router::{Router, Slot, layout, page};
use topcoat::view::{View, component, view};
use uuid::Uuid;

use crate::accounts::Account;
use crate::claims::{self, ClaimRow, DocumentKind};
use crate::notice::Notice;
use crate::oauth;
use crate::state::AppState;
use crate::web::{link, safe_next, session_account};

pub fn router(state: AppState) -> Router {
    Router::builder()
        .app_context(state)
        .layout(root_layout)
        .page(home_page)
        .page(login_page)
        .page(signup_page)
        .page(verify_page)
        .page(device_page)
        .page(account_page)
        .page(new_tin_page)
        .page(admin_claims_page)
        .page(claim_id::admin_claim_page)
        .build()
}

fn state(cx: &Cx) -> &AppState {
    app_context::<AppState>(cx)
}

fn fail(error: impl Display) -> topcoat::Error {
    std::io::Error::other(error.to_string()).into()
}

/// A query parameter of the current request.
fn query(cx: &Cx, key: &str) -> Option<String> {
    let raw = uri(cx).query()?;
    form_urlencoded::parse(raw.as_bytes()).find(|(name, _)| name == key).map(|(_, value)| value.into_owned())
}

fn notice(cx: &Cx) -> Option<Notice> {
    query(cx, "notice").and_then(|code| Notice::from_code(&code))
}

async fn current(cx: &Cx) -> Result<Option<Account>> {
    session_account(state(cx), headers(cx)).await.map_err(fail)
}

/// The signed-in account, or a redirect to sign in and return here.
async fn signed_in(cx: &Cx) -> Result<Account> {
    match current(cx).await? {
        Some(account) => Ok(account),
        None => {
            let here = uri(cx).path_and_query().map_or("/account", |path| path.as_str()).to_owned();
            Err(redirect(link("/login", Some(Notice::SignInFirst), &[("next", &here)])).into())
        }
    }
}

#[layout("/")]
async fn root_layout(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let account = current(cx).await?;
    let admin = account.as_ref().is_some_and(|account| state(cx).config.is_admin(&account.email));
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Buwiz"</title>
                <link rel="stylesheet" href="/assets/site.css">
            </head>
            <body>
                <header class="site">
                    <a class="brand" href="/">"Buwiz"</a>
                    <nav>
                        if let Some(account) = &account {
                            <a href="/account">"My TINs"</a>
                            if admin {
                                <a href="/admin/claims">"Review claims"</a>
                            }
                            <span class="muted">(account.email.clone())</span>
                            <form method="post" action="/logout">
                                <button class="link" type="submit">"Sign out"</button>
                            </form>
                        } else {
                            <a href="/login">"Sign in"</a>
                            <a class="button" href="/signup">"Create account"</a>
                        }
                    </nav>
                </header>
                (slot)
            </body>
        </html>
    })
}

#[component]
async fn notice_banner(notice: Option<Notice>) -> Result<impl View> {
    Ok(view! {
        if let Some(notice) = notice {
            <p class=(if notice.is_error() { "notice error" } else { "notice" }) role="status">(notice.message())</p>
        }
    })
}

#[page("/")]
async fn home_page() -> Result<impl View> {
    Ok(view! {
        <main>
            <section class="hero">
                <h1>"File BIR returns without the queue."</h1>
                <p class="lead">
                    "Buwiz prepares, checks and sends your BIR returns from your own computer. "
                    "Add your TIN once, and every filing after that takes minutes."
                </p>
                <div class="actions">
                    <a class="button" href="/signup">"Create a free account"</a>
                    <a class="button secondary" href="/login">"Sign in"</a>
                </div>
            </section>
            <h2>"How it works"</h2>
            <div class="steps">
                <div class="card">
                    <strong>"1. Create an account"</strong>
                    <p class="muted">"Sign in with a code sent to your email. No password to remember."</p>
                </div>
                <div class="card">
                    <strong>"2. Activate your TIN"</strong>
                    <p class="muted">"Upload your BIR Form 2303 and a government ID. We check that the TIN is yours."</p>
                </div>
                <div class="card">
                    <strong>"3. File from the Buwiz app"</strong>
                    <p class="muted">"Fill in the form, see penalties before they happen, and send it to BIR."</p>
                </div>
            </div>
        </main>
    })
}

#[component]
async fn email_form(cx: &Cx, title: &'static str, intro: &'static str) -> Result<impl View> {
    let next = safe_next(query(cx, "next").as_deref());
    let email = query(cx, "email").unwrap_or_default();
    Ok(view! {
        <main>
            <h1>(title)</h1>
            <p class="lead">(intro)</p>
            notice_banner(notice: notice(cx))
            <form class="card" method="post" action="/auth/email/start">
                <input type="hidden" name="next" value=(next)>
                <label for="email">"Email address"</label>
                <input id="email" type="email" name="email" value=(email) autocomplete="email" required=(true) autofocus=(true)>
                <div class="actions">
                    <button type="submit">"Email me a code"</button>
                </div>
            </form>
        </main>
    })
}

#[page("/login")]
async fn login_page(cx: &Cx) -> Result<impl View> {
    if current(cx).await?.is_some() {
        return Err(redirect(safe_next(query(cx, "next").as_deref())).into());
    }
    Ok(view! {
        email_form(title: "Sign in to Buwiz", intro: "We will email you a 6-digit code.")
    })
}

#[page("/signup")]
async fn signup_page(cx: &Cx) -> Result<impl View> {
    if current(cx).await?.is_some() {
        return Err(redirect(safe_next(query(cx, "next").as_deref())).into());
    }
    Ok(view! {
        email_form(
            title: "Create your Buwiz account",
            intro: "Enter your email. We will send a code to confirm it; that is all you need."
        )
    })
}

#[page("/login/verify")]
async fn verify_page(cx: &Cx) -> Result<impl View> {
    let next = safe_next(query(cx, "next").as_deref());
    let email = query(cx, "email").unwrap_or_default();
    let restart = link("/login", None, &[("next", &next), ("email", &email)]);
    Ok(view! {
        <main>
            <h1>"Enter your code"</h1>
            <p class="lead">"We sent a 6-digit code to " <strong>(email.clone())</strong> "."</p>
            notice_banner(notice: notice(cx))
            <form class="card" method="post" action="/auth/email/verify">
                <input type="hidden" name="next" value=(next)>
                <input type="hidden" name="email" value=(email)>
                <label for="code">"Code"</label>
                <input id="code" type="text" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="7" required=(true) autofocus=(true)>
                <div class="actions">
                    <button type="submit">"Sign in"</button>
                    <a class="button secondary" href=(restart)>"Use a different email"</a>
                </div>
            </form>
        </main>
    })
}

/// Approves a sign-in from a Buwiz app (RFC 8628 device flow).
#[page("/device")]
async fn device_page(cx: &Cx) -> Result<impl View> {
    let shown = notice(cx);
    // A finished decision shows only its notice, and needs no session.
    let finished = matches!(shown, Some(Notice::DeviceApproved | Notice::DeviceDenied));
    let raw_code = query(cx, "user_code").unwrap_or_default();
    let mut request = None;
    let mut email = String::new();
    if !finished {
        email = signed_in(cx).await?.email;
        if let Some(code) = oauth::normalize_user_code(&raw_code)
            && let Some(client) = oauth::pending_device(&state(cx).db, &code).await?
        {
            request = oauth::client_name(&client).map(|name| (code, name));
        }
    }
    let bad_code = !finished && request.is_none() && !raw_code.is_empty();
    Ok(view! {
        <main>
            <h1>"Sign in to the app"</h1>
            notice_banner(notice: shown)
            if bad_code {
                notice_banner(notice: Some(Notice::DeviceInvalid))
            }
            if finished {
            } else if let Some((code, name)) = request {
                <div class="card">
                    <p>"Check that this code matches the one shown in " <strong>(name)</strong> ":"</p>
                    <p class="code">(code.clone())</p>
                    <p class="muted">"Approve only if you started this sign-in yourself. The app will act as " (email) "."</p>
                    <form method="post" action="/device/decision">
                        <input type="hidden" name="user_code" value=(code)>
                        <div class="actions">
                            <button type="submit" name="decision" value="approve">"Approve"</button>
                            <button class="secondary" type="submit" name="decision" value="deny">"Decline"</button>
                        </div>
                    </form>
                </div>
            } else {
                <form class="card" method="get" action="/device">
                    <label for="user_code">"Code shown in the app"</label>
                    <input id="user_code" type="text" name="user_code" placeholder="ABCD-EFGH" autocomplete="off" required=(true) autofocus=(true)>
                    <div class="actions">
                        <button type="submit">"Continue"</button>
                    </div>
                </form>
            }
        </main>
    })
}

fn status_label(status: &str) -> &'static str {
    match status {
        "pending" => "Waiting for review",
        "approved" => "Active",
        "rejected" => "Not approved",
        "withdrawn" => "Withdrawn",
        _ => "Unknown",
    }
}

fn date(at: time::OffsetDateTime) -> String {
    let date = at.date();
    format!("{} {}, {}", date.month(), date.day(), date.year())
}

#[page("/account")]
async fn account_page(cx: &Cx) -> Result<impl View> {
    let account = signed_in(cx).await?;
    let claims = claims::for_account(&state(cx).db, account.id).await?;
    Ok(view! {
        <main>
            <h1>"My TINs"</h1>
            <p class="lead">"A TIN becomes active once we have checked your Form 2303. Active TINs unlock filing in the Buwiz app."</p>
            notice_banner(notice: notice(cx))
            <div class="actions">
                <a class="button" href="/tins/new">"Add a TIN"</a>
            </div>
            if claims.is_empty() {
                <p class="card muted">"No TINs yet. Add one to start filing."</p>
            } else {
                <div class="card">
                    <table>
                        <thead>
                            <tr><th>"TIN"</th><th>"Registered name"</th><th>"Sent"</th><th>"Status"</th></tr>
                        </thead>
                        <tbody>
                            for claim in claims {
                                claim_row(claim: claim)
                            }
                        </tbody>
                    </table>
                </div>
            }
        </main>
    })
}

#[component]
async fn claim_row(claim: ClaimRow) -> Result<impl View> {
    let withdraw = format!("/tins/claims/{}/withdraw", claim.id);
    Ok(view! {
        <tr>
            <td>(claim.tin().to_string())</td>
            <td>(claim.registered_name.clone())</td>
            <td>(date(claim.created_at))</td>
            <td>
                <span class=(format!("status {}", claim.status))>(status_label(&claim.status))</span>
                if let Some(note) = claim.decision_note.clone() {
                    <p class="hint">(note)</p>
                }
                if claim.status == "pending" {
                    <form method="post" action=(withdraw)>
                        <button class="link" type="submit">"Withdraw"</button>
                    </form>
                }
            </td>
        </tr>
    })
}

#[page("/tins/new")]
async fn new_tin_page(cx: &Cx) -> Result<impl View> {
    signed_in(cx).await?;
    Ok(view! {
        <main>
            <h1>"Add a TIN"</h1>
            <p class="lead">"Send us your BIR Form 2303 (Certificate of Registration) and a government ID. A person checks them, usually within one business day."</p>
            notice_banner(notice: notice(cx))
            <form class="card" method="post" action="/tins/claims" enctype="multipart/form-data">
                <label for="tin">"TIN" <span class="hint">" (with branch code if not head office, e.g. 123-456-789-00000)"</span></label>
                <input id="tin" type="text" name="tin" inputmode="numeric" placeholder="123-456-789-00000" required=(true)>
                <label for="registered_name">"Registered name" <span class="hint">" (exactly as on Form 2303)"</span></label>
                <input id="registered_name" type="text" name="registered_name" maxlength="200" required=(true)>
                <label for="form_2303">"BIR Form 2303" <span class="hint">" (PDF or photo, up to 10 MB)"</span></label>
                <input id="form_2303" type="file" name="form_2303" accept="application/pdf,image/*" required=(true)>
                <label for="government_id">"Government ID" <span class="hint">" (passport, driver's license, UMID, PhilSys…)"</span></label>
                <input id="government_id" type="file" name="government_id" accept="application/pdf,image/*" required=(true)>
                <label for="supporting">"Other documents" <span class="hint">" (optional, up to 5: e.g. authorization letter, SPA, DTI/SEC papers)"</span></label>
                <input id="supporting" type="file" name="supporting" accept="application/pdf,image/*" multiple=(true)>
                <p class="hint">"Your files are encrypted when stored and deleted once your claim is decided."</p>
                <div class="actions">
                    <button type="submit">"Send for review"</button>
                    <a class="button secondary" href="/account">"Cancel"</a>
                </div>
            </form>
        </main>
    })
}

/// Staff only; everyone else gets a 404.
async fn admin(cx: &Cx) -> Result<Account> {
    let account = signed_in(cx).await?;
    if state(cx).config.is_admin(&account.email) {
        Ok(account)
    } else {
        Err(topcoat::router::error::not_found().into())
    }
}

#[page("/admin/claims")]
async fn admin_claims_page(cx: &Cx) -> Result<impl View> {
    admin(cx).await?;
    let claims = claims::pending(&state(cx).db).await?;
    Ok(view! {
        <main class="wide">
            <h1>"TIN claims waiting for review"</h1>
            notice_banner(notice: notice(cx))
            if claims.is_empty() {
                <p class="card muted">"Nothing to review."</p>
            } else {
                <div class="card">
                    <table>
                        <thead>
                            <tr><th>"TIN"</th><th>"Registered name"</th><th>"Account"</th><th>"Sent"</th></tr>
                        </thead>
                        <tbody>
                            for claim in claims {
                                <tr>
                                    <td><a href=(format!("/admin/claims/{}", claim.id))>(claim.tin().to_string())</a></td>
                                    <td>(claim.registered_name.clone())</td>
                                    <td>(claim.email.clone())</td>
                                    <td>(date(claim.created_at))</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                </div>
            }
        </main>
    })
}

mod claim_id {
    use super::*;

    #[page("/admin/claims/{claim_id}")]
    pub async fn admin_claim_page(cx: &Cx) -> Result<impl View> {
        admin(cx).await?;
        let Ok(claim_id) = uri(cx).path().rsplit('/').next().unwrap_or_default().parse::<Uuid>() else {
            return Err(topcoat::router::error::not_found().into());
        };
        let db = &state(cx).db;
        let Some(claim) = claims::find(db, claim_id).await? else {
            return Err(topcoat::router::error::not_found().into());
        };
        let documents = claims::documents(db, claim_id).await?;
        let rivals = claims::rivals(db, &claim).await?;
        let decide = format!("/admin/claims/{claim_id}/decision");
        Ok(view! {
            <main class="wide">
                <p><a href="/admin/claims">"← All claims"</a></p>
                <h1>(claim.tin().to_string())</h1>
                notice_banner(notice: notice(cx))
                <div class="card">
                    <p><strong>"Registered name: "</strong>(claim.registered_name.clone())</p>
                    <p><strong>"Account: "</strong>(claim.email.clone())</p>
                    <p><strong>"Sent: "</strong>(date(claim.created_at))</p>
                    <p><strong>"Status: "</strong>(status_label(&claim.status))</p>
                </div>
                if !rivals.is_empty() {
                    <div class="notice error">
                        <strong>"Other accounts claim this TIN:"</strong>
                        <ul>
                            for rival in rivals {
                                <li>(rival.email.clone()) " — " (status_label(&rival.status)) " since " (date(rival.created_at))</li>
                            }
                        </ul>
                    </div>
                }
                <h2>"Documents"</h2>
                <div class="card">
                    <table>
                        <tbody>
                            for document in documents {
                                <tr>
                                    <td>(DocumentKind::label(&document.kind))</td>
                                    <td>
                                        if document.purged_at.is_some() {
                                            <span class="muted">(document.filename.clone()) " (deleted)"</span>
                                        } else {
                                            <a href=(format!("/admin/documents/{}", document.id)) target="_blank" rel="noopener">(document.filename.clone())</a>
                                        }
                                    </td>
                                    <td>(format!("{} KB", (document.size_bytes + 1023) / 1024))</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                </div>
                if claim.status == "pending" {
                    <h2>"Decision"</h2>
                    <form class="card" method="post" action=(decide)>
                        <p class="muted">"Check that the TIN, registered name and ID match the Form 2303. Approving makes this account the TIN's only owner."</p>
                        <label for="note">"Note to the claimant" <span class="hint">" (required when rejecting)"</span></label>
                        <textarea id="note" name="note" maxlength="500"></textarea>
                        <div class="actions">
                            <button type="submit" name="decision" value="approve">"Approve"</button>
                            <button class="danger" type="submit" name="decision" value="reject">"Reject"</button>
                        </div>
                    </form>
                }
            </main>
        })
    }
}
