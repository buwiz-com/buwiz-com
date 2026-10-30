//! Staff pages under `/staff`: the claims queue, one claim's review, every
//! account, and one account. Signed-in people who are not staff get a 404
//! on all of them ([`admin`]).

use super::*;
use crate::claims::FIX_REASONS;
use crate::staff::{self as queries, AppRow, IdStatus, QueueFilter, QueueRow, UserFilter};

/// A small note on a queue row or in the checks list: (tone, icon, text).
/// Tones: `good`, `warn`, `bad`, `info`.
type Flag = (&'static str, &'static str, String);

#[component]
async fn flag(item: Flag) -> Result<impl View> {
    let (tone, icon_name, text) = item;
    Ok(view! { <span class=(format!("flag {tone}"))>icon(name: icon_name) (text)</span> })
}

/// What a reviewer should notice first about a claim. These are plain
/// checks on the claim's rows; they help look, staff still decide.
fn queue_flags(row: &QueueRow) -> Vec<Flag> {
    let claim = &row.claim;
    let has = |kind: &str| row.document_kinds.iter().any(|stored| stored == kind);
    let mut flags = Vec::new();
    if row.rivals > 0 {
        flags.push(("bad", "group", "Another account claims this TIN".to_owned()));
    } else if claim.is_open() {
        flags.push(("good", "person_check", "First claim on this TIN".to_owned()));
    }
    if claim.status == "pending" && claim.resent_at.is_some() {
        flags.push(("warn", "replay", "Resent after a fix".to_owned()));
    }
    if claim.kind == "personal" {
        if has("selfie") {
            flags.push(("good", "face", "Selfie added".to_owned()));
        }
        if has("national_id") {
            flags.push(("good", "id_card", "National ID added".to_owned()));
        }
    } else if claim.is_open() && !has("authority") {
        flags.push(("info", "gavel", "No proof of authority".to_owned()));
    }
    flags
}

#[page("/staff")]
pub async fn claims_page(cx: &Cx) -> Result<impl View> {
    admin(cx).await?;
    let filter = QueueFilter::parse(query(cx, "status").as_deref());
    let search = query(cx, "q").unwrap_or_default();
    let db = &state(cx).db;
    let rows = queries::queue(db, filter, &search).await?;
    let (waiting, sent_back) = queries::open_counts(db).await?;
    let oldest_first = matches!(filter, QueueFilter::Waiting | QueueFilter::SentBack);
    let tabs: Vec<(QueueFilter, String, Option<i64>)> = QueueFilter::ALL
        .into_iter()
        .map(|tab| {
            let count = match tab {
                QueueFilter::Waiting => Some(waiting),
                QueueFilter::SentBack => Some(sent_back),
                _ => None,
            };
            (tab, link("/staff", None, &[("status", tab.code()), ("q", &search)]), count)
        })
        .collect();
    Ok(view! {
        <main class="home page staff">
            <div class="page-in staff-in">
                <div class="page-head">
                    <h1>"TIN claims"</h1>
                    search_box(action: "/staff", label: "Search claims", placeholder: "Search TIN, name or email", value: search.clone(), keep: vec![("status", filter.code())])
                </div>
                page_notice(notice: notice(cx))
                <div class="filter-row">
                    <nav class="filters" aria-label="Claim status">
                        for (tab, href, count) in tabs {
                            <a href=(href) class=(if tab == filter { "on" } else { "" }) aria-current=(if tab == filter { "page" } else { "false" })>
                                (tab.label())
                                if let Some(count) = count {
                                    <span class=(if tab == QueueFilter::Waiting { "count hot" } else { "count" })>(count.to_string())</span>
                                }
                            </a>
                        }
                    </nav>
                    <span class="sort">icon(name: "sort") (if oldest_first { "Oldest first" } else { "Newest first" })</span>
                </div>
                if rows.is_empty() {
                    <p class="panel empty">(if search.is_empty() { "Nothing here." } else { "No claims match that search." })</p>
                } else {
                    <div class="list">
                        #[key(row.claim.id.as_u128())]
                        for row in rows {
                            queue_row(row: row)
                        }
                    </div>
                }
                <p class="fine left">"Flags come from simple checks on each claim. They help you look; you still decide."</p>
            </div>
        </main>
    })
}

/// A GET search field that keeps the page's other filters.
#[component]
async fn search_box(
    action: &'static str,
    label: &'static str,
    placeholder: &'static str,
    value: String,
    keep: Vec<(&'static str, &'static str)>,
) -> Result<impl View> {
    Ok(view! {
        <form class="search" method="get" action=(action) role="search">
            for (name, kept) in keep {
                <input type="hidden" name=(name) value=(kept)>
            }
            icon(name: "search")
            <input type="search" name="q" value=(value) placeholder=(placeholder) aria-label=(label)>
        </form>
    })
}

#[component]
async fn queue_row(row: QueueRow) -> Result<impl View> {
    let flags = queue_flags(&row);
    let claim = row.claim;
    let (age, late) = match claim.status.as_str() {
        "pending" => {
            let since = claim.waiting_since();
            (format!("Waiting {}", span_since(since)), OffsetDateTime::now_utc() - since > time::Duration::days(1))
        }
        "needs_fix" => (format!("Sent back {}", ago(claim.sent_back_at)), false),
        _ => (format!("Decided {}", claim.decided_at.map(short_date).unwrap_or_default()), false),
    };
    Ok(view! {
        <a class="list-row" href=(format!("/staff/claims/{}", claim.id))>
            <span class="row-main">
                <span class="row-line">kind_pill(kind: claim.kind.clone()) tin_text(tin: claim.tin().to_string())</span>
                <span class="row-name">(claim.registered_name.clone())</span>
            </span>
            <span class="row-who">
                <span class="row-email">(claim.email.clone())</span>
                <span class=(if late { "row-age late" } else { "row-age" })>icon(name: "schedule") (age)</span>
            </span>
            <span class="row-flags">
                for item in flags {
                    flag(item: item)
                }
            </span>
            <span class="row-go">icon(name: "chevron_right")</span>
        </a>
    })
}

/// "Typed vs. on the document": what the person typed beside what the
/// document says. The document column waits for AI reading (a later
/// change); each row's third value is where an extracted value goes, and
/// it is `None` until then.
#[component]
async fn typed_vs_document(rows: Vec<(&'static str, String, Option<String>)>) -> Result<impl View> {
    Ok(view! {
        <section class="panel side-card">
            <div class="side-head">
                <h2>"Typed vs. on the document"</h2>
                <span class="side-tag">icon(name: "auto_awesome") "AI reading comes later"</span>
            </div>
            <div class="compare">
                <span></span>
                <span class="compare-head">"TYPED"</span>
                <span class="compare-head">"DOCUMENT"</span>
                <span></span>
                for (label, typed, read) in rows {
                    <span class="compare-label">(label)</span>
                    <span class="compare-value">(typed)</span>
                    if let Some(read) = read {
                        <span class="compare-value">(read)</span>
                    } else {
                        <span class="compare-value unread">"Not read yet"</span>
                    }
                    <span class="compare-mark">icon(name: "more_horiz")</span>
                }
            </div>
        </section>
    })
}

pub mod claim_id {
    use super::*;

    #[page("/staff/claims/{claim_id}")]
    pub async fn claim_page(cx: &Cx) -> Result<impl View> {
        let reviewer = admin(cx).await?;
        let claim_id = path_id(cx, 0)?;
        let db = &state(cx).db;
        let Some(claim) = claims::find(db, claim_id).await? else {
            return Err(topcoat::router::error::not_found().into());
        };
        let documents = claims::documents(db, claim_id).await?;
        let rivals = claims::rivals(db, &claim).await?;
        let (previous, next) = queries::neighbours(db, &claim).await?;
        let claimant = queries::user(db, claim.account_id).await?;
        let claimant_tins = claims::for_account(db, claim.account_id)
            .await?
            .iter()
            .filter(|other| !matches!(other.status.as_str(), "withdrawn" | "rejected"))
            .count();
        let kind = ClaimKind::parse(&claim.kind).unwrap_or(ClaimKind::Business);
        let stored = |document: DocumentKind| {
            documents.iter().any(|row| row.kind == document.as_str() && row.purged_at.is_none())
        };

        let mut checks: Vec<Flag> = Vec::new();
        let other_accounts: Vec<ClaimRow> =
            rivals.into_iter().filter(|rival| rival.account_id != claim.account_id).collect();
        if other_accounts.is_empty() {
            checks.push(("good", "person_check", "No other account claims this TIN".to_owned()));
        } else {
            checks.push(("bad", "group", format!("{} other account(s) claim this TIN", other_accounts.len())));
        }
        if claim.is_open() {
            for required in kind.required() {
                let label = DocumentKind::label(required.as_str());
                checks.push(if stored(*required) {
                    ("good", "task", format!("{label} sent"))
                } else {
                    ("bad", "error", format!("{label} missing"))
                });
            }
            match kind {
                ClaimKind::Personal => {
                    for (optional, label) in
                        [(DocumentKind::Selfie, "selfie"), (DocumentKind::NationalId, "National ID")]
                    {
                        checks.push(if stored(optional) {
                            ("good", "add_task", format!("Sent a {label} too"))
                        } else {
                            ("info", "remove", format!("No {label} (optional)"))
                        });
                    }
                }
                ClaimKind::Business => checks.push(if stored(DocumentKind::Authority) {
                    ("good", "gavel", "Proof of authority sent".to_owned())
                } else {
                    ("info", "gavel", "No proof of authority: needed only if the 2303 names a company".to_owned())
                }),
            }
            checks.push(("warn", "visibility", "Looks real, and matches what was typed? Your call".to_owned()));
        }

        let mut compare = vec![("TIN", claim.tin().to_string(), None), ("Name", claim.registered_name.clone(), None)];
        if let Some(trade) = &claim.trade_name {
            compare.push(("Trade name", trade.clone(), None));
        }
        let (status_label, status_icon, status_tone) = status_look(&claim.status);
        let age = match claim.status.as_str() {
            "pending" => Some(format!("Waiting {}", span_since(claim.waiting_since()))),
            _ => None,
        };
        let late =
            claim.status == "pending" && OffsetDateTime::now_utc() - claim.waiting_since() > time::Duration::days(1);
        let fix_reasons: Vec<&str> =
            claim.fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(label, _)| label).collect();
        let own_claim = claim.account_id == reviewer.id;
        let decide = format!("/staff/claims/{claim_id}/decision");
        let tabs: Vec<(usize, DocumentRow)> = documents.into_iter().enumerate().collect();
        Ok(view! {
            <main class="home page staff">
                <div class="page-in staff-in">
                    <div class="claim-head">
                        <div class="claim-heading">
                            <a class="back" href="/staff">icon(name: "arrow_back") "Claims waiting"</a>
                            <div class="claim-title">
                                <h1 class="mono">(claim.tin().to_string())</h1>
                                kind_pill(kind: claim.kind.clone())
                                if let Some(age) = age {
                                    pill(label: age, icon_name: "schedule", tone: (if late { "status-fix" } else { "status-waiting" }).to_owned())
                                } else {
                                    pill(label: status_label.to_owned(), icon_name: status_icon, tone: format!("status-{status_tone}"))
                                }
                            </div>
                        </div>
                        <nav class="claim-nav" aria-label="Queue">
                            if let Some(previous) = previous {
                                <a class="btn btn-ghost btn-small" href=(format!("/staff/claims/{previous}")) aria-label="Previous claim">icon(name: "chevron_left")</a>
                            }
                            if let Some(next) = next {
                                <a class="btn btn-ghost btn-small" href=(format!("/staff/claims/{next}"))>"Next claim" icon(name: "chevron_right")</a>
                            }
                        </nav>
                    </div>
                    page_notice(notice: notice(cx))
                    if !other_accounts.is_empty() {
                        <div class="notice-bar error">
                            icon(name: "group")
                            <div>
                                <strong>"Other accounts claim this TIN:"</strong>
                                <ul>
                                    for rival in other_accounts {
                                        <li>
                                            <a href=(format!("/staff/claims/{}", rival.id))>(rival.email.clone())</a>
                                            " · " (status_look(&rival.status).0) " since " (short_date(rival.created_at))
                                        </li>
                                    }
                                </ul>
                            </div>
                        </div>
                    }
                    <div class="review">
                        document_viewer(documents: tabs)
                        <aside class="review-side">
                            <section class="panel side-card who">
                                <span class="avatar big">(initials(&claim.email))</span>
                                <span class="who-text">
                                    <b>(claim.email.clone())</b>
                                    <span>
                                        "Joined " (claimant.map(|user| short_date(user.created_at)).unwrap_or_default())
                                        " · " (if claimant_tins <= 1 { "first TIN".to_owned() } else { format!("{claimant_tins} TINs") })
                                    </span>
                                </span>
                                <a href=(format!("/staff/users/{}", claim.account_id))>"View user"</a>
                            </section>
                            typed_vs_document(rows: compare)
                            if !checks.is_empty() {
                                <section class="panel side-card">
                                    <h2>"Checks"</h2>
                                    <div class="checks">
                                        for item in checks {
                                            flag(item: item)
                                        }
                                    </div>
                                </section>
                            }
                            if claim.sent_back_at.is_some() {
                                <section class="panel side-card">
                                    <h2>"Sent back before"</h2>
                                    <p class="side-text">
                                        (claim.sent_back_at.map(date_time).unwrap_or_default())
                                        if !fix_reasons.is_empty() {
                                            ": " (fix_reasons.join(", "))
                                        }
                                    </p>
                                    if let Some(note) = claim.fix_note.clone() {
                                        <p class="side-text quote">(note)</p>
                                    }
                                    if let Some(resent) = claim.resent_at {
                                        <p class="side-text">"Resent " (date_time(resent))</p>
                                    } else {
                                        <p class="side-text">"Waiting for the person's fix."</p>
                                    }
                                </section>
                            }
                            if own_claim && claim.is_open() {
                                <p class="panel side-card side-text">"This is your own claim. Another staff member decides it."</p>
                            } else if claim.is_open() {
                                <form class="panel side-card decision" method="post" action=(decide)>
                                    <h2>"Decision"</h2>
                                    <button class="btn btn-solid btn-block" type="submit" name="decision" value="approve">
                                        icon(name: "verified")
                                        <span class="ellipsis">"Approve: " (claim.email.clone()) " owns this TIN"</span>
                                    </button>
                                    if claim.status == "pending" {
                                        <p class="decision-or">"Or send it back with a reason"</p>
                                        <div class="reason-chips">
                                            for (code, label, _) in FIX_REASONS {
                                                <label class="reason">
                                                    <input type="checkbox" name="reasons" value=(*code)>
                                                    <span>(*label)</span>
                                                </label>
                                            }
                                        </div>
                                    }
                                    <label class="decision-label" for="note">"Note to the person"</label>
                                    <textarea id="note" name="note" rows="3" maxlength="500" placeholder="They see this on their My TINs page. Required to reject."></textarea>
                                    <div class="decision-row">
                                        if claim.status == "pending" {
                                            <button class="btn btn-ghost" type="submit" name="decision" value="fix">"Ask for a fix"</button>
                                        }
                                        <button class="btn btn-danger" type="submit" name="decision" value="reject">"Reject"</button>
                                    </div>
                                </form>
                            } else {
                                <section class="panel side-card">
                                    <h2>"Decided"</h2>
                                    <p class="side-text">(status_label) " · " (claim.decided_at.map(date_time).unwrap_or_default())</p>
                                    if let Some(note) = claim.decision_note.clone() {
                                        <p class="side-text quote">(note)</p>
                                    }
                                </section>
                            }
                        </aside>
                    </div>
                </div>
            </main>
        })
    }
}

/// The claim's files as tabs. The tabs are radio buttons, so they switch
/// without JavaScript; `account.js` adds zoom and rotate for photos.
#[component]
async fn document_viewer(documents: Vec<(usize, DocumentRow)>) -> Result<impl View> {
    let tabs: Vec<(usize, String, &'static str)> = documents
        .iter()
        .map(|(index, row)| (*index, DocumentKind::label(&row.kind).to_owned(), DocumentKind::icon(&row.kind)))
        .collect();
    let radios: Vec<usize> = documents.iter().map(|(index, _)| *index).collect();
    Ok(view! {
        <section class="viewer" aria-label="Documents">
            if documents.is_empty() {
                <p class="viewer-empty">"No files on this claim."</p>
            }
            for index in radios {
                <input class="viewer-radio" type="radio" name="document" id=(format!("document-{index}")) checked=(index == 0)>
            }
            <div class="viewer-bar">
                <div class="viewer-tabs">
                    for (index, label, icon_name) in tabs {
                        <label for=(format!("document-{index}"))>icon(name: icon_name) (label)</label>
                    }
                </div>
                <div class="viewer-tools" data-viewer-tools="" hidden=(true)>
                    <button type="button" data-zoom="-1" aria-label="Zoom out">icon(name: "zoom_out")</button>
                    <button type="button" data-zoom="1" aria-label="Zoom in">icon(name: "zoom_in")</button>
                    <button type="button" data-rotate="" aria-label="Rotate">icon(name: "rotate_right")</button>
                </div>
            </div>
            <div class="viewer-panes">
                for (_, document) in documents {
                    let href = format!("/staff/documents/{}", document.id);
                    <div class="viewer-pane">
                        <div class="viewer-stage">
                            if document.purged_at.is_some() {
                                <p class="viewer-gone">icon(name: "delete") "Deleted when the claim was decided or the file replaced."</p>
                            } else if document.is_viewable_image() {
                                <img src=(href.clone()) alt=(DocumentKind::label(&document.kind)) data-zoomable="">
                            } else {
                                <a class="viewer-open" href=(href.clone()) target="_blank" rel="noopener">
                                    icon(name: if document.content_type == "application/pdf" { "picture_as_pdf" } else { "image" })
                                    "Open the file"
                                </a>
                            }
                        </div>
                        <p class="viewer-foot">
                            <span>
                                (document.filename.clone()) " · " (format!("{} KB", (document.size_bytes + 1023) / 1024))
                                (if document.purged_at.is_some() { " · deleted" } else { " · deleted automatically once you decide" })
                            </span>
                            if document.purged_at.is_none() {
                                <a href=(href) target="_blank" rel="noopener">icon(name: "open_in_new") "Open the original"</a>
                            }
                        </p>
                    </div>
                }
            </div>
        </section>
    })
}

#[page("/staff/users")]
pub async fn users_page(cx: &Cx) -> Result<impl View> {
    admin(cx).await?;
    let filter = UserFilter::parse(query(cx, "filter").as_deref());
    let search = query(cx, "q").unwrap_or_default();
    let config = &state(cx).config;
    let db = &state(cx).db;
    let users = queries::users(db, filter, &search, &config.admin_emails).await?;
    let ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    let claims = queries::claims_of(db, &ids).await?;
    let rows: Vec<UserListRow> = users
        .into_iter()
        .enumerate()
        .map(|(index, user)| {
            let own: Vec<&queries::UserClaim> = claims.iter().filter(|claim| claim.account_id == user.id).collect();
            let mut names: Vec<String> = Vec::new();
            for claim in &own {
                if !names.contains(&claim.registered_name) {
                    names.push(claim.registered_name.clone());
                }
            }
            let tins = own
                .iter()
                .filter(|claim| matches!(claim.status.as_str(), "pending" | "needs_fix" | "approved"))
                .map(|claim| {
                    let tin =
                        crate::tin::Tin::parse(&claim.tin_root, &claim.branch_code).expect("stored TINs are valid");
                    (tin.to_string(), claim.status.clone())
                })
                .collect();
            let id_status = IdStatus::of(config.is_admin(&user.email), own.iter().map(|claim| claim.status.as_str()));
            UserListRow { color: index % 5, user, names, tins, id_status }
        })
        .collect();
    let chips: Vec<(UserFilter, String)> = UserFilter::ALL
        .into_iter()
        .map(|chip| (chip, link("/staff/users", None, &[("filter", chip.code()), ("q", &search)])))
        .collect();
    Ok(view! {
        <main class="home page staff">
            <div class="page-in staff-in">
                <div class="page-head">
                    <h1>"Users"</h1>
                    search_box(action: "/staff/users", label: "Search users", placeholder: "Search email, TIN or registered name", value: search.clone(), keep: vec![("filter", filter.code())])
                </div>
                page_notice(notice: notice(cx))
                <nav class="filters" aria-label="Filter users">
                    for (chip, href) in chips {
                        <a href=(href) class=(if chip == filter { "on" } else { "" }) aria-current=(if chip == filter { "page" } else { "false" })>
                            match chip {
                                UserFilter::Waiting => <span class="dot waiting"></span>,
                                UserFilter::Verified => <span class="dot active"></span>,
                                UserFilter::NoTin => <span class="dot none"></span>,
                                _ => "",
                            }
                            (chip.label())
                        </a>
                    }
                </nav>
                if rows.is_empty() {
                    <p class="panel empty">(if search.is_empty() { "No accounts here." } else { "No accounts match that search." })</p>
                } else {
                    <div class="list">
                        #[key(row.user.id.as_u128())]
                        for row in rows {
                            user_row(row: row)
                        }
                    </div>
                }
                <p class="fine left">"Active is the newest website sign-in or app token. Up to " (queries::LIST_LIMIT.to_string()) " accounts show; search to narrow."</p>
            </div>
        </main>
    })
}

struct UserListRow {
    color: usize,
    user: queries::UserRow,
    names: Vec<String>,
    /// (TIN, claim status)
    tins: Vec<(String, String)>,
    id_status: IdStatus,
}

#[component]
async fn user_row(row: UserListRow) -> Result<impl View> {
    let (id_label, id_icon, id_tone) = row.id_status.look();
    let names = if row.names.is_empty() { "No TIN added".to_owned() } else { row.names.join(" · ") };
    Ok(view! {
        <a class="list-row" href=(format!("/staff/users/{}", row.user.id))>
            <span class="row-main user">
                <span class=(format!("avatar c{}", row.color))>(initials(&row.user.email))</span>
                <span class="row-stack">
                    <span class="row-email strong">(row.user.email.clone())</span>
                    <span class="row-sub">(names)</span>
                </span>
            </span>
            <span class="row-flags">
                for (tin, status) in row.tins {
                    <span class="tin-chip mono"><span class=(format!("dot {}", status_look(&status).2))></span>(tin)</span>
                }
            </span>
            <span class=(format!("id-badge {id_tone}"))>icon(name: id_icon) (id_label)</span>
            <span class="row-dates">
                <span>"Joined " (short_date(row.user.created_at))</span>
                <span>"Active " (ago(row.user.last_active))</span>
            </span>
            <span class="row-go">icon(name: "chevron_right")</span>
        </a>
    })
}

pub mod account_id {
    use super::*;

    #[page("/staff/users/{account_id}")]
    pub async fn user_page(cx: &Cx) -> Result<impl View> {
        admin(cx).await?;
        let account_id = path_id(cx, 0)?;
        let config = &state(cx).config;
        let db = &state(cx).db;
        let Some(user) = queries::user(db, account_id).await? else {
            return Err(topcoat::router::error::not_found().into());
        };
        let claims = claims::for_account(db, account_id).await?;
        let documents = claims::account_documents(db, account_id).await?;
        let apps = queries::apps(db, account_id).await?;
        let sessions = queries::sessions(db, account_id).await?;
        let id_status = IdStatus::of(config.is_admin(&user.email), claims.iter().map(|claim| claim.status.as_str()));
        let (id_label, id_icon, id_tone) = id_status.look();
        let mut names: Vec<String> = Vec::new();
        for claim in claims.iter().filter(|claim| claim.status != "withdrawn") {
            if !names.contains(&claim.registered_name) {
                names.push(claim.registered_name.clone());
            }
        }
        let tin_of = |claim_id: Uuid| {
            claims
                .iter()
                .find(|claim| claim.id == claim_id)
                .map(|claim| (claim.tin().to_string(), claim.status.clone()))
        };
        let cards: Vec<(DocumentRow, String, String)> = documents
            .into_iter()
            .take(24)
            .map(|document| {
                let (tin, status) = tin_of(document.claim_id).unwrap_or_default();
                (document, tin, status)
            })
            .collect();
        let live_sessions = sessions.iter().filter(|(_, live)| *live).count();
        let signed_in: Vec<AppRow> = apps.iter().filter(|app| app.signed_in).cloned().collect();
        let history = history(&user, &claims, &apps, &sessions);
        let subtitle = {
            let mut parts = vec![format!("Joined {}", date(user.created_at))];
            parts.push(format!("active {}", ago(user.last_active)));
            if !names.is_empty() {
                parts.push(names.join(" · "));
            }
            parts.join(" · ")
        };
        Ok(view! {
            <main class="home page staff">
                <div class="page-in staff-in">
                    <a class="back" href="/staff/users">icon(name: "arrow_back") "Users"</a>
                    page_notice(notice: notice(cx))
                    <section class="panel user-head">
                        <span class="avatar huge">(initials(&user.email))</span>
                        <span class="user-who">
                            <b>(user.email.clone())</b>
                            <span>(subtitle)</span>
                        </span>
                        <span class="user-actions">
                            <span class=(format!("id-badge {id_tone}"))>icon(name: id_icon) (id_label)</span>
                            <form method="post" action=(format!("/staff/users/{account_id}/sign-out")) data-confirm="Sign out everywhere?" data-confirm-detail="This account is signed out of every browser, the desktop app and every AI agent. They can sign in again." data-confirm-label="Sign out everywhere" data-confirm-tone="danger">
                                <button class="btn btn-ghost btn-small" type="submit">icon(name: "logout") "Sign out everywhere"</button>
                            </form>
                        </span>
                    </section>
                    <div class="user-grid">
                        <div class="user-main">
                            <section class="panel side-card">
                                <div class="side-head">
                                    <h2>"Identity documents"</h2>
                                    <span class="side-note">"Files are deleted once the claim is decided"</span>
                                </div>
                                if cards.is_empty() {
                                    <p class="side-text">"No documents sent."</p>
                                } else {
                                    <div class="doc-grid">
                                        for (document, tin, status) in cards {
                                            document_card(document: document, tin: tin, status: status)
                                        }
                                    </div>
                                }
                            </section>
                            <section class="panel side-card">
                                <h2>"TINs"</h2>
                                if claims.is_empty() {
                                    <p class="side-text">"No TIN added."</p>
                                }
                                for claim in claims.clone() {
                                    let (label, status_icon, tone) = status_look(&claim.status);
                                    <div class="user-tin">
                                        kind_pill(kind: claim.kind.clone())
                                        tin_text(tin: claim.tin().to_string())
                                        <span class="user-tin-name">(claim.registered_name.clone())</span>
                                        <span class=(format!("user-tin-status {tone}"))>
                                            icon(name: status_icon)
                                            if claim.status == "pending" {
                                                "Waiting " (span_since(claim.waiting_since()))
                                            } else {
                                                (label)
                                            }
                                        </span>
                                        <a href=(format!("/staff/claims/{}", claim.id))>"Open claim"</a>
                                    </div>
                                }
                            </section>
                        </div>
                        <aside class="user-side">
                            <section class="panel side-card">
                                <h2>"Connected apps"</h2>
                                if signed_in.is_empty() && live_sessions == 0 {
                                    <p class="side-text">"Not signed in anywhere."</p>
                                }
                                if live_sessions > 0 {
                                    <div class="app">
                                        <span class="app-icon">icon(name: "language")</span>
                                        <span class="app-text">
                                            <b>"Website"</b>
                                            <span>(if live_sessions == 1 { "1 browser signed in".to_owned() } else { format!("{live_sessions} browsers signed in") })</span>
                                        </span>
                                    </div>
                                }
                                for app in signed_in {
                                    connected_app(account_id: account_id, app: app)
                                }
                            </section>
                            <section class="panel side-card">
                                <h2>"History"</h2>
                                <ol class="history">
                                    for (at, icon_name, what) in history {
                                        <li>
                                            <span class="history-icon">icon(name: icon_name)</span>
                                            <span class="history-text"><b>(what)</b><span>(date_time(at))</span></span>
                                        </li>
                                    }
                                </ol>
                            </section>
                        </aside>
                    </div>
                </div>
            </main>
        })
    }
}

/// One file in a user's identity documents, linking to its claim's review.
#[component]
async fn document_card(document: DocumentRow, tin: String, status: String) -> Result<impl View> {
    let pending = status == "pending";
    Ok(view! {
        <a class=(if pending { "doc-card hot" } else { "doc-card" }) href=(format!("/staff/claims/{}", document.claim_id))>
            <span class="doc-thumb">
                if document.is_viewable_image() {
                    <img src=(format!("/staff/documents/{}", document.id)) alt="" loading="lazy">
                } else if document.purged_at.is_some() {
                    icon(name: "delete")
                } else {
                    icon(name: DocumentKind::icon(&document.kind))
                }
            </span>
            <span class="doc-title">
                <b>(DocumentKind::label(&document.kind))</b>
                if pending {
                    <span class="doc-hot">"Review now"</span>
                } else if document.purged_at.is_some() {
                    <span class="doc-gone">"Deleted"</span>
                }
            </span>
            <span class="doc-sub">"Sent " (short_date(document.created_at)) " · for " (tin)</span>
        </a>
    })
}

#[component]
async fn connected_app(account_id: Uuid, app: AppRow) -> Result<impl View> {
    let name = app.name();
    let detail = if app.is_agent() {
        format!("Read only · allowed {}", short_date(app.created_at))
    } else if app.scope == crate::oauth::SCOPE_APP {
        format!("Signed in {} · can file after confirming", short_date(app.created_at))
    } else {
        format!("Signed in {} · sees TIN ids, never files", short_date(app.created_at))
    };
    let action = format!("/staff/users/{account_id}/apps/{}/sign-out", app.id);
    Ok(view! {
        <div class="app">
            if app.is_agent() {
                <span class="app-icon agent">(name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default())</span>
            } else {
                <span class="app-icon">icon(name: "desktop_windows")</span>
            }
            <span class="app-text">
                <b>(name.clone())</b>
                <span>(detail)</span>
            </span>
            <form method="post" action=(action) data-confirm=(format!("Sign {name} out?")) data-confirm-detail="It loses access to this account until the person connects it again." data-confirm-label=(if app.is_agent() { "Remove" } else { "Sign out" }) data-confirm-tone="danger">
                <button class="btn btn-ghost btn-small" type="submit">(if app.is_agent() { "Remove" } else { "Sign out" })</button>
            </form>
        </div>
    })
}

/// The account's story from the timestamps already kept: when it was
/// made, its sign-ins, its claims and what happened to them. Newest first.
fn history(
    user: &queries::UserRow,
    claims: &[ClaimRow],
    apps: &[AppRow],
    sessions: &[(OffsetDateTime, bool)],
) -> Vec<(OffsetDateTime, &'static str, String)> {
    let mut events = vec![(user.created_at, "person_add", "Created the account".to_owned())];
    for (at, _) in sessions {
        events.push((*at, "login", "Signed in on the website".to_owned()));
    }
    for app in apps {
        let name = app.name();
        if app.is_agent() {
            events.push((app.created_at, "smart_toy", format!("Allowed {name} to read the account")));
        } else {
            events.push((app.created_at, "desktop_windows", format!("Signed in to {name}")));
        }
        if let Some(at) = app.revoked_at {
            let why = match app.revoke_reason.as_deref() {
                Some("refresh_token_reuse") => "ended: an old refresh token was reused",
                Some("signed_out_everywhere" | "signed_out_by_staff") => "signed out by staff",
                _ => "signed out",
            };
            events.push((at, "logout", format!("{name} {why}")));
        }
    }
    for claim in claims {
        let tin = claim.tin().to_string();
        events.push((claim.created_at, "upload_file", format!("Sent {} TIN {tin}", ClaimKind::label(&claim.kind))));
        if let Some(at) = claim.sent_back_at {
            events.push((at, "assignment_return", format!("{tin} sent back for a fix")));
        }
        if let Some(at) = claim.resent_at {
            events.push((at, "replay", format!("Resent {tin}")));
        }
        if let Some(at) = claim.decided_at {
            let (icon_name, what) = match claim.status.as_str() {
                "approved" => ("verified", format!("{tin} approved")),
                "rejected" => ("cancel", format!("{tin} not approved")),
                _ => ("undo", format!("Withdrew {tin}")),
            };
            events.push((at, icon_name, what));
        }
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.0));
    events.truncate(40);
    events
}
