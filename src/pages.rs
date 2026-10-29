//! The account site, rendered with Topcoat. Pages only read; every change
//! goes through a form post handled in [`crate::web`].

use std::collections::HashMap;
use std::fmt::Display;

use time::{OffsetDateTime, UtcOffset};
use topcoat::Result;
use topcoat::context::{Cx, app_context};
use topcoat::router::error::redirect;
use topcoat::router::request::{headers, uri};
use topcoat::router::{Router, Slot, layout, page};
use topcoat::view::{View, component, view};
use uuid::Uuid;

use crate::accounts::Account;
use crate::agents::{self, AuthorizeProblem};
use crate::claims::{self, ClaimKind, ClaimRow, DocumentKind, DocumentRow};
use crate::notice::Notice;
use crate::oauth;
use crate::state::AppState;
use crate::web::{link, safe_next, session_account};

mod home;
mod staff;
mod tins;

pub fn router(state: AppState) -> Router {
    Router::builder()
        .app_context(state)
        .layout(root_layout)
        .page(home::home_page)
        .page(login_page)
        .page(signup_page)
        .page(verify_page)
        .page(device_page)
        .page(authorize_page)
        .page(tins::tins_page)
        .page(tins::new_tin_page)
        .page(tins::claim_id::fix_page)
        .page(staff::claims_page)
        .page(staff::claim_id::claim_page)
        .page(staff::users_page)
        .page(staff::account_id::user_page)
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

/// The id at the end of the path (`/staff/claims/{id}`), or a 404.
fn path_id(cx: &Cx, segment_from_end: usize) -> Result<Uuid> {
    uri(cx)
        .path()
        .rsplit('/')
        .nth(segment_from_end)
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| topcoat::router::error::not_found().into())
}

async fn current(cx: &Cx) -> Result<Option<Account>> {
    session_account(state(cx), headers(cx)).await.map_err(fail)
}

/// The signed-in account, or a redirect to sign in and return here.
async fn signed_in(cx: &Cx) -> Result<Account> {
    match current(cx).await? {
        Some(account) => Ok(account),
        None => {
            let here = uri(cx).path_and_query().map_or("/tins", |path| path.as_str()).to_owned();
            Err(redirect(link("/login", Some(Notice::SignInFirst), &[("next", &here)])).into())
        }
    }
}

/// Staff only; signed-in people who are not get a 404.
async fn admin(cx: &Cx) -> Result<Account> {
    let account = signed_in(cx).await?;
    if state(cx).config.is_admin(&account.email) {
        Ok(account)
    } else {
        Err(topcoat::router::error::not_found().into())
    }
}

#[layout("/")]
async fn root_layout(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let account = current(cx).await?;
    let admin = account.as_ref().is_some_and(|account| state(cx).config.is_admin(&account.email));
    let path = uri(cx).path().to_owned();
    let staff_area = admin && (path == "/staff" || path.starts_with("/staff/"));
    let waiting = if staff_area { crate::staff::open_counts(&state(cx).db).await?.0 } else { 0 };
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Buwiz"</title>
                <link rel="stylesheet" href="/assets/site.css">
                brand_styles()
                <link rel="stylesheet" href="/assets/account.css">
                // Not deferred: it marks the page as scripted before the
                // first paint, then sets itself up once the page is parsed.
                <script src="/assets/account.js"></script>
            </head>
            <body>
                if staff_area {
                    staff_header(account: account.clone().expect("staff are signed in"), path: path.clone(), waiting: waiting)
                } else {
                    site_header(account: account.clone(), admin: admin, path: path.clone())
                }
                (slot)
            </body>
        </html>
    })
}

/// Two letters for an account's avatar, from its email.
fn initials(email: &str) -> String {
    email.chars().filter(char::is_ascii_alphanumeric).take(2).collect::<String>().to_uppercase()
}

/// The light header of the account site.
#[component]
async fn site_header(account: Option<Account>, admin: bool, path: String) -> Result<impl View> {
    let on_tins = path.starts_with("/tins");
    Ok(view! {
        <header class="home top">
            <div class="top-in">
                <a class="top-brand" href="/">
                    <picture>
                        <source srcset="/assets/brand/buwiz_logo_dark.webp" media="(prefers-color-scheme: dark)">
                        <img src="/assets/brand/buwiz_logo_light.webp" alt="" width="28" height="28">
                    </picture>
                    <span class="top-name">"Buwiz"</span>
                </a>
                if let Some(account) = account {
                    <nav class="top-nav" aria-label="Account">
                        <a href="/tins" class=(if on_tins { "on" } else { "" })>"My TINs"</a>
                    </nav>
                    account_menu(email: account.email.clone(), admin: admin, staff_area: false)
                } else {
                    <nav class="top-nav top-guest" aria-label="Account">
                        <a class="btn btn-ghost" href="/login">"Sign in"</a>
                        <a class="btn btn-solid" href="/signup">"Create account"</a>
                    </nav>
                }
            </div>
        </header>
    })
}

/// The dark header of the staff pages.
#[component]
async fn staff_header(account: Account, path: String, waiting: i64) -> Result<impl View> {
    let on_users = path.starts_with("/staff/users");
    Ok(view! {
        <header class="home top staff-top">
            <div class="top-in staff-in">
                <a class="top-brand" href="/staff">
                    <span class="staff-logo"><img src="/assets/brand/buwiz_logo_light.webp" alt="" width="24" height="24"></span>
                    <span class="top-name">"Buwiz"</span>
                    <span class="staff-tag">"STAFF"</span>
                </a>
                <nav class="top-nav" aria-label="Staff">
                    <a href="/staff" class=(if on_users { "" } else { "on" })>
                        icon(name: "fact_check")
                        "Claims"
                        if waiting > 0 {
                            <span class="count" aria-label=(format!("{waiting} waiting"))>(waiting.to_string())</span>
                        }
                    </a>
                    <a href="/staff/users" class=(if on_users { "on" } else { "" })>icon(name: "group") "Users"</a>
                </nav>
                account_menu(email: account.email.clone(), admin: true, staff_area: true)
            </div>
        </header>
    })
}

/// The avatar button and its menu. A `<details>`, so it opens without
/// JavaScript.
#[component]
async fn account_menu(email: String, admin: bool, staff_area: bool) -> Result<impl View> {
    Ok(view! {
        <details class="top-menu">
            <summary aria-label=(format!("Account menu for {email}"))>
                <span class="avatar">(initials(&email))</span>
                icon(name: "expand_more")
            </summary>
            <div class="top-menu-panel">
                <p class="top-menu-email">(email)</p>
                if staff_area {
                    <a href="/tins">icon(name: "badge") "My TINs"</a>
                } else if admin {
                    <a href="/staff">icon(name: "shield_person") "Staff"</a>
                }
                <form method="post" action="/logout">
                    <button type="submit">icon(name: "logout") "Sign out"</button>
                </form>
            </div>
        </details>
    })
}

/// A notice above a page's content, with an icon for its tone.
#[component]
async fn page_notice(notice: Option<Notice>) -> Result<impl View> {
    Ok(view! {
        if let Some(notice) = notice {
            <p class=(if notice.is_error() { "notice-bar error" } else { "notice-bar" }) role="status">
                icon(name: if notice.is_error() { "error" } else { "check_circle" })
                <span>(notice.message())</span>
            </p>
        }
    })
}

/// The fonts and stylesheet of the home page's look, which every page
/// shares.
#[component]
async fn brand_styles() -> Result<impl View> {
    Ok(view! {
        <link rel="preconnect" href="https://fonts.googleapis.com">
        <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Manrope:wght@400;500;600;700;800&family=IBM+Plex+Mono:wght@500;600&display=swap">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:wght,FILL@100..700,0..1&display=block">
        <link rel="stylesheet" href="/assets/home.css">
    })
}

#[component]
async fn icon(name: &'static str) -> Result<impl View> {
    Ok(view! { <span class="icon" aria-hidden="true">(name)</span> })
}

/// The Buwiz logo and name at the top of a sign-in card.
#[component]
async fn auth_brand() -> Result<impl View> {
    Ok(view! {
        <a class="auth-brand" href="/" aria-label="Buwiz home">
            <picture>
                <source srcset="/assets/brand/buwiz_logo_dark.webp" media="(prefers-color-scheme: dark)">
                <img src="/assets/brand/buwiz_logo_light.webp" alt="" width="30" height="30">
            </picture>
            <span>"BUWIZ"</span>
        </a>
    })
}

/// A notice on a sign-in card, with an icon for its tone.
#[component]
async fn auth_notice(notice: Option<Notice>) -> Result<impl View> {
    Ok(view! {
        if let Some(notice) = notice {
            <p class=(if notice.is_error() { "auth-notice error" } else { "auth-notice" }) role="status">
                icon(name: if notice.is_error() { "error" } else { "check_circle" })
                <span>(notice.message())</span>
            </p>
        }
    })
}

/// Sign in and sign up: both email a code. `switch_*` links to the other.
#[component]
async fn email_form(
    cx: &Cx,
    title: &'static str,
    intro: &'static str,
    switch_prompt: &'static str,
    switch_label: &'static str,
    switch_path: &'static str,
) -> Result<impl View> {
    let next = safe_next(query(cx, "next").as_deref());
    let email = query(cx, "email").unwrap_or_default();
    let switch = link(switch_path, None, &[("next", &next)]);
    Ok(view! {
        <main class="home auth">
            <section class="auth-card">
                auth_brand()
                <h1>(title)</h1>
                <p class="auth-lead">(intro)</p>
                auth_notice(notice: notice(cx))
                <form class="auth-form" method="post" action="/auth/email/start">
                    <input type="hidden" name="next" value=(next)>
                    <label class="auth-label" for="email">"Email address"</label>
                    <div class="auth-field">
                        icon(name: "mail")
                        <input
                            id="email"
                            type="email"
                            name="email"
                            value=(email)
                            placeholder="you@example.com"
                            autocomplete="email"
                            required=(true)
                            autofocus=(true)
                        >
                    </div>
                    <button class="btn btn-solid auth-submit" type="submit">"Email me a code" icon(name: "arrow_forward")</button>
                </form>
                <p class="auth-hint">icon(name: "lock") "No password to remember. Each code works once, for 10 minutes."</p>
                <p class="auth-switch">(switch_prompt) " " <a href=(switch)>(switch_label)</a></p>
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
        </main>
    })
}

#[page("/login")]
async fn login_page(cx: &Cx) -> Result<impl View> {
    if current(cx).await?.is_some() {
        return Err(redirect(safe_next(query(cx, "next").as_deref())).into());
    }
    Ok(view! {
        email_form(
            title: "Sign in to Buwiz",
            intro: "Enter your email and we will send you a 6-digit code.",
            switch_prompt: "New to Buwiz?",
            switch_label: "Create an account",
            switch_path: "/signup"
        )
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
            intro: "Enter your email. We will send a code to confirm it; that is all you need.",
            switch_prompt: "Already have an account?",
            switch_label: "Sign in",
            switch_path: "/login"
        )
    })
}

#[page("/login/verify")]
async fn verify_page(cx: &Cx) -> Result<impl View> {
    let next = safe_next(query(cx, "next").as_deref());
    let email = query(cx, "email").unwrap_or_default();
    let restart = link("/login", None, &[("next", &next), ("email", &email)]);
    Ok(view! {
        <main class="home auth">
            <section class="auth-card">
                auth_brand()
                <span class="auth-badge">icon(name: "mark_email_unread")</span>
                <h1>"Check your email"</h1>
                <p class="auth-lead">"We sent a 6-digit code to " <strong>(email.clone())</strong> "."</p>
                auth_notice(notice: notice(cx))
                <form class="auth-form" method="post" action="/auth/email/verify" data-code-form="">
                    <input type="hidden" name="next" value=(next.clone())>
                    <input type="hidden" name="email" value=(email.clone())>
                    <label class="auth-label" for="code">"Verification code"</label>
                    // One field without JavaScript; account.js splits it
                    // into six boxes and signs in on the last digit.
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
                        data-code=""
                    >
                    <p class="auth-hint code-hint" hidden=(true)>icon(name: "bolt") "Paste the whole code or type it. You're signed in as soon as the last digit goes in."</p>
                    <button class="btn btn-solid auth-submit" type="submit">"Sign in" icon(name: "arrow_forward")</button>
                </form>
                <form class="auth-resend" method="post" action="/auth/email/start">
                    <input type="hidden" name="next" value=(next)>
                    <input type="hidden" name="email" value=(email)>
                    <span>"No code? Check spam, or"</span>
                    <button class="auth-link" type="submit">"send a new one"</button>
                </form>
                <p class="auth-switch"><a href=(restart)>"Use a different email"</a></p>
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
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
        <main class="home auth">
            <section class="auth-card">
                auth_brand()
                <span class="auth-badge">icon(name: "devices")</span>
                <h1>"Sign in to the app"</h1>
                auth_notice(notice: shown)
                if bad_code {
                    auth_notice(notice: Some(Notice::DeviceInvalid))
                }
                if finished {
                } else if let Some((code, name)) = request {
                    <p class="auth-lead">"Check that this code matches the one shown in " <strong>(name)</strong> ":"</p>
                    <p class="auth-device-code">(code.clone())</p>
                    <p class="auth-hint">icon(name: "shield_person") "Approve only if you started this sign-in yourself. The app will act as " (email) "."</p>
                    <form class="auth-form" method="post" action="/device/decision">
                        <input type="hidden" name="user_code" value=(code)>
                        <div class="auth-actions">
                            <button class="btn btn-solid" type="submit" name="decision" value="approve">icon(name: "check") "Approve"</button>
                            <button class="btn btn-ghost" type="submit" name="decision" value="deny">"Decline"</button>
                        </div>
                    </form>
                } else {
                    <p class="auth-lead">"Enter the code the Buwiz app shows you."</p>
                    <form class="auth-form" method="get" action="/device">
                        <label class="auth-label" for="user_code">"Code shown in the app"</label>
                        <input
                            id="user_code"
                            class="auth-code auth-code-letters"
                            type="text"
                            name="user_code"
                            placeholder="ABCD-EFGH"
                            autocomplete="off"
                            required=(true)
                            autofocus=(true)
                        >
                        <button class="btn btn-solid auth-submit" type="submit">"Continue" icon(name: "arrow_forward")</button>
                    </form>
                }
            </section>
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
        </main>
    })
}

/// An AI agent asks to read the account (OAuth consent).
#[page("/oauth/authorize")]
async fn authorize_page(cx: &Cx) -> Result<impl View> {
    let params: HashMap<String, String> =
        uri(cx).query().map(|raw| form_urlencoded::parse(raw.as_bytes()).into_owned().collect()).unwrap_or_default();
    let config = &state(cx).config;
    let (request, problem) = match agents::check_authorize(&state(cx).db, config, &params).await {
        Ok(request) => (Some(request), ""),
        Err(problem @ AuthorizeProblem::Redirect { .. }) => {
            let location = agents::problem_redirect(&problem, &config.account_origin).expect("redirect problem");
            return Err(redirect(location).into());
        }
        Err(AuthorizeProblem::Untrusted(message)) => (None, message),
    };
    let fields: Vec<(String, String)> = [
        "client_id",
        "redirect_uri",
        "response_type",
        "code_challenge",
        "code_challenge_method",
        "state",
        "scope",
        "resource",
    ]
    .iter()
    .filter_map(|name| params.get(*name).map(|value| ((*name).to_owned(), value.clone())))
    .collect();
    let email = signed_in(cx).await?.email;
    Ok(view! {
        <main class="home auth">
            if let Some(request) = request {
                <section class="auth-card consent">
                    <div class="consent-pair" aria-hidden="true">
                        <span class="consent-app">(request.client_name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default())</span>
                        icon(name: "sync_alt")
                        <span class="consent-buwiz">
                            <picture>
                                <source srcset="/assets/brand/buwiz_logo_dark.webp" media="(prefers-color-scheme: dark)">
                                <img src="/assets/brand/buwiz_logo_light.webp" alt="" width="32" height="32">
                            </picture>
                        </span>
                    </div>
                    <h1>(request.client_name.clone()) " wants to read your Buwiz account"</h1>
                    <p class="consent-who">"Signed in as " <strong>(email)</strong></p>
                    <div class="consent-lists">
                        <div class="consent-can">
                            <p class="consent-head">icon(name: "visibility") "It can see"</p>
                            <ul>
                                <li>"Your active TINs, with the middle digits hidden"</li>
                                <li>"Your TIN claims and their review status"</li>
                                <li>"Returns you sent through Buwiz, and their receipts"</li>
                            </ul>
                        </div>
                        <div class="consent-cannot">
                            <p class="consent-head">icon(name: "block") "It can't"</p>
                            <ul>
                                <li>"File or send a return"</li>
                                <li>"Change your account or your TINs"</li>
                                <li>"See your uploaded documents"</li>
                            </ul>
                        </div>
                    </div>
                    <p class="auth-hint">icon(name: "desktop_windows") "Returns are filed only from the Buwiz app on your computer, after you confirm them there."</p>
                    <form class="auth-form" method="post" action="/oauth/authorize/decision">
                        for (name, value) in fields {
                            <input type="hidden" name=(name) value=(value)>
                        }
                        <div class="auth-actions">
                            <button class="btn btn-ghost" type="submit" name="decision" value="deny">"Deny"</button>
                            <button class="btn btn-solid" type="submit" name="decision" value="approve">"Allow"</button>
                        </div>
                    </form>
                    <p class="consent-back">"After you answer, you go back to " <strong>(agents::redirect_host(&request.redirect_uri))</strong></p>
                </section>
            } else {
                <section class="auth-card">
                    auth_brand()
                    <span class="auth-badge">icon(name: "link_off")</span>
                    <h1>"This sign-in request cannot be used"</h1>
                    <p class="auth-notice error" role="status">icon(name: "error") <span>(problem)</span></p>
                    <p class="auth-lead">"Go back to the AI app and connect to Buwiz again."</p>
                </section>
            }
            <p class="auth-foot"><a href="/">"Buwiz"</a> " · Philippine tax filing, made modern"</p>
        </main>
    })
}

/// Times people read are Philippine time (UTC+8, no daylight saving).
fn local(at: OffsetDateTime) -> OffsetDateTime {
    at.to_offset(UtcOffset::from_hms(8, 0, 0).expect("valid offset"))
}

/// `October 29, 2026`
fn date(at: OffsetDateTime) -> String {
    let date = local(at).date();
    format!("{} {}, {}", date.month(), date.day(), date.year())
}

/// `Oct 29, 2026`
fn short_date(at: OffsetDateTime) -> String {
    let date = local(at).date();
    format!("{:.3} {}, {}", date.month().to_string(), date.day(), date.year())
}

/// `Oct 29, 2026 · 4:12 PM`
fn date_time(at: OffsetDateTime) -> String {
    let local = local(at);
    let (hour, minute) = (local.hour(), local.minute());
    let half = if hour < 12 { "AM" } else { "PM" };
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{} · {hour}:{minute:02} {half}", short_date(at))
}

/// How long ago: `5 minutes`, `2 hours`, `3 days`.
fn span_since(at: OffsetDateTime) -> String {
    let elapsed = OffsetDateTime::now_utc() - at;
    let (count, unit) = if elapsed.whole_days() >= 1 {
        (elapsed.whole_days(), "day")
    } else if elapsed.whole_hours() >= 1 {
        (elapsed.whole_hours(), "hour")
    } else {
        (elapsed.whole_minutes().max(1), "minute")
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// `2 hours ago`, or `never`.
fn ago(at: Option<OffsetDateTime>) -> String {
    match at {
        Some(at) if OffsetDateTime::now_utc() - at < time::Duration::minutes(1) => "just now".to_owned(),
        Some(at) => format!("{} ago", span_since(at)),
        None => "never".to_owned(),
    }
}

/// A claim's status as people read it: (label, icon, CSS tone).
fn status_look(status: &str) -> (&'static str, &'static str, &'static str) {
    match status {
        "pending" => ("Being checked", "schedule", "waiting"),
        "needs_fix" => ("Needs a fix", "error", "fix"),
        "approved" => ("Active", "check_circle", "active"),
        "rejected" => ("Not approved", "cancel", "rejected"),
        _ => ("Withdrawn", "undo", "withdrawn"),
    }
}

/// A rounded label with an icon.
#[component]
async fn pill(label: String, icon_name: &'static str, tone: String) -> Result<impl View> {
    Ok(view! {
        <span class=(format!("tagpill {tone}"))>icon(name: icon_name) (label)</span>
    })
}

/// Personal or business, as a pill.
#[component]
async fn kind_pill(kind: String) -> Result<impl View> {
    let personal = kind == "personal";
    Ok(view! {
        pill(
            label: ClaimKind::label(&kind).to_owned(),
            icon_name: if personal { "person" } else { "storefront" },
            tone: (if personal { "kind-personal" } else { "kind-business" }).to_owned()
        )
    })
}

/// The masked or full TIN in the monospace face.
#[component]
async fn tin_text(tin: String) -> Result<impl View> {
    Ok(view! { <span class="mono tin-number">(tin)</span> })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_in_philippine_time() {
        let at = OffsetDateTime::from_unix_timestamp(1_793_262_720).unwrap(); // 2026-10-29 08:32 UTC
        assert_eq!(date(at), "October 29, 2026");
        assert_eq!(short_date(at), "Oct 29, 2026");
        assert_eq!(date_time(at), "Oct 29, 2026 · 4:32 PM");
    }

    #[test]
    fn avatars_use_two_letters_of_the_email() {
        assert_eq!(initials("ana@example.com"), "AN");
        assert_eq!(initials("a.b@example.com"), "AB");
    }
}
