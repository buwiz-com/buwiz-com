//! Staff pages under `/staff`: the claims queue, one claim's review, every
//! account, and one account. Signed-in people who are not staff get a 404
//! on all of them ([`admin`]).

use super::*;
use crate::account::claims::FIX_REASONS;
use crate::account::roles::Role;
use crate::account::staff::{
    self as queries, AccountAuditRow, AppRow, AuditRow, IdStatus, QueueFilter, QueueRow, UserFilter,
};
use crate::ocr::extract::{self, Kind, Verdict};
use crate::ocr::{self, Reading};

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
    if row.test_account {
        flags.push(("info", "science", "Test".to_owned()));
    }
    if row.rivals > 0 {
        flags.push(("bad", "group", "Another account claims this TIN".to_owned()));
    } else if claim.is_open() {
        flags.push(("good", "person_check", "First claim on this TIN".to_owned()));
    }
    if claim.verified_by.as_deref() == Some("auto") && claim.auto_confirmed_at.is_none() {
        flags.push(("warn", "bolt", "Activated automatically · check".to_owned()));
    }
    if claim.paused_at.is_some() {
        flags.push(("warn", "pause_circle", "Paused".to_owned()));
    }
    if claim.status == "pending" && claim.resent_at.is_some() {
        flags.push(("warn", "replay", "Resent after a fix".to_owned()));
    }
    if let Some(at) = claim.files_changed_while_waiting() {
        flags.push(("warn", "swap_horiz", format!("Files changed after sending · {}", date_time(at))));
    }
    let document = if row.reading_kind.as_deref() == Some("form_2303") { "the 2303" } else { "the ID" };
    match ocr::verdict(row.tin_match.as_deref()) {
        Some(Verdict::Match) => flags.push(("good", "fact_check", format!("TIN matches {document}"))),
        Some(Verdict::Partial) => flags.push(("warn", "rule", format!("Branch code differs on {document}"))),
        Some(Verdict::Mismatch) => flags.push(("bad", "report", format!("TIN differs from {document}"))),
        None => {}
    }
    match ocr::verdict(row.name_match.as_deref()) {
        Some(Verdict::Match) => flags.push(("good", "fact_check", "Name matches".to_owned())),
        Some(Verdict::Partial) => flags.push(("warn", "rule", "Name partly matches".to_owned())),
        Some(Verdict::Mismatch) => flags.push(("warn", "report", "Name differs".to_owned())),
        None => {}
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
    staff_member(cx, Power::Review).await?;
    let filter = QueueFilter::parse(query(cx, "status").as_deref());
    let search = query(cx, "q").unwrap_or_default();
    let db = &state(cx).db;
    let rows = queries::queue(db, filter, &search).await?;
    let (waiting, sent_back) = queries::open_counts(db).await?;
    let unconfirmed_auto = queries::unconfirmed_auto(db).await?;
    let oldest_first = matches!(filter, QueueFilter::Waiting | QueueFilter::SentBack);
    let tabs: Vec<(QueueFilter, String, Option<i64>)> = QueueFilter::ALL
        .into_iter()
        .map(|tab| {
            let count = match tab {
                QueueFilter::Waiting => Some(waiting),
                QueueFilter::SentBack => Some(sent_back),
                QueueFilter::Auto => Some(unconfirmed_auto),
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
        "unverified" => (format!("Saved {}", short_date(claim.created_at)), false),
        "removed" | "revoked" => (format!("Ended {}", claim.removed_at.map(short_date).unwrap_or_default()), false),
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

/// One row of "Typed vs. on the document".
struct CompareRow {
    label: &'static str,
    /// `None` for what the form does not ask (birth date, RDO code).
    typed: Option<String>,
    read: Option<String>,
    /// How sure the engine was of `read`, 0 to 1.
    confidence: Option<f32>,
    verdict: Option<Verdict>,
}

/// Where the reading of the claim's TIN ID or 2303 stands, and its rows.
struct Comparison {
    /// Shown in the document column instead of values: "Reading…" and the
    /// like. `None` once read.
    waiting: Option<&'static str>,
    failed: Option<String>,
    from_pdf: bool,
    rows: Vec<CompareRow>,
}

/// What the person typed beside what OCR read from the claim's TIN ID or
/// 2303, with a mark per row. The marks help look; staff still decide.
fn comparison(claim: &ClaimRow, kind: ClaimKind, reading: Option<&Reading>) -> Comparison {
    let status = reading.map(|reading| reading.status.as_str());
    let waiting = match status {
        None => Some("Not read yet"),
        Some("reading") => Some("Reading…"),
        Some("failed") => Some("Couldn't read"),
        Some(_) => None,
    };
    let failed = reading
        .filter(|reading| reading.status == "failed")
        .map(|reading| reading.error.clone().unwrap_or_else(|| "unknown reason".to_owned()));
    let read = reading.filter(|reading| reading.status == "read");
    let fields = read.and_then(|reading| reading.fields.as_ref()).map(|fields| &fields.0);
    let sure = read.and_then(|reading| reading.confidence.as_ref()).map(|confidence| confidence.0.clone());
    let sure = sure.unwrap_or_default();
    let get = |field: fn(&extract::Fields) -> &Option<String>| fields.and_then(|fields| field(fields).clone());
    let expected = if kind == ClaimKind::Personal { Kind::TinId } else { Kind::Form2303 };
    let tin = claim.tin();
    let mut rows = Vec::new();
    if let Some(fields) = fields {
        rows.push(CompareRow {
            label: "Document",
            typed: Some(expected.label().to_owned()),
            read: Some(fields.kind.label().to_owned()),
            confidence: None,
            verdict: Some(if fields.kind == expected { Verdict::Match } else { Verdict::Mismatch }),
        });
    }
    let read_tin = get(|fields| &fields.tin);
    rows.push(CompareRow {
        label: "TIN",
        typed: Some(tin.to_string()),
        verdict: read_tin.as_deref().map(|read| extract::compare_tin(&tin, read)),
        read: read_tin,
        confidence: sure.tin,
    });
    let read_name = get(|fields| &fields.name);
    rows.push(CompareRow {
        label: "Name",
        typed: Some(claim.registered_name.clone()),
        verdict: read_name.as_deref().map(|read| extract::compare_names(&claim.registered_name, read)),
        read: read_name,
        confidence: sure.name,
    });
    match kind {
        ClaimKind::Personal => rows.push(CompareRow {
            label: "Birth date",
            typed: None,
            read: get(|fields| &fields.birth_date),
            confidence: sure.birth_date,
            verdict: None,
        }),
        ClaimKind::Business => {
            let read_trade = get(|fields| &fields.trade_name);
            if claim.trade_name.is_some() || read_trade.is_some() {
                let verdict = match (&claim.trade_name, &read_trade) {
                    (Some(typed), Some(read)) => Some(extract::compare_names(typed, read)),
                    (Some(_), None) | (None, Some(_)) => Some(Verdict::Mismatch),
                    (None, None) => None,
                };
                rows.push(CompareRow {
                    label: "Trade name",
                    typed: claim.trade_name.clone(),
                    read: read_trade,
                    confidence: sure.trade_name,
                    verdict: if fields.is_some() { verdict } else { None },
                });
            }
            rows.push(CompareRow {
                label: "RDO code",
                typed: None,
                read: get(|fields| &fields.rdo),
                confidence: sure.rdo,
                verdict: None,
            });
        }
    }
    Comparison { waiting, failed, from_pdf: read.is_some_and(Reading::from_text_layer), rows }
}

/// "Typed vs. on the document", filled from the reading.
#[component]
async fn typed_vs_document(comparison: Comparison) -> Result<impl View> {
    let Comparison { waiting, failed, from_pdf, rows } = comparison;
    let (tag_icon, tag) = if from_pdf {
        ("picture_as_pdf", "Read from the PDF on our server")
    } else {
        ("document_scanner", "Read by OCR on our server")
    };
    Ok(view! {
        <section class="panel side-card">
            <div class="side-head">
                <h2>"Typed vs. on the document"</h2>
                <span class="side-tag">icon(name: tag_icon) (tag)</span>
            </div>
            if let Some(reason) = failed {
                <p class="compare-status">icon(name: "error") "Couldn't read: " (reason)</p>
            }
            <div class="compare">
                <span></span>
                <span class="compare-head">"TYPED"</span>
                <span class="compare-head">"DOCUMENT"</span>
                <span></span>
                for row in rows {
                    <span class="compare-label">(row.label)</span>
                    if let Some(typed) = row.typed {
                        <span class="compare-value">(typed)</span>
                    } else {
                        <span class="compare-value unread">"Not asked"</span>
                    }
                    if let Some(waiting) = waiting {
                        <span class="compare-value unread">(waiting)</span>
                    } else if let Some(read) = row.read {
                        <span class="compare-value">
                            (read)
                            if let Some(sure) = row.confidence {
                                <span class="compare-sure">(format!("{:.0}% sure", sure * 100.0))</span>
                            }
                        </span>
                    } else {
                        <span class="compare-value unread">"Not found"</span>
                    }
                    match row.verdict {
                        Some(Verdict::Match) => <span class="compare-mark good" title="Matches">icon(name: "check_circle")</span>,
                        Some(Verdict::Partial) => <span class="compare-mark warn" title="Partly matches">icon(name: "warning")</span>,
                        Some(Verdict::Mismatch) => <span class="compare-mark bad" title="Differs">icon(name: "warning")</span>,
                        None => <span class="compare-mark">icon(name: "more_horiz")</span>,
                    }
                }
            </div>
        </section>
    })
}

pub mod claim_id {
    use super::*;

    fn own_claim_of(claim: &ClaimRow, reviewer: &Staff) -> bool {
        claim.account_id == reviewer.account.id
    }

    #[page("/staff/claims/{claim_id}")]
    pub async fn claim_page(cx: &Cx) -> Result<impl View> {
        let reviewer = staff_member(cx, Power::Review).await?;
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
            .filter(|other| crate::filing::gate::is_live(&other.status))
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
            if let Some(at) = claim.files_changed_while_waiting() {
                checks.push(("warn", "swap_horiz", format!("Files changed after sending · {}", date_time(at))));
            }
            checks.push(("warn", "visibility", "Looks real, and matches what was typed? Your call".to_owned()));
        }

        let reading = ocr::for_claim(db, claim_id).await?;
        let compare = comparison(&claim, kind, reading.as_ref());
        let (status_label, status_icon, status_tone) = status_look(claim.shown_status());
        let pauses = claims::pauses(db, &claim.tin()).await?;
        let paused_by = match claim.paused_at {
            Some(_) => sqlx::query_scalar::<_, String>(
                "SELECT a.email FROM tin_claims c JOIN accounts a ON a.id = c.paused_by WHERE c.id = $1",
            )
            .bind(claim.id)
            .fetch_optional(db)
            .await?
            // No one named: Buwiz paused it itself.
            .unwrap_or_else(|| "Buwiz".to_owned()),
            None => String::new(),
        };
        let auto_waiting = claim.verified_by.as_deref() == Some("auto") && claim.auto_confirmed_at.is_none();
        let can_confirm = auto_waiting && reviewer.can(Power::ConfirmAuto) && !own_claim_of(&claim, &reviewer);
        let confirm = format!("/staff/claims/{claim_id}/confirm");
        let auto_sure = claim.auto_check.as_ref().map(|check| {
            let percent = |field: &str| format!("{:.0}%", check["confidence"][field].as_f64().unwrap_or(0.0) * 100.0);
            (percent("tin"), percent("name"))
        });
        let auto_doubts = crate::account::verify::doubt_words(claim.auto_check.as_ref());
        let can_pause = reviewer.can(Power::Pause) && claim.status == "approved" && !own_claim_of(&claim, &reviewer);
        let pause = format!("/staff/claims/{claim_id}/pause");
        let resume = format!("/staff/claims/{claim_id}/resume");
        let age = match claim.status.as_str() {
            "pending" => Some(format!("Waiting {}", span_since(claim.waiting_since()))),
            _ => None,
        };
        let late =
            claim.status == "pending" && OffsetDateTime::now_utc() - claim.waiting_since() > time::Duration::days(1);
        let fix_reasons: Vec<&str> =
            claim.fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(label, _)| label).collect();
        let own_claim = claim.account_id == reviewer.account.id;
        let decide = format!("/staff/claims/{claim_id}/decision");
        let delete = format!("/staff/claims/{claim_id}/delete");
        let revoke = format!("/staff/claims/{claim_id}/revoke");
        let show_delete = claims::DELETABLE.contains(&claim.status.as_str()) && reviewer.can(Power::DeleteClaim);
        let show_revoke = claim.status == "approved" && reviewer.can(Power::EndOwnership);
        let ended = matches!(claim.status.as_str(), "removed" | "revoked");
        let unverified = claim.status == "unverified";
        let claimant_banned = claimant.as_ref().is_some_and(|user| user.banned_at.is_some());
        let can_activate = reviewer.can(Power::ActivateWithoutPapers)
            && matches!(claim.status.as_str(), "unverified" | "pending" | "needs_fix");
        let activate = format!("/staff/claims/{claim_id}/activate");
        let tin = claim.tin().to_string();
        let empty_viewer = if unverified {
            format!("No papers yet. Added without the upload on {}.", date(claim.created_at))
        } else {
            "No files on this claim.".to_owned()
        };
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
                                            " · " (status_look(rival.shown_status()).0) " since " (short_date(rival.created_at))
                                        </li>
                                    }
                                </ul>
                            </div>
                        </div>
                    }
                    <div class="review">
                        document_viewer(documents: tabs, empty: empty_viewer)
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
                            if !unverified {
                                typed_vs_document(comparison: compare)
                            }
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
                            if !auto_doubts.is_empty() {
                                <section class="panel side-card">
                                    <h2>"Not activated automatically"</h2>
                                    <ul class="side-text">
                                        for words in auto_doubts {
                                            <li>(words)</li>
                                        }
                                    </ul>
                                </section>
                            }
                            if claim.verified_by.as_deref() == Some("auto") {
                                <section class="panel side-card">
                                    <h2>"Activated automatically"</h2>
                                    <p class="side-text">
                                        (claim.decided_at.map(date_time).unwrap_or_default()) ". TIN and name matched the TIN ID read on our server"
                                        if let Some((tin_sure, name_sure)) = auto_sure.clone() {
                                            " (" (tin_sure) " and " (name_sure) " sure)"
                                        }
                                        "."
                                    </p>
                                    if let Some(at) = claim.auto_confirmed_at {
                                        <p class="side-text">"Confirmed by staff " (date_time(at)) "; its papers were deleted."</p>
                                    } else {
                                        <p class="side-text">"No person has confirmed it yet. Check the papers, then confirm. They are deleted when you confirm; until then they are kept, even if the TIN is removed or revoked."</p>
                                        if can_confirm {
                                            confirm_form(
                                                label: "Looks right: confirm",
                                                action: confirm.clone(),
                                                question: format!("Confirm {tin} for {}?", claim.email),
                                                detail: "You checked the papers and they match. They are deleted now.".to_owned(),
                                                confirm: "Confirm"
                                            )
                                        } else if own_claim_of(&claim, &reviewer) {
                                            <p class="side-text">"This is your own TIN. Another staff member confirms it."</p>
                                        }
                                    }
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
                            if own_claim && claim.is_open() && can_activate {
                                <p class="panel side-card side-text">"This is your own claim. Another staff member reviews its papers, or you can activate it without papers; the reason goes in your history."</p>
                            } else if own_claim && claim.is_open() {
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
                            } else if unverified {
                                <section class="panel side-card">
                                    <h2>"Not verified"</h2>
                                    <p class="side-text">"The person can prepare returns but not send them."</p>
                                </section>
                            } else if ended {
                                <section class="panel side-card">
                                    <h2>"Ended"</h2>
                                    <p class="side-text">"Approved " (claim.decided_at.map(date_time).unwrap_or_default())</p>
                                    <p class="side-text">
                                        (if claim.status == "removed" { "Removed by the owner " } else { "Revoked by staff " })
                                        (claim.removed_at.map(date_time).unwrap_or_default())
                                    </p>
                                    if let Some(note) = claim.removal_note.clone() {
                                        <p class="side-text quote">(note)</p>
                                    }
                                </section>
                            } else {
                                <section class="panel side-card">
                                    <h2>"Decided"</h2>
                                    <p class="side-text">(status_label) " · " (claim.decided_at.map(date_time).unwrap_or_default())</p>
                                    if let Some(note) = claim.decision_note.clone() {
                                        <p class="side-text quote">(note)</p>
                                    }
                                </section>
                            }
                            if !pauses.is_empty() {
                                <section class="panel side-card">
                                    <h2>"Paused before"</h2>
                                    for line in pauses {
                                        <p class="side-text">
                                            (if line.action == "paused" { "Paused by " } else { "Resumed by " })
                                            (if line.source == "auto" { "Buwiz".to_owned() } else { line.actor_email.clone() }) " on " (date(line.created_at))
                                            if let Some(reason) = line.reason.clone() {
                                                ": " (reason)
                                            }
                                        </p>
                                    }
                                </section>
                            }
                            if can_pause {
                                <section class="panel side-card end-claim">
                                    if let Some(at) = claim.paused_at {
                                        <h2>"Paused"</h2>
                                        <p class="side-text">"Paused by " (paused_by.clone()) " on " (date(at)) ": " (claim.pause_reason.clone().unwrap_or_default())</p>
                                        confirm_form(
                                            label: "Resume sending",
                                            action: resume,
                                            question: format!("Resume sending for {tin}?"),
                                            detail: "The owner can send returns again at once.".to_owned(),
                                            confirm: "Resume",
                                            reason: true
                                        )
                                    } else {
                                        <h2>"Pause sending"</h2>
                                        <p class="side-text">"Keeps the TIN on this account but stops every send at once. The owner sees your reason on My TINs."</p>
                                        confirm_form(
                                            label: "Pause sending",
                                            action: pause,
                                            question: format!("Pause sending for {tin}?"),
                                            detail: "Tickets and sends are refused until you resume. Returns already sent are not affected.".to_owned(),
                                            confirm: "Pause",
                                            reason: true
                                        )
                                    }
                                </section>
                            }
                            if can_activate {
                                <section class="panel side-card end-claim">
                                    <h2>(if unverified { "Activate without papers" } else { "Or activate without papers" })</h2>
                                    if claimant_banned {
                                        <p class="side-text">(Notice::ClaimantBanned.message())</p>
                                    } else {
                                        <p class="side-text">"Makes " (claim.email.clone()) " the owner of " (tin.clone()) " now, without papers. Use it for accounts you know. The reason goes in the account's history."</p>
                                        confirm_form(
                                            label: "Activate without papers",
                                            action: activate,
                                            question: format!("Activate {tin} for {} without papers?", claim.email),
                                            detail: "The TIN becomes active at once and can send returns to the BIR. Your reason is kept in the account's history.".to_owned(),
                                            confirm: "Activate",
                                            reason: true
                                        )
                                    }
                                </section>
                            }
                            if !own_claim && (show_delete || show_revoke) {
                                <section class="panel side-card end-claim">
                                    if show_revoke {
                                        <p class="side-text">"Ends this account's ownership. The TIN stops being active at once, its sent returns stay, and it can be claimed again."</p>
                                        confirm_form(
                                            label: "Revoke ownership",
                                            action: revoke,
                                            question: format!("Revoke {}'s ownership of {}?", claim.email, claim.tin()),
                                            detail: "The TIN stops being active at once; the owner sees your reason on My TINs.".to_owned(),
                                            confirm: "Revoke",
                                            reason: true
                                        )
                                    } else {
                                        <p class="side-text">"Erases the claim and its files for good. Only a line in the account's history stays."</p>
                                        confirm_form(
                                            label: "Delete claim",
                                            action: delete,
                                            question: format!("Delete this claim by {}?", claim.email),
                                            detail: "The claim and every file on it are erased for good.".to_owned(),
                                            confirm: "Delete",
                                            reason: true
                                        )
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
async fn document_viewer(documents: Vec<(usize, DocumentRow)>, empty: String) -> Result<impl View> {
    let tabs: Vec<(usize, String, &'static str)> = documents
        .iter()
        .map(|(index, row)| (*index, DocumentKind::label(&row.kind).to_owned(), DocumentKind::icon(&row.kind)))
        .collect();
    let radios: Vec<usize> = documents.iter().map(|(index, _)| *index).collect();
    Ok(view! {
        <section class="viewer" aria-label="Documents">
            if documents.is_empty() {
                <p class="viewer-empty">(empty)</p>
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
    staff_member(cx, Power::SeeUsers).await?;
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
                // `paused` is an approved row as the list shows it.
                .filter(|claim| claim.status == "paused" || crate::filing::gate::is_live(&claim.status))
                .map(|claim| {
                    let tin = crate::account::tin::Tin::parse(&claim.tin_root, &claim.branch_code)
                        .expect("stored TINs are valid");
                    (tin.to_string(), claim.status.clone())
                })
                .collect();
            let role = user_role(config, &user);
            let id_status = IdStatus::of(role.is_some(), own.iter().map(|claim| claim.status.as_str()));
            // The list says "Support"; Admins already read "Staff".
            let badge = role_badge(role).filter(|(_, tone)| *tone == "support");
            let (test, banned) = (user.test_account, user.banned_at.is_some());
            UserListRow { color: index % 5, user, names, tins, id_status, badge, test, banned }
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

/// The role that counts for a listed account (see [`roles::of`]).
fn user_role(config: &crate::base::config::Config, user: &queries::UserRow) -> Option<(Role, bool)> {
    roles::of(config, &user.email, user.role.as_deref(), user.banned_at.is_some())
}

/// The badge for a staff role: (label, CSS tone).
fn role_badge(role: Option<(Role, bool)>) -> Option<(&'static str, &'static str)> {
    match role? {
        (Role::Admin, true) => Some(("Admin · server setting", "staff")),
        (Role::Admin, false) => Some(("Admin", "staff")),
        (Role::Support, _) => Some(("Support", "support")),
    }
}

struct UserListRow {
    color: usize,
    user: queries::UserRow,
    names: Vec<String>,
    /// (TIN, claim status)
    tins: Vec<(String, String)>,
    id_status: IdStatus,
    badge: Option<(&'static str, &'static str)>,
    test: bool,
    banned: bool,
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
            if let Some((label, tone)) = row.badge {
                <span class=(format!("id-badge {tone}"))>(label)</span>
            }
            if row.test {
                <span class="id-badge test">icon(name: "science") "Test"</span>
            }
            if row.banned {
                <span class="id-badge banned">icon(name: "block") "Banned"</span>
            }
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
        let viewer = staff_member(cx, Power::SeeUsers).await?;
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
        let audit = queries::audit(db, account_id).await?;
        let account_audit = queries::account_audit(db, account_id).await?;
        let role = user_role(config, &user);
        let id_status = IdStatus::of(role.is_some(), claims.iter().map(|claim| claim.status.as_str()));
        let (id_label, id_icon, id_tone) = id_status.look();
        let badge = role_badge(role);
        let is_admin = role.is_some_and(|(role, _)| role == Role::Admin);
        let from_server = role.is_some_and(|(_, from_server)| from_server);
        // Support may sign anyone out but an Admin.
        let may_sign_out = viewer.role == Role::Admin || !is_admin;
        let admin_viewer = viewer.can(Power::ChangeRole);
        let own_account = viewer.account.id == account_id;
        let stored_role = user.role.clone().unwrap_or_else(|| "none".to_owned());
        let role_choices: Vec<(&'static str, &'static str, bool)> = [
            ("none", "No staff role"),
            ("support", "Support: reviews claims, sees users, signs people out"),
            ("admin", "Admin: everything"),
        ]
        .into_iter()
        .map(|(value, label)| (value, label, value == stored_role))
        .collect();
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
        let history = history(&user, &claims, &apps, &sessions, &audit, &account_audit);
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
                            if let Some((label, tone)) = badge {
                                <span class=(format!("id-badge {tone}"))>icon(name: "shield_person") (label)</span>
                            }
                            if user.test_account {
                                <span class="id-badge test">icon(name: "science") "Test account"</span>
                            }
                            if user.banned_at.is_some() {
                                <span class="id-badge banned">icon(name: "block") "Banned"</span>
                            }
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
                                    let (label, status_icon, tone) = status_look(claim.shown_status());
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
                            <section class="panel side-card staff-actions">
                                <h2>"Staff actions"</h2>
                                if may_sign_out {
                                    confirm_form(
                                        label: "Sign out everywhere",
                                        action: format!("/staff/users/{account_id}/sign-out"),
                                        question: "Sign out everywhere?".to_owned(),
                                        detail: "This account is signed out of every browser, the desktop app and every AI agent. They can sign in again.".to_owned(),
                                        confirm: "Sign out everywhere",
                                        reason: true
                                    )
                                }
                                if !admin_viewer {
                                    <p class="side-text">"Only an admin can change roles, bans and test labels."</p>
                                } else if from_server {
                                    <p class="side-text">"Admin through the server setting ADMIN_EMAILS. Change it there."</p>
                                } else if own_account {
                                    <p class="side-text">"This is you. Another admin can change your role."</p>
                                } else {
                                    if user.test_account {
                                        <form class="end-form" method="post" action=(format!("/staff/users/{account_id}/test")) data-confirm=(format!("Remove the test label from {}?", user.email)) data-confirm-detail="Staff no longer see it as a test account." data-confirm-label="Remove">
                                            <input type="hidden" name="test" value="off">
                                            reason_field()
                                            <button class="btn btn-ghost" type="submit">"Remove the test label"</button>
                                        </form>
                                    } else {
                                        <form class="end-form" method="post" action=(format!("/staff/users/{account_id}/test")) data-confirm=(format!("Mark {} as a test account?", user.email)) data-confirm-detail="A label for staff only. Sends work as usual." data-confirm-label="Mark">
                                            <input type="hidden" name="test" value="on">
                                            reason_field()
                                            <button class="btn btn-ghost" type="submit">"Mark as test account"</button>
                                        </form>
                                    }
                                    if let Some(at) = user.banned_at {
                                        <p class="side-text">
                                            "Banned by " (user.banned_by_email.clone().unwrap_or_default()) " on " (date(at)) ": "
                                            (user.ban_reason.clone().unwrap_or_default())
                                        </p>
                                        confirm_form(
                                            label: "Lift the ban",
                                            action: format!("/staff/users/{account_id}/unban"),
                                            question: format!("Lift the ban on {}?", user.email),
                                            detail: "They can sign in and send again.".to_owned(),
                                            confirm: "Lift ban",
                                            reason: true
                                        )
                                    } else if is_admin {
                                        <p class="side-text">"An admin can't be banned. Change the role first."</p>
                                    } else {
                                        confirm_form(
                                            label: "Ban this account",
                                            action: format!("/staff/users/{account_id}/ban"),
                                            question: format!("Ban {}?", user.email),
                                            detail: "They can't sign in or send returns until you lift the ban. Their TINs stay on the account.".to_owned(),
                                            confirm: "Ban",
                                            reason: true
                                        )
                                    }
                                    <form class="end-form" method="post" action=(format!("/staff/users/{account_id}/role")) data-confirm=(format!("Change {}'s role?", user.email)) data-confirm-detail="It takes effect on their next page." data-confirm-label="Change role">
                                        <label class="decision-label">
                                            "Role"
                                            <select name="role">
                                                for (value, label, chosen) in role_choices {
                                                    <option value=(value) selected=(chosen)>(label)</option>
                                                }
                                            </select>
                                        </label>
                                        <label class="decision-label">
                                            "Reason"
                                            <textarea name="reason" rows="2" maxlength="500" required=(true) placeholder="Kept in the account's history."></textarea>
                                        </label>
                                        <button class="btn btn-ghost" type="submit">"Change role"</button>
                                    </form>
                                }
                            </section>
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
                                    connected_app(account_id: account_id, app: app, may_sign_out: may_sign_out)
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
async fn connected_app(account_id: Uuid, app: AppRow, may_sign_out: bool) -> Result<impl View> {
    let name = app.name();
    let detail = if app.is_agent() {
        format!("Read only · allowed {}", short_date(app.created_at))
    } else if app.scope == crate::account::oauth::SCOPE_APP {
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
            if may_sign_out {
                <form method="post" action=(action) data-confirm=(format!("Sign {name} out?")) data-confirm-detail="It loses access to this account until the person connects it again." data-confirm-label=(if app.is_agent() { "Remove" } else { "Sign out" }) data-confirm-tone="danger">
                    <button class="btn btn-ghost btn-small" type="submit">(if app.is_agent() { "Remove" } else { "Sign out" })</button>
                </form>
            }
        </div>
    })
}

/// The account's story from the timestamps already kept: when it was
/// made, its sign-ins, its claims and what happened to them, and the audit
/// lines (deleted claims included). Newest first.
fn history(
    user: &queries::UserRow,
    claims: &[ClaimRow],
    apps: &[AppRow],
    sessions: &[(OffsetDateTime, bool)],
    audit: &[AuditRow],
    account_audit: &[AccountAuditRow],
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
        let kind = ClaimKind::label(&claim.kind);
        match claim.submitted_at {
            Some(at) if at == claim.created_at => {
                events.push((claim.created_at, "upload_file", format!("Sent {kind} TIN {tin}")));
            }
            sent => {
                events.push((claim.created_at, "bookmark_add", format!("Added {kind} TIN {tin} without papers")));
                if let Some(at) = sent {
                    events.push((at, "upload_file", format!("Sent the papers for {tin}")));
                }
            }
        }
        if let Some(at) = claim.sent_back_at {
            events.push((at, "assignment_return", format!("{tin} sent back for a fix")));
        }
        if let Some(at) = claim.resent_at {
            events.push((at, "replay", format!("Resent {tin}")));
        }
        if let Some(at) = claim.decided_at {
            let (icon_name, what) = match claim.status.as_str() {
                "approved" | "removed" | "revoked" => match claim.verified_by.as_deref() {
                    Some("admin") => ("verified", format!("{tin} activated without papers")),
                    Some("seed") => ("verified", format!("{tin} activated by the seed tool")),
                    Some("auto") => ("bolt", format!("{tin} activated automatically from its TIN ID")),
                    Some("unknown") => ("verified", format!("{tin} became active; how is not on record")),
                    _ => ("verified", format!("{tin} approved")),
                },
                "rejected" => ("cancel", format!("{tin} not approved")),
                _ => ("undo", format!("Withdrew {tin}")),
            };
            events.push((at, icon_name, what));
        }
    }
    for line in audit {
        events.push((line.created_at, audit_icon(&line.action), audit_text(line, user.id)));
    }
    for line in account_audit {
        events.push((line.created_at, account_audit_icon(&line.action), account_audit_text(line)));
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.0));
    events.truncate(40);
    events
}

fn audit_icon(action: &str) -> &'static str {
    match action {
        "deleted" => "delete",
        "removed" => "remove_circle",
        "revoked" => "block",
        "file_replaced" => "swap_horiz",
        "activated" | "auto_activated" | "auto_confirmed" => "verified",
        "paused" => "pause_circle",
        "resumed" => "play_circle",
        _ => "draft",
    }
}

/// Who did it, as the history names them: the person themself, a staff
/// member, or the seed tool and who ran it.
fn actor_text(actor_id: Uuid, actor_email: &str, source: &str, account_id: Option<Uuid>) -> String {
    if source == "seed" {
        format!("The seed tool, run by {actor_email},")
    } else if source == "auto" {
        "Buwiz".to_owned()
    } else if Some(actor_id) == account_id {
        "The person".to_owned()
    } else {
        format!("Staff {actor_email}")
    }
}

/// An audit line as the history reads it. Staff actions name the staff
/// member and carry their reason.
fn audit_text(line: &AuditRow, account_id: Uuid) -> String {
    let tin = &line.tin_masked;
    let file = line.document_kind.as_deref().map(DocumentKind::label).unwrap_or_default();
    let what = match line.action.as_str() {
        "deleted" if line.claim_status == "unverified" => format!("deleted {tin}, saved without papers"),
        "deleted" => {
            let status = match line.claim_status.as_str() {
                "pending" => "waiting",
                "needs_fix" => "sent back",
                other => other,
            };
            format!("deleted a {status} claim on {tin}")
        }
        "removed" => format!("removed {tin}"),
        "revoked" => format!("revoked the ownership of {tin}"),
        "file_replaced" => format!("replaced the {file} on {tin}"),
        "activated" => format!("activated {tin} without papers"),
        "auto_activated" => format!("activated {tin} automatically"),
        "auto_confirmed" => format!("confirmed the automatic activation of {tin}"),
        "paused" => format!("paused {tin}"),
        "resumed" => format!("resumed {tin}"),
        _ => format!("removed the {file} from {tin}"),
    };
    let who = actor_text(line.actor_id, &line.actor_email, &line.source, Some(account_id));
    match &line.reason {
        Some(reason) => format!("{who} {what}: {reason}"),
        None => format!("{who} {what}"),
    }
}

fn account_audit_icon(action: &str) -> &'static str {
    match action {
        "banned" => "block",
        "unbanned" => "lock_open",
        "role_changed" => "shield_person",
        "test_marked" | "test_unmarked" => "science",
        "signed_out" | "app_signed_out" => "logout",
        "created" => "person_add",
        _ => "draft",
    }
}

/// An `account_audit` line as the history reads it, with its reason.
fn account_audit_text(line: &AccountAuditRow) -> String {
    let what = match line.action.as_str() {
        "banned" => "banned the account".to_owned(),
        "unbanned" => "lifted the ban".to_owned(),
        "role_changed" => format!(
            "changed the role from {} to {}",
            roles::role_label(line.old_value.as_deref()),
            roles::role_label(line.new_value.as_deref())
        ),
        "test_marked" => "marked it as a test account".to_owned(),
        "test_unmarked" => "removed the test label".to_owned(),
        "signed_out" => "signed the account out everywhere".to_owned(),
        "app_signed_out" => match line.new_value.as_deref().and_then(crate::account::oauth::client_name) {
            Some(name) => format!("signed the {name} app out"),
            None => "signed an app or agent out".to_owned(),
        },
        "created" => "created the login".to_owned(),
        other => other.replace('_', " "),
    };
    let who = actor_text(line.actor_id, &line.actor_email, &line.source, None);
    match &line.reason {
        Some(reason) => format!("{who} {what}: {reason}"),
        None => format!("{who} {what}"),
    }
}

/// The required reason of a staff form, kept in the account's history.
#[component]
async fn reason_field() -> Result<impl View> {
    Ok(view! {
        <label class="decision-label">
            "Reason"
            <textarea name="reason" rows="2" maxlength="500" required=(true) placeholder="Kept in the account's history."></textarea>
        </label>
    })
}
