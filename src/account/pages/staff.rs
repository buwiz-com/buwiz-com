//! Staff pages under `/staff`, in the staff console's shell
//! ([`shell::document`]): the Overview, the review queue with the selected
//! claim in the inspector, one claim's full review, every account with the
//! selected one in the inspector, one account in tabs, and search
//! everything. Signed-in people who are not staff get a 404 on all of them
//! ([`staff_member`]). Selection is in the URL (`?open=`, `?tab=`), so every
//! state works without script; staff.js swaps the inspector in place.

use super::*;
use crate::account::claims::FIX_REASONS;
use crate::account::roles::Role;
use crate::account::staff::{
    self as queries, AccountAuditRow, AppRow, AuditRow, IdStatus, QueueFilter, QueueRow, UserFilter,
};
use crate::ocr::extract::{self, Kind, Verdict};
use crate::ocr::{self, Reading};
use topcoat::view::{Unescaped, ViewExt};
use ui::{
    act, act_buttons, avatar, flag_pill, glyph, href, ic, icon_with, kind_icon, page_head, reason_act, reason_field,
    short_span, status_pill, sx_notice, sx_pill,
};

mod overview;
mod search;
pub(crate) mod shell;
pub(crate) mod ui;

pub use overview::overview_page;
pub use search::search_page;

/// A small note on a queue row or in the checks list: (tone, icon, text).
/// Tones: `good`, `warn`, `bad`, `info`.
type Flag = (&'static str, &'static str, String);

/// The flag on a TIN an Admin activated for themselves.
const SELF_ACTIVATED: &str = "Activated by its owner, an admin";

/// What a reviewer should notice first about a claim. These are plain
/// checks on the claim's rows; they help look, staff still decide.
fn queue_flags(row: &QueueRow) -> Vec<Flag> {
    let claim = &row.claim;
    let has = |kind: &str| row.document_kinds.iter().any(|stored| stored == kind);
    let mut flags = Vec::new();
    if row.test_account {
        flags.push(("info", "science", "Test".to_owned()));
    }
    if claim.tin().is_test() {
        flags.push(("info", "science", "Test TIN: never sends".to_owned()));
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
    if claim.self_activated() {
        flags.push(("warn", "how_to_reg", SELF_ACTIVATED.to_owned()));
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

/// How serious a flag's tone is: `bad` first, then `warn`, `info`, `good`.
fn severity(tone: &str) -> u8 {
    match tone {
        "bad" => 0,
        "warn" => 1,
        "info" => 2,
        _ => 3,
    }
}

/// A queue row's flags as the row shows them, most serious first: every
/// `bad` one on the row itself (the most serious one when none is), the
/// rest read aloud and named in the "+n" tip, so a red warning never hides
/// behind a green one and the row stays one line beside the inspector.
fn row_flags(mut flags: Vec<Flag>) -> (Vec<Flag>, Vec<Flag>) {
    flags.sort_by_key(|(tone, ..)| severity(tone));
    let bad = flags.iter().filter(|(tone, ..)| *tone == "bad").count();
    let rest = flags.split_off(bad.max(1).min(flags.len()));
    (flags, rest)
}

/// The `?open=` of the request: the row the inspector shows.
fn open_id(cx: &Cx) -> Option<Uuid> {
    query(cx, "open").and_then(|raw| raw.parse().ok())
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
    let id = format!("search-{}", action.trim_start_matches('/').replace('/', "-"));
    Ok(view! {
        <form class="sx-search" method="get" action=(action) role="search">
            for (name, kept) in keep {
                <input type="hidden" name=(name) value=(kept)>
            }
            (ic("search"))
            <label class="sx-vh" for=(id.clone())>(label)</label>
            <input id=(id) type="search" name="q" value=(value) placeholder=(placeholder)>
        </form>
    })
}

/// The inspector: the right-hand panel of a list. On desktop it is always
/// there, showing `?open=` or the list's first row; on a tablet it slides
/// over the list and on a phone it is a sheet, both only once a row is
/// opened (`open`). `close` is the list without the selection.
#[component]
async fn inspector(
    title: &'static str,
    open: bool,
    close: String,
    #[default] child: topcoat::view::Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <aside id="inspector" class=(if open { "sx-insp is-open" } else { "sx-insp" }) aria-label=(title) aria-labelledby="inspector-title">
            <span class="sx-grab" aria-hidden="true"></span>
            <header class="sx-insp-h">
                <h2 id="inspector-title">(title)</h2>
                <a class="sx-ib" href=(close.clone()) data-close-insp="" aria-label="Close">(ic("x"))</a>
            </header>
            (child)
        </aside>
        <a class=(if open { "sx-scrim is-open" } else { "sx-scrim" }) href=(close) data-close-insp="" tabindex="-1" aria-hidden="true"></a>
    })
}

// ---------------------------------------------------------------- the review queue

/// A list row's classes: `sel` for the row in the inspector, and `auto`
/// when nobody chose it (the list's first row), which phones do not mark.
fn row_class(selected: bool, explicit: bool) -> &'static str {
    match (selected, explicit) {
        (true, true) => "sel",
        (true, false) => "sel auto",
        _ => "",
    }
}

/// What the "When" column says about a claim, and whether it is late.
fn queue_age(claim: &ClaimRow) -> (String, bool) {
    match claim.status.as_str() {
        "pending" => {
            let since = claim.waiting_since();
            (short_span(since), OffsetDateTime::now_utc() - since > time::Duration::days(1))
        }
        "needs_fix" => (format!("Sent back {}", ago(claim.sent_back_at)), false),
        "unverified" => (format!("Saved {}", short_date(claim.created_at)), false),
        "removed" | "revoked" => (format!("Ended {}", claim.removed_at.map(short_date).unwrap_or_default()), false),
        _ => (format!("Decided {}", claim.decided_at.map(short_date).unwrap_or_default()), false),
    }
}

#[page("/staff/queue")]
pub async fn claims_page(cx: &Cx) -> Result<impl View> {
    let reviewer = staff_member(cx, Power::Review).await?;
    let filter = QueueFilter::parse(query(cx, "status").as_deref());
    let search = query(cx, "q").unwrap_or_default();
    let opened = open_id(cx);
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
            (tab, href("/staff/queue", &[("status", tab.code()), ("q", &search)]), count)
        })
        .collect();
    let (main_tabs, other_tabs): (Vec<_>, Vec<_>) = tabs
        .into_iter()
        .partition(|(tab, ..)| matches!(tab, QueueFilter::Waiting | QueueFilter::SentBack | QueueFilter::Auto));
    let other_on = !matches!(filter, QueueFilter::Waiting | QueueFilter::SentBack | QueueFilter::Auto);
    let close = href("/staff/queue", &[("status", filter.code()), ("q", &search)]);
    let selected = opened.or_else(|| rows.first().map(|row| row.claim.id));
    let review = match selected {
        Some(claim_id) => load_claim(cx, &reviewer, claim_id).await?,
        None => None,
    };
    let selected = review.as_ref().map(|view| view.claim.id);
    let explicit = opened.is_some();
    // The first row, shown because nobody chose one, comes without its
    // decision: the way to it is "Review this claim", to `?open=`.
    let preview = selected.filter(|_| !explicit).map(|claim_id| {
        href("/staff/queue", &[("status", filter.code()), ("q", &search), ("open", &claim_id.to_string())])
    });
    let lines: Vec<(QueueRow, String, bool)> = rows
        .into_iter()
        .map(|row| {
            let open =
                href("/staff/queue", &[("status", filter.code()), ("q", &search), ("open", &row.claim.id.to_string())]);
            let on = Some(row.claim.id) == selected;
            (row, open, on)
        })
        .collect();
    let empty = lines.is_empty();
    let subtitle = match filter {
        QueueFilter::Waiting => format!("{waiting} waiting · oldest first"),
        _ => format!("{} · {}", filter.label(), if oldest_first { "oldest first" } else { "newest first" }),
    };
    Ok(view! {
        <main id="main" class="sx-main" tabindex="-1">
            page_head(title: "Review queue", sub: subtitle)
            <div class="sx-notices">sx_notice(notice: notice(cx))</div>
            <div class="sx-scroll">
                <div class="sx-stack">
                    <div class="sx-tabrow">
                        <nav class="sx-tabs" aria-label="Claim status">
                            for (tab, link, count) in main_tabs {
                                <a class="sx-tab" href=(link) if tab == filter { aria-current="page" }>
                                    (tab.label())
                                    if let Some(count) = count {
                                        <span class=(if tab == QueueFilter::Waiting && count > 0 { "sx-count hot" } else { "sx-count" })>(count.to_string())</span>
                                    }
                                </a>
                            }
                        </nav>
                        <details class="sx-moretabs">
                            <summary class=(if other_on { "sx-tab on-more" } else { "sx-tab" }) if other_on { aria-current="page" }>
                                (if other_on { filter.label() } else { "More" })
                                (ic("chevron-down"))
                            </summary>
                            <nav class="sx-menu" aria-label="More claim lists">
                                for (tab, link, _) in other_tabs {
                                    <a href=(link) if tab == filter { aria-current="page" }>(tab.label())</a>
                                }
                            </nav>
                        </details>
                    </div>
                    <div class="sx-toolbar">
                        search_box(action: "/staff/queue", label: "Search claims", placeholder: "Search TIN, name or email", value: search.clone(), keep: vec![("status", filter.code())])
                    </div>
                    if empty {
                        <p class="sx-card sx-empty big">(if search.is_empty() { "Nothing here. All caught up." } else { "No claims match that search." })</p>
                    } else {
                        <div class="sx-card sx-tablewrap">
                            <table class="sx-table">
                                <caption class="sx-vh">(format!("{} claims", filter.label()))</caption>
                                <colgroup><col class="c-tin"><col><col class="sx-col-wide"><col class="c-age"><col class="c-checks"></colgroup>
                                <thead>
                                    <tr>
                                        <th scope="col">"TIN"</th>
                                        <th scope="col">"Name"</th>
                                        <th scope="col" class="sx-col-wide">"Email"</th>
                                        <th scope="col">(if filter == QueueFilter::Waiting { "Waiting" } else { "When" })</th>
                                        <th scope="col">"Checks"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    #[key(line.0.claim.id.as_u128())]
                                    for line in lines {
                                        queue_row(row: line.0, open: line.1, selected: line.2, explicit: explicit)
                                    }
                                </tbody>
                            </table>
                        </div>
                    }
                    <p class="sx-fine">"Checks come from simple tests on each claim. They help you look; you still decide."</p>
                </div>
            </div>
        </main>
        inspector(title: "Claim", open: opened.is_some() && review.is_some(), close: close,
            if let Some(view) = review {
                claim_body(view: view, wide: false, review: preview)
            } else {
                <div class="sx-insp-b"><p class="sx-empty">"Nothing to review in this tab."</p></div>
            }
        )
    }
    .boxed())
}

#[component]
async fn queue_row(row: QueueRow, open: String, selected: bool, explicit: bool) -> Result<impl View> {
    let (shown, rest) = row_flags(queue_flags(&row));
    let claim = row.claim;
    let (age, late) = queue_age(&claim);
    let rest: Vec<String> = rest.into_iter().map(|(_, _, text)| text).collect();
    Ok(view! {
        <tr data-id=(claim.id.to_string()) class=(row_class(selected, explicit))>
            <td class="a-main">
                <span class="sx-cell-tin">
                    kind_icon(kind: claim.kind.clone())
                    <a class="sx-rowlink" href=(open) data-open="" if selected { aria-current="true" }>(claim.tin().to_string())</a>
                </span>
            </td>
            <td class="b cut a-sub">(claim.registered_name.clone())</td>
            <td class="m cut sx-col-wide">(claim.email.clone())</td>
            <td class="a-side">
                <span class=(if late { "sx-age late" } else { "sx-age" })>
                    (ic("clock"))
                    (age)
                </span>
            </td>
            <td class="a-foot">
                <span class="sx-flags">
                    for item in shown {
                        flag_pill(item: item)
                    }
                    if !rest.is_empty() {
                        <span class="sx-more" title=(rest.join(" · ")) aria-hidden="true">(format!("+{}", rest.len()))</span>
                        <span class="sx-vh">(rest.join(". "))</span>
                    }
                </span>
            </td>
        </tr>
    })
}

// ---------------------------------------------------------------- one claim

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
    let tag = if from_pdf { "Read from the PDF on our server" } else { "Read by OCR on our server" };
    Ok(view! {
        <section class="sx-sub" aria-labelledby="sx-compare">
            <div class="sx-subh">
                <h3 id="sx-compare">"Typed vs. on the document"</h3>
                <span class="sx-fine">(tag)</span>
            </div>
            if let Some(reason) = failed {
                <p class="sx-notice error">(ic("alert")) <span>"Couldn't read: " (reason)</span></p>
            }
            <div class="sx-cmp">
                <span></span>
                <span class="h">"Typed"</span>
                <span class="h">"On the document"</span>
                <span></span>
                for row in rows {
                    <span class="l">(row.label)</span>
                    if let Some(typed) = row.typed {
                        <span>(typed)</span>
                    } else {
                        <span class="u">"Not asked"</span>
                    }
                    if let Some(waiting) = waiting {
                        <span class="u">(waiting)</span>
                    } else if let Some(read) = row.read {
                        <span>
                            (read)
                            if let Some(sure) = row.confidence {
                                <em>(format!("{:.0}% sure", sure * 100.0))</em>
                            }
                        </span>
                    } else {
                        <span class="u">"Not found"</span>
                    }
                    match row.verdict {
                        Some(Verdict::Match) => <span class="mk good" title="Matches">(ic("check-circle"))<span class="sx-vh">"Matches"</span></span>,
                        Some(Verdict::Partial) => <span class="mk warn" title="Partly matches">(ic("warning"))<span class="sx-vh">"Partly matches"</span></span>,
                        Some(Verdict::Mismatch) => <span class="mk bad" title="Differs">(ic("warning"))<span class="sx-vh">"Differs"</span></span>,
                        None => <span class="mk">(ic("more"))</span>,
                    }
                }
            </div>
        </section>
    })
}

/// Everything a claim's review shows and may offer, read once for the
/// inspector or the claim's own page.
struct ClaimView {
    claim: ClaimRow,
    documents: Vec<DocumentRow>,
    empty_viewer: String,
    other_accounts: Vec<ClaimRow>,
    joined: String,
    claimant_tins: usize,
    claimant_banned: bool,
    checks: Vec<Flag>,
    compare: Comparison,
    status: (&'static str, &'static str, &'static str),
    age: Option<String>,
    late: bool,
    pauses: Vec<claims::PauseLine>,
    admin_must_approve: bool,
    paused_by: String,
    paused_by_buwiz: bool,
    confirm_needs_admin: bool,
    own_claim: bool,
    own_withheld: bool,
    can_confirm: bool,
    auto_sure: Option<(String, String)>,
    auto_doubts: Vec<String>,
    can_pause: bool,
    fix_reasons: Vec<&'static str>,
    show_delete: bool,
    show_revoke: bool,
    ended: bool,
    unverified: bool,
    can_activate: bool,
}

/// Staff decide on their own claim only as an Admin (the audit line shows it
/// was their own); Support leave it to someone else.
fn own_claim_of(claim: &ClaimRow, reviewer: &Staff) -> bool {
    claim.account_id == reviewer.account.id
}

/// Reads [`ClaimView`] for `claim_id`, or `None` when there is no such claim.
async fn load_claim(cx: &Cx, reviewer: &Staff, claim_id: Uuid) -> Result<Option<ClaimView>> {
    let db = &state(cx).db;
    let Some(claim) = claims::find(db, claim_id).await? else {
        return Ok(None);
    };
    let documents = claims::documents(db, claim_id).await?;
    let rivals = claims::rivals(db, &claim).await?;
    let claimant = queries::user(db, claim.account_id).await?;
    let claimant_tins = claims::for_account(db, claim.account_id)
        .await?
        .iter()
        .filter(|other| crate::filing::gate::is_live(&other.status))
        .count();
    let kind = ClaimKind::parse(&claim.kind).unwrap_or(ClaimKind::Business);
    let stored =
        |document: DocumentKind| documents.iter().any(|row| row.kind == document.as_str() && row.purged_at.is_none());

    let mut checks: Vec<Flag> = Vec::new();
    let other_accounts: Vec<ClaimRow> =
        rivals.into_iter().filter(|rival| rival.account_id != claim.account_id).collect();
    if other_accounts.is_empty() {
        checks.push(("good", "person_check", "No other account claims this TIN".to_owned()));
    } else {
        checks.push(("bad", "group", format!("{} other account(s) claim this TIN", other_accounts.len())));
    }
    if claim.self_activated() {
        checks.push(("warn", "how_to_reg", SELF_ACTIVATED.to_owned()));
    }
    if claim.tin().is_test() {
        checks.push(("info", "science", "Test TIN: Buwiz never sends returns for it".to_owned()));
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
                for (optional, label) in [(DocumentKind::Selfie, "selfie"), (DocumentKind::NationalId, "National ID")] {
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
    let status = status_look(claim.shown_status());
    let pauses = claims::pauses(db, &claim.tin()).await?;
    // Opening sending again on a TIN that was paused is an Admin's call.
    let admin_must_approve = !reviewer.can(Power::Pause) && claims::paused_ever(db, &claim.tin()).await?;
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
    // Only an Admin reopens a paused TIN, and lifting Buwiz's own pause
    // takes a reason.
    let paused_by_buwiz = claim.paused_at.is_some() && paused_by == "Buwiz";
    let confirm_needs_admin = claim.paused_at.is_some() && !reviewer.can(Power::Pause);
    let own_claim = own_claim_of(&claim, reviewer);
    let own_withheld = own_claim && reviewer.role != Role::Admin;
    let can_confirm = auto_waiting && reviewer.can(Power::ConfirmAuto) && !own_withheld && !confirm_needs_admin;
    let auto_sure = claim.auto_check.as_ref().map(|check| {
        let percent = |field: &str| format!("{:.0}%", check["confidence"][field].as_f64().unwrap_or(0.0) * 100.0);
        (percent("tin"), percent("name"))
    });
    let auto_doubts = crate::account::verify::doubt_words(claim.auto_check.as_ref());
    let can_pause = reviewer.can(Power::Pause) && claim.status == "approved";
    let age = (claim.status == "pending").then(|| format!("Waiting {}", span_since(claim.waiting_since())));
    let late = claim.status == "pending" && OffsetDateTime::now_utc() - claim.waiting_since() > time::Duration::days(1);
    let fix_reasons: Vec<&'static str> =
        claim.fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(label, _)| label).collect();
    let show_delete = claims::DELETABLE.contains(&claim.status.as_str()) && reviewer.can(Power::DeleteClaim);
    let show_revoke = claim.status == "approved" && reviewer.can(Power::EndOwnership);
    let ended = matches!(claim.status.as_str(), "removed" | "revoked");
    let unverified = claim.status == "unverified";
    let claimant_banned = claimant.as_ref().is_some_and(|user| user.banned_at.is_some());
    let can_activate = reviewer.can(Power::ActivateWithoutPapers)
        && matches!(claim.status.as_str(), "unverified" | "pending" | "needs_fix");
    let empty_viewer = if unverified {
        format!("No papers yet. Added without the upload on {}.", date(claim.created_at))
    } else {
        "No files on this claim.".to_owned()
    };
    Ok(Some(ClaimView {
        joined: claimant.map(|user| short_date(user.created_at)).unwrap_or_default(),
        claim,
        documents,
        empty_viewer,
        other_accounts,
        claimant_tins,
        claimant_banned,
        checks,
        compare,
        status,
        age,
        late,
        pauses,
        admin_must_approve,
        paused_by,
        paused_by_buwiz,
        confirm_needs_admin,
        own_claim,
        own_withheld,
        can_confirm,
        auto_sure,
        auto_doubts,
        can_pause,
        fix_reasons,
        show_delete,
        show_revoke,
        ended,
        unverified,
        can_activate,
    }))
}

/// A claim's review: in the inspector (`wide: false`, one column with the
/// thumb bar a phone's sheet shows) or on its own page (`wide: true`, the
/// papers beside the review). Every decision and action form posts to the
/// claim's routes, as before.
///
/// `review` is set when nobody chose the claim (the queue's first row in
/// the inspector): it shows the facts and the papers but no decision or
/// other action, so no click lands on Approve for a claim the reviewer did
/// not pick; in their place one button, "Review this claim", goes to
/// `review` (the list with `?open=`), where the controls are.
#[component]
async fn claim_body(view: ClaimView, wide: bool, #[default] review: Option<String>) -> Result<impl View> {
    let ClaimView {
        claim,
        documents,
        empty_viewer,
        other_accounts,
        joined,
        claimant_tins,
        claimant_banned,
        checks,
        compare,
        status,
        age,
        late,
        pauses,
        admin_must_approve,
        paused_by,
        paused_by_buwiz,
        confirm_needs_admin,
        own_claim,
        own_withheld,
        can_confirm,
        auto_sure,
        auto_doubts,
        can_pause,
        fix_reasons,
        show_delete,
        show_revoke,
        ended,
        unverified,
        can_activate,
    } = view;
    let claim_id = claim.id;
    let tin = claim.tin().to_string();
    let (status_label, status_icon, status_tone) = status;
    let pending = claim.status == "pending";
    let decide = format!("/staff/claims/{claim_id}/decision");
    let confirm = format!("/staff/claims/{claim_id}/confirm");
    let files = viewer_files(documents);
    let kind = claim.kind.clone();
    let (email, account_id) = (claim.email.clone(), claim.account_id);
    let preview = review.is_some();
    let review = review.unwrap_or_default();
    let decides = claim.is_open() && !own_withheld && !preview;
    // In a preview the button stands where the decision would; for a claim
    // with no decision to make, at the end of the review.
    let review_inline = preview && claim.is_open() && !own_withheld;
    let review_at_end = preview && !review_inline;
    let can_confirm = can_confirm && !preview;
    let can_activate = can_activate && !preview;
    let show_revoke = show_revoke && !preview;
    let show_delete = show_delete && !preview;

    let tin_head = tin.clone();
    let pills = view! {
        <div class="sx-pills">
            sx_pill(label: ClaimKind::label(&kind), sym: if kind == "personal" { "person" } else { "store" }, tone: "neutral")
            if let Some(age) = age {
                sx_pill(label: age, sym: "clock", tone: if late { "bad" } else { "warn" })
            } else {
                sx_pill(label: status_label, sym: glyph(status_icon), tone: ui::status_tone(status_tone))
            }
        </div>
    }
    .boxed();
    let rivals = view! {
        if !other_accounts.is_empty() {
            <div class="sx-notice error" role="note">
                (ic("users"))
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
    }
    .boxed();
    let who = view! {
        <div class="sx-who">
            avatar(email: email.clone())
            <span class="x">
                <b>(email)</b>
                <span>
                    "Joined " (joined) " · " (if claimant_tins <= 1 { "first TIN".to_owned() } else { format!("{claimant_tins} TINs") })
                </span>
            </span>
            <a class="go" href=(format!("/staff/users/{account_id}"))>"View user" (icon_with("chevron-right", "ic sm"))</a>
        </div>
    }
    .boxed();
    let sections = view! {
        if !unverified {
            typed_vs_document(comparison: compare)
        }
        if !checks.is_empty() {
            <section class="sx-sub" aria-labelledby="sx-checks">
                <h3 id="sx-checks">"Checks"</h3>
                <div class="sx-checks">
                    for (tone, sym, text) in checks {
                        <span class=(format!("sx-check {tone}"))>(ic(glyph(sym))) <span>(text)</span></span>
                    }
                </div>
            </section>
        }
        if !auto_doubts.is_empty() {
            <section class="sx-sub">
                <h3>"Not activated automatically"</h3>
                <ul class="sx-text">
                    for words in auto_doubts {
                        <li>(words)</li>
                    }
                </ul>
            </section>
        }
        if claim.verified_by.as_deref() == Some("auto") {
            <section class="sx-sub">
                <h3>"Activated automatically"</h3>
                <p class="sx-text">
                    (claim.decided_at.map(date_time).unwrap_or_default()) ". TIN and name matched the TIN ID read on our server"
                    if let Some((tin_sure, name_sure)) = auto_sure.clone() {
                        " (" (tin_sure) " and " (name_sure) " sure)"
                    }
                    "."
                </p>
                if let Some(at) = claim.auto_confirmed_at {
                    <p class="sx-text">"Confirmed by staff " (date_time(at)) "; its papers were deleted."</p>
                } else {
                    <p class="sx-text">"No person has confirmed it yet. Check the papers, then confirm. They are deleted when you confirm; until then they are kept, even if the TIN is removed or revoked."</p>
                    if can_confirm && paused_by_buwiz {
                        <div class="sx-acts">
                            reason_act(
                                id: format!("act-confirm-{claim_id}"),
                                label: "Looks right: confirm and lift the pause",
                                sym: "check",
                                tone: "pri",
                                action: confirm.clone(),
                                question: format!("Confirm {tin} for {}?", claim.email),
                                detail: "You checked the papers and they match. They are deleted now, and the pause Buwiz put on it is lifted: the owner can send again at once.",
                                confirm: "Confirm",
                                danger: false
                            )
                        </div>
                    } else if can_confirm && own_claim {
                        <div class="sx-acts">
                            reason_act(
                                id: format!("act-confirm-{claim_id}"),
                                label: "Looks right: confirm",
                                sym: "check",
                                tone: "pri",
                                action: confirm.clone(),
                                question: format!("Confirm your own TIN {tin}?"),
                                detail: "You checked the papers and they match. They are deleted now. This is your own TIN: write why; it goes in your history.",
                                confirm: "Confirm",
                                danger: false
                            )
                        </div>
                    } else if can_confirm {
                        <form class="sx-acts" method="post" action=(confirm.clone()) data-confirm=(format!("Confirm {tin} for {}?", claim.email)) data-confirm-detail="You checked the papers and they match. They are deleted now." data-confirm-label="Confirm" data-confirm-tone="danger">
                            <button class="sx-btn pri" type="submit">(ic("check")) "Looks right: confirm"</button>
                        </form>
                    } else if own_withheld {
                        <p class="sx-text">"This is your own TIN. Another staff member confirms it."</p>
                    } else if confirm_needs_admin {
                        <p class="sx-text">"Sending for this TIN is paused, so an admin confirms it."</p>
                    }
                }
            </section>
        }
        if claim.sent_back_at.is_some() {
            <section class="sx-sub">
                <h3>"Sent back before"</h3>
                <p class="sx-text">
                    (claim.sent_back_at.map(date_time).unwrap_or_default())
                    if !fix_reasons.is_empty() {
                        ": " (fix_reasons.join(", "))
                    }
                </p>
                if let Some(note) = claim.fix_note.clone() {
                    <p class="sx-quote">(note)</p>
                }
                if let Some(resent) = claim.resent_at {
                    <p class="sx-text">"Resent " (date_time(resent))</p>
                } else {
                    <p class="sx-text">"Waiting for the person's fix."</p>
                }
            </section>
        }
        if own_withheld && claim.is_open() {
            <p class="sx-notice info">(ic("info")) <span>"This is your own claim. Another staff member decides it."</span></p>
        } else if review_inline {
            review_this_claim(review: review.clone(), open: true)
        } else if claim.is_open() {
            <form class="sx-sub sx-form" id="decision" method="post" action=(decide)>
                <h3>"Decision"</h3>
                if own_claim {
                    <p class="sx-text">"This is your own claim. Your decision goes in your history as one on your own account; to approve it, write why in the note."</p>
                }
                if admin_must_approve {
                    <p class="sx-text">"Sending for this TIN was paused before: an admin must approve it. You can still send it back or reject it."</p>
                } else {
                    <button class="sx-btn pri block sx-in-thumb" type="submit" name="decision" value="approve">
                        (ic("check"))
                        <span class="sx-ellipsis">"Approve: " (claim.email.clone()) " owns this TIN"</span>
                    </button>
                }
                if pending {
                    <fieldset class="sx-sub sx-plain">
                        <legend class="sx-text">"Or send it back with a reason"</legend>
                        <div class="sx-reasons">
                            for (code, label, _) in FIX_REASONS {
                                <label class="sx-reason">
                                    <input type="checkbox" name="reasons" value=(*code)>
                                    <span>(*label)</span>
                                </label>
                            }
                        </div>
                    </fieldset>
                }
                <label class="sx-field">
                    <span>"Note to the person"</span>
                    <textarea name="note" rows="3" maxlength="500" placeholder="They see this on their My TINs page. Required to reject."></textarea>
                </label>
                <div class="sx-decide sx-in-thumb">
                    if pending {
                        <button class="sx-btn" type="submit" name="decision" value="fix">"Ask for a fix"</button>
                    }
                    <button class="sx-btn dan" type="submit" name="decision" value="reject">"Reject"</button>
                </div>
            </form>
        } else if unverified {
            <section class="sx-sub">
                <h3>"Not verified"</h3>
                <p class="sx-text">"The person can prepare returns but not send them."</p>
            </section>
        } else if ended {
            <section class="sx-sub">
                <h3>"Ended"</h3>
                <p class="sx-text">"Approved " (claim.decided_at.map(date_time).unwrap_or_default())</p>
                <p class="sx-text">
                    (if claim.status == "removed" { "Removed by the owner " } else { "Revoked by staff " })
                    (claim.removed_at.map(date_time).unwrap_or_default())
                </p>
                if let Some(note) = claim.removal_note.clone() {
                    <p class="sx-quote">(note)</p>
                }
            </section>
        } else {
            <section class="sx-sub">
                <h3>"Decided"</h3>
                <p class="sx-text">(status_label) " · " (claim.decided_at.map(date_time).unwrap_or_default())</p>
                if let Some(note) = claim.decision_note.clone() {
                    <p class="sx-quote">(note)</p>
                }
            </section>
        }
        if !pauses.is_empty() {
            <section class="sx-sub">
                <h3>"Paused before"</h3>
                for line in pauses {
                    <p class="sx-text">
                        (if line.action == "paused" { "Paused by " } else { "Resumed by " })
                        (if line.source == "auto" { "Buwiz".to_owned() } else { line.actor_email.clone() }) " on " (date(line.created_at))
                        if let Some(reason) = line.reason.clone() {
                            ": " (reason)
                        }
                    </p>
                }
            </section>
        }
        if can_pause && (claim.paused_at.is_some() || !preview) {
            <section class="sx-sub">
                if let Some(at) = claim.paused_at {
                    <h3>"Paused"</h3>
                    <p class="sx-text">"Paused by " (paused_by.clone()) " on " (date(at)) ": " (claim.pause_reason.clone().unwrap_or_default())</p>
                    if !preview {
                        <div class="sx-acts">
                            reason_act(
                                id: format!("act-resume-{claim_id}"),
                                label: "Resume sending",
                                sym: "play",
                                action: format!("/staff/claims/{claim_id}/resume"),
                                question: format!("Resume sending for {tin}?"),
                                detail: "The owner can send returns again at once.",
                                confirm: "Resume",
                                danger: false
                            )
                        </div>
                    }
                } else {
                    <h3>"Pause sending"</h3>
                    <p class="sx-text">"Keeps the TIN on this account but stops every send at once. The owner sees your reason on My TINs."</p>
                    <div class="sx-acts">
                        reason_act(
                            id: format!("act-pause-{claim_id}"),
                            label: "Pause sending",
                            sym: "pause",
                            action: format!("/staff/claims/{claim_id}/pause"),
                            question: format!("Pause sending for {tin}?"),
                            detail: "Tickets and sends are refused until you resume. Returns already sent are not affected.",
                            confirm: "Pause"
                        )
                    </div>
                }
            </section>
        }
        if can_activate {
            <section class="sx-sub">
                <h3>(if unverified { "Activate without papers" } else { "Or activate without papers" })</h3>
                if claimant_banned {
                    <p class="sx-text">(Notice::ClaimantBanned.message())</p>
                } else {
                    <p class="sx-text">"Makes " (claim.email.clone()) " the owner of " (tin.clone()) " now, without papers. Use it for accounts you know. The reason goes in the account's history."</p>
                    <div class="sx-acts">
                        reason_act(
                            id: format!("act-activate-{claim_id}"),
                            label: "Activate without papers",
                            sym: "check-circle",
                            action: format!("/staff/claims/{claim_id}/activate"),
                            question: format!("Activate {tin} for {} without papers?", claim.email),
                            detail: "The TIN becomes active at once and can send returns to the BIR. Your reason is kept in the account's history.",
                            confirm: "Activate",
                            danger: false
                        )
                    </div>
                }
            </section>
        }
        if show_delete && own_claim {
            <p class="sx-notice info">(ic("info")) <span>"This is your own claim: delete it from My TINs."</span></p>
        } else if show_revoke {
            <section class="sx-sub">
                <h3>"Revoke ownership"</h3>
                <p class="sx-text">"Ends this account's ownership. The TIN stops being active at once, its sent returns stay, and it can be claimed again."</p>
                <div class="sx-acts">
                    reason_act(
                        id: format!("act-revoke-{claim_id}"),
                        label: "Revoke ownership",
                        sym: "ban",
                        tone: "dan",
                        action: format!("/staff/claims/{claim_id}/revoke"),
                        question: format!("Revoke {}'s ownership of {}?", claim.email, claim.tin()),
                        detail: "The TIN stops being active at once; the owner sees your reason on My TINs.",
                        confirm: "Revoke"
                    )
                </div>
            </section>
        } else if show_delete {
            <section class="sx-sub">
                <h3>"Delete claim"</h3>
                <p class="sx-text">"Erases the claim and its files for good. Only a line in the account's history stays."</p>
                <div class="sx-acts">
                    reason_act(
                        id: format!("act-delete-{claim_id}"),
                        label: "Delete claim",
                        sym: "trash",
                        tone: "dan",
                        action: format!("/staff/claims/{claim_id}/delete"),
                        question: format!("Delete this claim by {}?", claim.email),
                        detail: "The claim and every file on it are erased for good.",
                        confirm: "Delete"
                    )
                </div>
            </section>
        }
        if review_at_end {
            review_this_claim(review: review, open: false)
        }
    }
    .boxed();
    let has_files = !files.is_empty();
    let listed = files.clone();
    let viewer = view! { document_viewer(files: files, empty: empty_viewer) }.boxed();
    let file_rows = view! {
        if has_files {
            file_list(files: listed)
        }
    }
    .boxed();
    Ok(view! {
        if wide {
            <div class="sx-stack">
                (rivals)
                <div class="sx-review">
                    (viewer)
                    <div class="sx-review-side">
                        (pills)
                        (who)
                        (sections)
                        (file_rows)
                    </div>
                </div>
            </div>
            if decides {
                <div class="sx-thumbbar page">
                    if !admin_must_approve {
                        <button class="sx-btn pri" type="submit" form="decision" name="decision" value="approve">(ic("check")) "Approve"</button>
                    }
                    if pending {
                        <button class="sx-btn" type="submit" form="decision" name="decision" value="fix">"Ask for a fix"</button>
                    }
                    <button class="sx-btn dan" type="submit" form="decision" name="decision" value="reject">"Reject"</button>
                </div>
            }
        } else {
            <div class="sx-insp-b">
                <div class="sx-detail">
                    <div class="sx-dhead">
                        <span class="sx-mono">(tin_head)</span>
                        (pills)
                    </div>
                    (rivals)
                    (viewer)
                    (file_rows)
                    (who)
                    (sections)
                    <a class="sx-btn block" href=(format!("/staff/claims/{claim_id}"))>(ic("external")) "Open full page"</a>
                </div>
            </div>
            if decides {
                <div class="sx-thumbbar">
                    if !admin_must_approve {
                        <button class="sx-btn pri" type="submit" form="decision" name="decision" value="approve">(ic("check")) "Approve"</button>
                    }
                    if pending {
                        <button class="sx-btn" type="submit" form="decision" name="decision" value="fix">"Ask for a fix"</button>
                    }
                    <button class="sx-btn dan" type="submit" form="decision" name="decision" value="reject">"Reject"</button>
                </div>
            }
        }
    })
}

/// In place of the decision and the actions of a claim nobody chose: the
/// one way to them.
#[component]
async fn review_this_claim(review: String, open: bool) -> Result<impl View> {
    Ok(view! {
        <section class="sx-sub sx-preview" aria-labelledby="sx-preview">
            <h3 id="sx-preview">(if open { "Decision" } else { "Actions" })</h3>
            <p class="sx-text">"The first claim in the list, shown until you choose one. Its decision and actions are on the claim itself."</p>
            <a class="sx-btn pri block" href=(review)>(ic("arrow-right")) "Review this claim"</a>
        </section>
    })
}

pub mod claim_id {
    use super::*;

    #[page("/staff/claims/{claim_id}")]
    pub async fn claim_page(cx: &Cx) -> Result<impl View> {
        let reviewer = staff_member(cx, Power::Review).await?;
        let claim_id = path_id(cx, 0)?;
        let Some(view) = load_claim(cx, &reviewer, claim_id).await? else {
            return Err(topcoat::router::error::not_found().into());
        };
        let (previous, next) = queries::neighbours(&state(cx).db, &view.claim).await?;
        let title = view.claim.tin().to_string();
        let sub = format!("{} · {}", view.claim.registered_name, view.claim.email);
        Ok(view! {
            <main id="main" class="sx-main" tabindex="-1">
                page_head(title: title, sub: sub, back: Some("/staff/queue".to_owned()), mono: true,
                    <nav class="sx-acts" aria-label="Queue">
                        if let Some(previous) = previous {
                            <a class="sx-btn sm" href=(format!("/staff/claims/{previous}")) aria-label="Previous claim">(ic("chevron-left"))</a>
                        }
                        if let Some(next) = next {
                            <a class="sx-btn sm" href=(format!("/staff/claims/{next}"))>"Next claim" (ic("chevron-right"))</a>
                        }
                    </nav>
                )
                <div class="sx-notices">sx_notice(notice: notice(cx))</div>
                <div class="sx-scroll">
                    claim_body(view: view, wide: true)
                </div>
            </main>
        }
        .boxed())
    }
}

/// One of a claim's files as the viewer and the file list name it.
#[derive(Clone)]
struct ViewerFile {
    document: DocumentRow,
    /// The kind, numbered when the claim has more than one of it: `TIN ID`,
    /// `TIN ID (2)`.
    label: String,
    icon: &'static str,
}

/// The claim's files in their order, each with its label.
fn viewer_files(documents: Vec<DocumentRow>) -> Vec<ViewerFile> {
    let mut of_kind: HashMap<String, usize> = HashMap::new();
    for document in &documents {
        *of_kind.entry(document.kind.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    documents
        .into_iter()
        .map(|document| {
            let nth = seen.entry(document.kind.clone()).or_default();
            *nth += 1;
            let kind = DocumentKind::label(&document.kind);
            let label =
                if *nth > 1 && of_kind[&document.kind] > 1 { format!("{kind} ({nth})") } else { kind.to_owned() };
            let symbol = glyph(DocumentKind::icon(&document.kind));
            ViewerFile { document, label, icon: symbol }
        })
        .collect()
}

/// `12 KB`.
fn kilobytes(document: &DocumentRow) -> String {
    format!("{} KB", (document.size_bytes + 1023) / 1024)
}

/// The claim's files, one at a time, for any number of them. Each file is a
/// radio button followed by its pane, so `:checked + pane` shows it without
/// script; each pane carries the row of tabs with its own tab marked, so the
/// row needs no rule per position. The radios are what keyboards and screen
/// readers use (the tab rows are for the pointer). Photos load only once
/// their pane shows (`loading="lazy"`): hidden panes and a closed inspector
/// fetch nothing. staff.js adds zoom and rotate for photos.
#[component]
async fn document_viewer(files: Vec<ViewerFile>, empty: String) -> Result<impl View> {
    let tabs: Vec<(usize, String, &'static str)> =
        files.iter().enumerate().map(|(index, file)| (index, file.label.clone(), file.icon)).collect();
    let count = files.len();
    Ok(view! {
        <section class="sx-viewer viewer" aria-label="Documents">
            if files.is_empty() {
                <p class="sx-viewer-empty">(empty)</p>
            }
            for (index, file) in files.into_iter().enumerate() {
                let document = file.document;
                let href = format!("/staff/documents/{}", document.id);
                let image = document.is_viewable_image();
                <input class="sx-viewer-radio" type="radio" name="document" id=(format!("document-{index}")) checked=(index == 0) aria-label=(format!("{} ({} of {count})", file.label, index + 1))>
                <div class="sx-viewer-pane">
                    <div class="sx-viewer-bar">
                        <div class="sx-viewer-tabs" aria-hidden="true">
                            for (other, label, icon_name) in tabs.clone() {
                                <label for=(format!("document-{other}")) class=(if other == index { "on" } else { "" })>(icon_with(icon_name, "ic sm")) (label)</label>
                            }
                        </div>
                        if image {
                            <div class="sx-viewer-tools" data-viewer-tools="" hidden=(true)>
                                <button type="button" data-zoom="-1" aria-label="Zoom out">(ic("zoom-out"))</button>
                                <button type="button" data-zoom="1" aria-label="Zoom in">(ic("zoom-in"))</button>
                                <button type="button" data-rotate="" aria-label="Rotate">(ic("rotate"))</button>
                            </div>
                        }
                    </div>
                    <div class="sx-viewer-stage">
                        if document.purged_at.is_some() {
                            <p class="sx-viewer-gone">"Deleted when the claim was decided or the file replaced."</p>
                        } else if image {
                            <img src=(href.clone()) alt=(file.label.clone()) data-zoomable="" loading="lazy">
                        } else {
                            <a class="sx-viewer-open" href=(href.clone()) target="_blank" rel="noopener">
                                (ic("file"))
                                "Open the file"
                            </a>
                        }
                    </div>
                    <p class="sx-viewer-foot">
                        <span>
                            (document.filename.clone()) " · " (kilobytes(&document))
                            (if document.purged_at.is_some() { " · deleted" } else { " · deleted automatically once you decide" })
                        </span>
                        if document.purged_at.is_none() {
                            <a href=(href) target="_blank" rel="noopener">(icon_with("external", "ic sm")) "Open the original"</a>
                        }
                    </p>
                </div>
            }
        </section>
    })
}

/// Every file of the claim as a plain list beside the viewer, each stored
/// one with its "Open the original", so each is one link away whatever the
/// viewer shows.
#[component]
async fn file_list(files: Vec<ViewerFile>) -> Result<impl View> {
    let count = files.len();
    Ok(view! {
        <section class="sx-sub sx-filelist" aria-labelledby="sx-files">
            <div class="sx-subh">
                <h3 id="sx-files">"Files"</h3>
                <span class="sx-fine">(if count == 1 { "1 file".to_owned() } else { format!("{count} files") })</span>
            </div>
            <ul class="sx-files">
                for file in files {
                    let document = file.document;
                    <li>
                        <span class="sx-files-ic">(ic(file.icon))</span>
                        <span class="x">
                            <b>(file.label.clone())</b>
                            <span>(document.filename.clone()) " · " (kilobytes(&document))</span>
                        </span>
                        if document.purged_at.is_some() {
                            <span class="sx-faint">"Deleted"</span>
                        } else {
                            <a href=(format!("/staff/documents/{}", document.id)) target="_blank" rel="noopener">
                                (icon_with("external", "ic sm"))
                                "Open the original"
                                <span class="sx-vh">(format!(": {}, {}", file.label, document.filename))</span>
                            </a>
                        }
                    </li>
                }
            </ul>
        </section>
    })
}

// ---------------------------------------------------------------- users

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

/// The pill of a role badge: Admins dark, Support blue-grey.
fn badge_tone(tone: &str) -> &'static str {
    if tone == "staff" { "inv" } else { "info" }
}

/// The ID status as a pill tone.
fn id_tone(status: IdStatus) -> &'static str {
    match status {
        IdStatus::Staff => "inv",
        IdStatus::Waiting => "warn",
        IdStatus::Verified => "ok",
        IdStatus::None => "neutral",
    }
}

struct UserListRow {
    user: queries::UserRow,
    names: Vec<String>,
    /// (TIN as shown, claim status)
    tins: Vec<(String, String)>,
    id_status: IdStatus,
    badge: Option<(&'static str, &'static str)>,
    open: String,
    selected: bool,
    explicit: bool,
}

#[page("/staff/users")]
pub async fn users_page(cx: &Cx) -> Result<impl View> {
    let viewer = staff_member(cx, Power::SeeUsers).await?;
    let can_create = viewer.can(Power::CreateLogin);
    let filter = UserFilter::parse(query(cx, "filter").as_deref());
    let search = query(cx, "q").unwrap_or_default();
    let opened = open_id(cx);
    let new_open = can_create && query(cx, "new").is_some();
    let config = &state(cx).config;
    let db = &state(cx).db;
    let users = queries::users(db, filter, &search, &config.admin_emails).await?;
    let selected = opened.or_else(|| users.first().map(|user| user.id));
    let mut ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    if let Some(selected) = selected.filter(|id| !ids.contains(id)) {
        ids.push(selected);
    }
    let claims = queries::claims_of(db, &ids).await?;
    let list_params = |open: &str| href("/staff/users", &[("filter", filter.code()), ("q", &search), ("open", open)]);
    let summary = match selected {
        Some(id) => user_summary_data(cx, id, &claims).await?,
        None => None,
    };
    let selected = summary.as_ref().map(|summary| summary.user.id);
    let rows: Vec<UserListRow> = users
        .into_iter()
        .map(|user| {
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
                    let tin = crate::account::tin::Tin::parse_any(&claim.tin_root, &claim.branch_code)
                        .expect("stored TINs have the shape of one");
                    let shown = if tin.is_test() { format!("{tin} · test") } else { tin.to_string() };
                    (shown, claim.status.clone())
                })
                .collect();
            let role = user_role(config, &user);
            // A test TIN proves no identity.
            let real = own.iter().filter(|claim| claim.tin_root != crate::account::tin::TEST_ROOT);
            let id_status = IdStatus::of(role.is_some(), real.map(|claim| claim.status.as_str()));
            let badge = role_badge(role);
            let open = list_params(&user.id.to_string());
            let selected = Some(user.id) == selected;
            UserListRow { user, names, tins, id_status, badge, open, selected, explicit: opened.is_some() }
        })
        .collect();
    let chips: Vec<(UserFilter, String)> = UserFilter::ALL
        .into_iter()
        .map(|chip| (chip, href("/staff/users", &[("filter", chip.code()), ("q", &search)])))
        .collect();
    let close = list_params("");
    let empty = rows.is_empty();
    let sub = if search.is_empty() {
        filter.label().to_owned()
    } else {
        format!("{} · matching “{search}”", filter.label())
    };
    Ok(view! {
        <main id="main" class="sx-main" tabindex="-1">
            page_head(title: "Users", sub: sub,
                if can_create {
                    <a class="sx-btn pri sx-hide-phone" href="/staff/users?new=1" data-new-user="">(ic("user-plus")) "New user"</a>
                }
            )
            <div class="sx-notices">sx_notice(notice: notice(cx))</div>
            <div class="sx-scroll">
                <div class="sx-stack">
                    <div class="sx-toolbar">
                        search_box(action: "/staff/users", label: "Search users", placeholder: "Search email, TIN or registered name", value: search.clone(), keep: vec![("filter", filter.code())])
                    </div>
                    <nav class="sx-chips" aria-label="Filter users">
                        for (chip, link) in chips {
                            <a class="sx-chip" href=(link) if chip == filter { aria-current="page" }>
                                match chip {
                                    UserFilter::Waiting => <span class="sx-dot warn" aria-hidden="true"></span>,
                                    UserFilter::Verified => <span class="sx-dot ok" aria-hidden="true"></span>,
                                    UserFilter::NoTin => <span class="sx-dot" aria-hidden="true"></span>,
                                    _ => "",
                                }
                                (chip.label())
                            </a>
                        }
                    </nav>
                    if empty {
                        <p class="sx-card sx-empty big">(if search.is_empty() { "No accounts here." } else { "No accounts match that search." })</p>
                    } else {
                        <div class="sx-card sx-tablewrap">
                            <table class="sx-table users">
                                <caption class="sx-vh">"Accounts"</caption>
                                <colgroup><col class="c-acct"><col><col class="c-tins"></colgroup>
                                <thead>
                                    <tr>
                                        <th scope="col">"Account"</th>
                                        <th scope="col">"Registered name"</th>
                                        <th scope="col">"TINs"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    #[key(row.user.id.as_u128())]
                                    for row in rows {
                                        user_row(row: row)
                                    }
                                </tbody>
                            </table>
                        </div>
                    }
                    <p class="sx-fine">"Active is the newest website sign-in or app token. Up to " (queries::LIST_LIMIT.to_string()) " accounts show; search to narrow."</p>
                </div>
            </div>
        </main>
        inspector(title: "User", open: opened.is_some() && summary.is_some(), close: close.clone(),
            if let Some(summary) = summary {
                user_summary(summary: summary)
            } else {
                <div class="sx-insp-b"><p class="sx-empty">"No account to show."</p></div>
            }
        )
        if can_create {
            new_login_form(open: new_open, close: close)
            <a class="sx-fab" href="/staff/users?new=1" data-new-user="" aria-label="New user">(ic("user-plus"))</a>
        }
    }
    .boxed())
}

#[component]
async fn user_row(row: UserListRow) -> Result<impl View> {
    let (id_label, _, _) = row.id_status.look();
    let names = row.names.join(" · ");
    let count = row.tins.len();
    let test_tins = row.tins.iter().any(|(shown, _)| shown.ends_with(" · test"));
    Ok(view! {
        <tr data-id=(row.user.id.to_string()) class=(row_class(row.selected, row.explicit))>
            <td class="a-main">
                <span class="sx-acct">
                    avatar(email: row.user.email.clone(), size: "sm")
                    <a class="sx-rowlink" href=(row.open) data-open="" if row.selected { aria-current="true" }>(row.user.email.clone())</a>
                    if let Some((label, tone)) = row.badge.filter(|(_, tone)| *tone == "support") {
                        sx_pill(label: label, sym: "", tone: badge_tone(tone))
                    }
                    if row.user.test_account {
                        sx_pill(label: "Test", sym: "flask", tone: "test")
                    }
                    if row.user.banned_at.is_some() {
                        sx_pill(label: "Banned", sym: "ban", tone: "bad")
                    }
                </span>
            </td>
            <td class="m cut a-sub" title=(names.clone())>
                if names.is_empty() {
                    <span class="sx-faint">"No TIN added"</span>
                } else {
                    (names)
                }
            </td>
            <td class="a-side">
                <span class="sx-tins">
                    <b>(count.to_string())</b>
                    for (tin, status) in row.tins {
                        let (label, _, tone) = status_look(&status);
                        <span class=(format!("sx-dot {}", ui::status_tone(tone))) role="img" aria-label=(format!("{tin}: {label}")) title=(format!("{tin}: {label}"))></span>
                    }
                    if test_tins {
                        <span class="sx-faint">"· test"</span>
                    }
                    <span class="sx-faint">(id_label)</span>
                </span>
            </td>
        </tr>
    })
}

/// What the users inspector shows of one account.
struct UserSummary {
    user: queries::UserRow,
    claims: Vec<queries::UserClaim>,
    badge: Option<(&'static str, &'static str)>,
    id_status: IdStatus,
    live_sessions: usize,
    apps: Vec<AppRow>,
}

async fn user_summary_data(cx: &Cx, account_id: Uuid, claims: &[queries::UserClaim]) -> Result<Option<UserSummary>> {
    let db = &state(cx).db;
    let Some(user) = queries::user(db, account_id).await? else {
        return Ok(None);
    };
    let claims: Vec<queries::UserClaim> =
        claims.iter().filter(|claim| claim.account_id == account_id).cloned().collect();
    let role = user_role(&state(cx).config, &user);
    // A test TIN proves no identity.
    let real = claims.iter().filter(|claim| claim.tin_root != crate::account::tin::TEST_ROOT);
    let id_status = IdStatus::of(role.is_some(), real.map(|claim| claim.status.as_str()));
    let live_sessions = queries::sessions(db, account_id).await?.iter().filter(|(_, live)| *live).count();
    let apps = queries::apps(db, account_id).await?.into_iter().filter(|app| app.signed_in).collect();
    Ok(Some(UserSummary { badge: role_badge(role), user, claims, id_status, live_sessions, apps }))
}

/// The inspector's account: who it is, its TINs and sign-ins, and the way
/// to its full page, where its actions are.
#[component]
async fn user_summary(summary: UserSummary) -> Result<impl View> {
    let UserSummary { user, claims, badge, id_status, live_sessions, apps } = summary;
    let (id_label, id_icon, _) = id_status.look();
    let mut names: Vec<String> = Vec::new();
    for claim in &claims {
        if !names.contains(&claim.registered_name) {
            names.push(claim.registered_name.clone());
        }
    }
    let page = format!("/staff/users/{}", user.id);
    let tins: Vec<(String, bool, String)> = claims
        .iter()
        .map(|claim| {
            let tin = crate::account::tin::Tin::parse_any(&claim.tin_root, &claim.branch_code)
                .expect("stored TINs have the shape of one");
            (tin.to_string(), tin.is_test(), claim.status.clone())
        })
        .collect();
    Ok(view! {
        <div class="sx-insp-b">
            <div class="sx-detail">
                <div class="sx-uhead panel">
                    avatar(email: user.email.clone(), size: "lg")
                    <span class="x">
                        <b>(user.email.clone())</b>
                        <span>
                            (if names.is_empty() { "No TIN added".to_owned() } else { names.join(" · ") })
                        </span>
                        <span class="sx-pills">
                            sx_pill(label: id_label, sym: glyph(id_icon), tone: id_tone(id_status))
                            if let Some((label, tone)) = badge.filter(|(_, tone)| *tone == "support") {
                                sx_pill(label: label, sym: "shield", tone: badge_tone(tone))
                            }
                            if user.test_account {
                                sx_pill(label: "Test account", sym: "flask", tone: "test")
                            }
                            if user.banned_at.is_some() {
                                sx_pill(label: "Banned", sym: "ban", tone: "bad")
                            }
                        </span>
                    </span>
                </div>
                <div class="sx-facts">
                    <div class="sx-fact"><span class="k">"Role"</span><span class="v">(badge.map_or("No staff role", |(label, _)| label))</span></div>
                    <div class="sx-fact"><span class="k">"Active"</span><span class="v">(ago(user.last_active))</span></div>
                    <div class="sx-fact"><span class="k">"Joined"</span><span class="v">(date(user.created_at))</span></div>
                </div>
                <section class="sx-sub">
                    <h3>"TINs"</h3>
                    if tins.is_empty() {
                        <p class="sx-empty">"No TIN added yet."</p>
                    }
                    <div>
                        for (tin, test, status) in tins {
                            <div class="sx-tinrow mini">
                                <span class="sx-mono">(tin)</span>
                                if test {
                                    sx_pill(label: "test TIN", sym: "flask", tone: "testtin")
                                }
                                <span class="sx-sp"></span>
                                status_pill(status: status)
                            </div>
                        }
                    </div>
                </section>
                <section class="sx-sub">
                    <h3>"Signed in"</h3>
                    if live_sessions == 0 && apps.is_empty() {
                        <p class="sx-empty">"Not signed in anywhere."</p>
                    }
                    <div>
                        if live_sessions > 0 {
                            <div class="sx-app-row mini">
                                <span class="sx-app-ic">(ic("globe"))</span>
                                <span class="x">
                                    <b>"Website"</b>
                                    <span>(if live_sessions == 1 { "1 browser signed in".to_owned() } else { format!("{live_sessions} browsers signed in") })</span>
                                </span>
                            </div>
                        }
                        for app in apps {
                            <div class="sx-app-row mini">
                                <span class="sx-app-ic">(ic(if app.is_agent() { "bot" } else { "monitor" }))</span>
                                <span class="x">
                                    <b>(app.name())</b>
                                    <span>(if app.is_agent() { "AI agent · read only" } else { "Buwiz app" })</span>
                                </span>
                            </div>
                        }
                    </div>
                </section>
                <a class="sx-btn pri block sx-in-thumb" href=(page.clone())>(ic("arrow-right")) "Open full page"</a>
            </div>
        </div>
        <div class="sx-thumbbar">
            <a class="sx-btn pri" href=(page)>(ic("arrow-right")) "Open full page"</a>
        </div>
    }
    .boxed())
}

/// "New user": an Admin makes a login. Nothing is mailed. A dialog over
/// the users list: hidden until New user opens it (with script) or the page
/// is asked for with `?new=1` (without).
#[component]
async fn new_login_form(open: bool, close: String) -> Result<impl View> {
    Ok(view! {
        <div class="sx-modal" id="new-user" role="dialog" aria-modal="true" aria-labelledby="new-user-title" hidden=(!open)>
            <a class="sx-scrim-link" href=(close.clone()) data-modal-close="" tabindex="-1" aria-hidden="true"></a>
            <div class="sx-modal-box">
                <h2 id="new-user-title">"New user"</h2>
                <p>"An Admin makes the login. They sign in with a code sent to its email."</p>
                <form class="sx-form" method="post" action="/staff/users/new">
                    <label class="sx-field">
                        <span>"Email"</span>
                        <input type="email" name="email" required=(true) maxlength="254" autocomplete="off" placeholder="name@example.com">
                        <span class="sx-hint">"They sign in with a code sent to this address, so it must be able to receive mail. Nothing is sent now. A staff role can be given on their page after their first sign-in."</span>
                    </label>
                    <label class="sx-check-in">
                        <input type="checkbox" name="test" value="on">
                        <span>"Test account"</span>
                    </label>
                    reason_field()
                    <div class="sx-form-a">
                        <a class="sx-btn" href=(close) data-modal-close="">"Cancel"</a>
                        <button class="sx-btn pri" type="submit">(ic("user-plus")) "Make the login"</button>
                    </div>
                </form>
            </div>
        </div>
    })
}

// ---------------------------------------------------------------- one user

/// The user page's tabs: (key, label).
const USER_TABS: [(&str, &str); 4] =
    [("account", "Account"), ("tins", "TINs"), ("sign", "Sign-ins and apps"), ("history", "History")];

pub mod account_id {
    use super::*;

    #[page("/staff/users/{account_id}")]
    pub async fn user_page(cx: &Cx) -> Result<impl View> {
        let viewer = staff_member(cx, Power::SeeUsers).await?;
        let account_id = path_id(cx, 0)?;
        // Staff read their own page in full, history and reasons too; the
        // changes that could lock them out are left to another Admin.
        let own_page = viewer.account.id == account_id;
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
        // A test TIN proves no identity.
        let real = claims.iter().filter(|claim| !claim.tin().is_test());
        let id_status = IdStatus::of(role.is_some(), real.map(|claim| claim.status.as_str()));
        let (id_label, id_icon, _) = id_status.look();
        let badge = role_badge(role);
        let is_admin = role.is_some_and(|(role, _)| role == Role::Admin);
        let from_server = role.is_some_and(|(_, from_server)| from_server);
        // Support may sign anyone out but an Admin.
        let may_sign_out = viewer.role == Role::Admin || !is_admin;
        let admin_viewer = viewer.can(Power::ChangeRole);
        let stored_role = user.role.clone().unwrap_or_else(|| "none".to_owned());
        let role_choices: Vec<(&'static str, &'static str, bool)> = [
            ("none", "No staff role"),
            ("support", "Support: reviews claims, sees users, signs people out"),
            ("admin", "Admin: everything"),
        ]
        .into_iter()
        .map(|(value, label)| (value, label, value == stored_role))
        .collect();
        let holds_test_tins =
            claims.iter().any(|claim| claim.tin().is_test() && crate::filing::gate::is_live(&claim.status));
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
        // A role waits for a first sign-in (`admin::change_role`).
        let never_signed_in = sessions.is_empty();
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
        let tab = query(cx, "tab").filter(|tab| USER_TABS.iter().any(|(key, _)| key == tab)).unwrap_or_default();
        let tab = if tab.is_empty() { "account".to_owned() } else { tab };
        let page = format!("/staff/users/{account_id}");
        let tabs: Vec<(&'static str, &'static str, String, bool, Option<usize>)> = USER_TABS
            .iter()
            .map(|(key, label)| {
                let count = (*key == "tins").then_some(claims.len());
                (*key, *label, href(&page, &[("tab", key)]), *key == tab, count)
            })
            .collect();
        let hidden: Vec<bool> = USER_TABS.iter().map(|(key, _)| *key != tab).collect();
        let title = if own_page { "My account" } else { "User" };
        let email = user.email.clone();
        Ok(view! {
            <main id="main" class="sx-main" tabindex="-1">
                page_head(title: title, back: Some("/staff/users".to_owned()))
                <div class="sx-notices">sx_notice(notice: notice(cx))</div>
                <div class="sx-scroll">
                    <div class="sx-userpage">
                        <div class="sx-uhead">
                            avatar(email: email.clone(), size: "lg", me: own_page)
                            <span class="x">
                                <b>(email.clone())</b>
                                <span>(subtitle)</span>
                                <span class="sx-pills">
                                    sx_pill(label: id_label, sym: glyph(id_icon), tone: id_tone(id_status))
                                    if let Some((label, tone)) = badge {
                                        sx_pill(label: label, sym: "shield", tone: badge_tone(tone))
                                    }
                                    if user.test_account {
                                        sx_pill(label: "Test account", sym: "flask", tone: "test")
                                    }
                                    if user.banned_at.is_some() {
                                        sx_pill(label: "Banned", sym: "ban", tone: "bad")
                                    }
                                </span>
                            </span>
                            if may_sign_out {
                                <div class="sx-uacts">
                                    reason_act(
                                        id: "act-sign-out".to_owned(),
                                        label: "Sign out everywhere",
                                        sym: "log-out",
                                        action: format!("/staff/users/{account_id}/sign-out"),
                                        question: "Sign out everywhere?",
                                        detail: "This account is signed out of every browser, the desktop app and every AI agent. They can sign in again.",
                                        confirm: "Sign out everywhere"
                                    )
                                </div>
                            }
                        </div>
                        <nav class="sx-tabs" role="tablist" aria-label="Account" data-tabs="">
                            for (key, label, link, on, count) in tabs {
                                <a class="sx-tab" role="tab" id=(format!("tab-{key}-tab")) href=(link) aria-controls=(format!("tab-{key}")) aria-selected=(if on { "true" } else { "false" }) tabindex=(if on { "0" } else { "-1" }) data-tab=(key)>
                                    (label)
                                    if let Some(count) = count {
                                        <span class="sx-count">(count.to_string())</span>
                                    }
                                </a>
                            }
                        </nav>
                        <section class="sx-card sx-tabbody" role="tabpanel" id="tab-account" aria-labelledby="tab-account-tab" hidden=(hidden[0])>
                            <div class="sx-stack">
                                if !admin_viewer {
                                    <p class="sx-notice info">(ic("info")) <span>"Only an admin can change roles, bans and test labels."</span></p>
                                } else if own_page {
                                    <p class="sx-notice info">(ic("info")) <span>"This is your own account: another admin changes its role, ban or test label."</span></p>
                                } else if from_server {
                                    <p class="sx-notice info">(ic("info")) <span>"Admin through the server setting ADMIN_EMAILS. Change it there."</span></p>
                                }
                                let acts = admin_viewer && !own_page && !from_server;
                                <div class="sx-facts">
                                    <div class="sx-fact">
                                        <span class="k">"Email"</span>
                                        <span class="v">(email.clone())</span>
                                        <span></span>
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Registered names"</span>
                                        <span class="v">(if names.is_empty() { "None yet".to_owned() } else { names.join(" · ") })</span>
                                        <span></span>
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Role"</span>
                                        <span class="v">
                                            (badge.map_or("No staff role", |(label, _)| label))
                                            if acts && never_signed_in {
                                                <span class="sx-text">" They have not signed in yet: a staff role can be given after their first sign-in."</span>
                                            }
                                        </span>
                                        if acts {
                                            act(id: "act-role".to_owned(), label: "Change", small: true, question: format!("Change {email}'s role?"), detail: "It takes effect on their next page.".to_owned(),
                                                <form class="sx-form" method="post" action=(format!("/staff/users/{account_id}/role"))>
                                                    <label class="sx-field">
                                                        <span>"Role"</span>
                                                        <select name="role">
                                                            for (value, label, chosen) in role_choices {
                                                                <option value=(value) selected=(chosen)>(label)</option>
                                                            }
                                                        </select>
                                                    </label>
                                                    reason_field()
                                                    act_buttons(label: "Change role")
                                                </form>
                                            )
                                        } else {
                                            <span></span>
                                        }
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Test label"</span>
                                        <span class="v">
                                            if user.test_account {
                                                sx_pill(label: "Test account", sym: "flask", tone: "test")
                                            } else {
                                                "Not a test account"
                                            }
                                            if acts && holds_test_tins {
                                                <span class="sx-text">" It holds test TINs: revoke them before you remove the test label."</span>
                                            }
                                        </span>
                                        if acts && !holds_test_tins && user.test_account {
                                            act(id: "act-test".to_owned(), label: "Remove", small: true, question: format!("Remove the test label from {email}?"), detail: "Staff no longer see it as a test account.".to_owned(),
                                                <form class="sx-form" method="post" action=(format!("/staff/users/{account_id}/test"))>
                                                    <input type="hidden" name="test" value="off">
                                                    reason_field()
                                                    act_buttons(label: "Remove the test label")
                                                </form>
                                            )
                                        } else if acts && !holds_test_tins {
                                            act(id: "act-test".to_owned(), label: "Mark as test", small: true, question: format!("Mark {email} as a test account?"), detail: "A label for staff only. Sends work as usual.".to_owned(),
                                                <form class="sx-form" method="post" action=(format!("/staff/users/{account_id}/test"))>
                                                    <input type="hidden" name="test" value="on">
                                                    reason_field()
                                                    act_buttons(label: "Mark as test account")
                                                </form>
                                            )
                                        } else {
                                            <span></span>
                                        }
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Status"</span>
                                        <span class="v">
                                            if let Some(at) = user.banned_at {
                                                sx_pill(label: "Banned", sym: "ban", tone: "bad")
                                                <span class="sx-text">
                                                    " Banned by " (user.banned_by_email.clone().unwrap_or_default()) " on " (date(at)) ": "
                                                    (user.ban_reason.clone().unwrap_or_default())
                                                </span>
                                            } else {
                                                "Can sign in"
                                                if acts && is_admin {
                                                    <span class="sx-text">" An admin can't be banned. Change the role first."</span>
                                                }
                                            }
                                        </span>
                                        if acts && user.banned_at.is_some() {
                                            reason_act(
                                                id: "act-unban".to_owned(),
                                                label: "Lift the ban",
                                                small: true,
                                                action: format!("/staff/users/{account_id}/unban"),
                                                question: format!("Lift the ban on {email}?"),
                                                detail: "They can sign in and send again.",
                                                confirm: "Lift ban",
                                                danger: false
                                            )
                                        } else if acts && !is_admin {
                                            reason_act(
                                                id: "act-ban".to_owned(),
                                                label: "Ban",
                                                sym: "ban",
                                                tone: "dan",
                                                small: true,
                                                action: format!("/staff/users/{account_id}/ban"),
                                                question: format!("Ban {email}?"),
                                                detail: "They can't sign in or send returns until you lift the ban. Their TINs stay on the account.",
                                                confirm: "Ban"
                                            )
                                        } else {
                                            <span></span>
                                        }
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Joined"</span>
                                        <span class="v">(date(user.created_at))</span>
                                        <span></span>
                                    </div>
                                    <div class="sx-fact">
                                        <span class="k">"Last active"</span>
                                        <span class="v">(ago(user.last_active))</span>
                                        <span></span>
                                    </div>
                                </div>
                            </div>
                        </section>
                        <section class="sx-card sx-tabbody" role="tabpanel" id="tab-tins" aria-labelledby="tab-tins-tab" hidden=(hidden[1])>
                            <div class="sx-stack">
                                <div class="sx-subh">
                                    <span class="sx-faint">(format!("{} TIN{}", claims.len(), if claims.len() == 1 { "" } else { "s" }))</span>
                                    // Not for ADMIN_EMAILS: their page cannot take the label off.
                                    if admin_viewer && user.test_account && user.banned_at.is_none() && !from_server {
                                        test_tin_form(account_id: account_id)
                                    }
                                </div>
                                if claims.is_empty() {
                                    <p class="sx-empty">"No TIN added."</p>
                                } else {
                                    <div>
                                        for claim in claims.clone() {
                                            <div class="sx-tinrow" data-status=(claim.shown_status().to_owned())>
                                                kind_icon(kind: claim.kind.clone())
                                                <span class="x">
                                                    <span>
                                                        <span class="sx-mono">(claim.tin().to_string())</span>
                                                        if claim.tin().is_test() {
                                                            sx_pill(label: "Test TIN", sym: "flask", tone: "testtin")
                                                        }
                                                    </span>
                                                    <span class="nm">(claim.registered_name.clone())</span>
                                                </span>
                                                if claim.status == "pending" {
                                                    sx_pill(label: format!("Waiting {}", span_since(claim.waiting_since())), sym: "clock", tone: "warn")
                                                } else {
                                                    status_pill(status: claim.shown_status().to_owned())
                                                }
                                                <a class="sx-btn sm" href=(format!("/staff/claims/{}", claim.id))>"Open claim"</a>
                                            </div>
                                        }
                                    </div>
                                }
                                <section class="sx-sub">
                                    <div class="sx-subh">
                                        <h3>"Identity documents"</h3>
                                        <span class="sx-fine">"Files are deleted once the claim is decided"</span>
                                    </div>
                                    if cards.is_empty() {
                                        <p class="sx-empty">"No documents sent."</p>
                                    } else {
                                        <div class="sx-docs">
                                            for (document, tin, status) in cards {
                                                document_card(document: document, tin: tin, status: status)
                                            }
                                        </div>
                                    }
                                </section>
                            </div>
                        </section>
                        <section class="sx-card sx-tabbody" role="tabpanel" id="tab-sign" aria-labelledby="tab-sign-tab" hidden=(hidden[2])>
                            if signed_in.is_empty() && live_sessions == 0 {
                                <p class="sx-empty">"Not signed in anywhere."</p>
                            }
                            <div>
                                if live_sessions > 0 {
                                    <div class="sx-app-row">
                                        <span class="sx-app-ic">(ic("globe"))</span>
                                        <span class="x">
                                            <b>"Website"</b>
                                            <span>(if live_sessions == 1 { "1 browser signed in".to_owned() } else { format!("{live_sessions} browsers signed in") })</span>
                                        </span>
                                    </div>
                                }
                                for app in signed_in {
                                    connected_app(account_id: account_id, app: app, may_sign_out: may_sign_out)
                                }
                            </div>
                        </section>
                        <section class="sx-card sx-tabbody" role="tabpanel" id="tab-history" aria-labelledby="tab-history-tab" hidden=(hidden[3])>
                            <ol class="sx-hist">
                                for (at, icon_name, what) in history {
                                    <li>
                                        <span class="sx-feed-ic">(ic(glyph(icon_name)))</span>
                                        <span class="x"><b>(what)</b><span class="t">(date_time(at))</span></span>
                                    </li>
                                }
                            </ol>
                        </section>
                    </div>
                </div>
            </main>
        }
        .boxed())
    }
}

/// An Admin adds a made-up TIN to a test account: active at once, it never
/// sends.
#[component]
async fn test_tin_form(account_id: Uuid) -> Result<impl View> {
    Ok(view! {
        act(id: "act-testtin".to_owned(), label: "Add a test TIN", sym: "plus", small: true, question: "Add a test TIN",
            detail: "A made-up TIN, 000-000-000 at any branch, active at once for trying the apps. Buwiz never sends its returns to the BIR.".to_owned(),
            <form class="sx-form" method="post" action=(format!("/staff/users/{account_id}/test-tin"))>
                <label class="sx-field">
                    <span>"TIN"</span>
                    <input class="sx-mono" type="text" name="tin" required=(true) value="000-000-000-00000" inputmode="numeric">
                </label>
                <label class="sx-field">
                    <span>"Registered name"</span>
                    <input type="text" name="registered_name" required=(true) maxlength="200">
                </label>
                <label class="sx-field">
                    <span>"Kind"</span>
                    <select name="kind">
                        <option value="personal" selected=(true)>"Personal"</option>
                        <option value="business">"Business"</option>
                    </select>
                </label>
                reason_field()
                act_buttons(label: "Add the test TIN")
            </form>
        )
    })
}

/// One file in a user's identity documents, linking to its claim's review.
#[component]
async fn document_card(document: DocumentRow, tin: String, status: String) -> Result<impl View> {
    let pending = status == "pending";
    Ok(view! {
        <a class=(if pending { "sx-doc hot" } else { "sx-doc" }) href=(format!("/staff/claims/{}", document.claim_id))>
            <span class="sx-doc-thumb">
                if document.is_viewable_image() {
                    <img src=(format!("/staff/documents/{}", document.id)) alt="" loading="lazy">
                } else if document.purged_at.is_some() {
                    (ic("trash"))
                } else {
                    (ic(glyph(DocumentKind::icon(&document.kind))))
                }
            </span>
            <b>(DocumentKind::label(&document.kind))</b>
            if pending {
                sx_pill(label: "Review now", sym: "clock", tone: "warn")
            } else if document.purged_at.is_some() {
                <span class="sx-faint">"Deleted"</span>
            }
            <span class="sx-faint">"Sent " (short_date(document.created_at)) " · for " (tin)</span>
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
    let label = if app.is_agent() { "Remove" } else { "Sign out" };
    Ok(view! {
        <div class="sx-app-row">
            if app.is_agent() {
                <span class="sx-app-ic">(name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default())</span>
            } else {
                <span class="sx-app-ic">(ic("monitor"))</span>
            }
            <span class="x">
                <b>(name.clone())</b>
                <span>(detail)</span>
            </span>
            if may_sign_out {
                reason_act(
                    id: format!("act-app-{}", app.id),
                    label: label,
                    small: true,
                    action: action,
                    question: format!("Sign {name} out?"),
                    detail: "It loses access to this account until the person connects it again.",
                    confirm: label
                )
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
    // A login staff or the seed tool made has a `created` line saying who
    // made it, and why; any other account was made at its first sign-in.
    let mut events = Vec::new();
    if !account_audit.iter().any(|line| line.action == "created") {
        events.push((user.created_at, "person_add", "Made the account at their first sign-in".to_owned()));
    }
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
            // Staff added it, active at once: its activation says so.
            _ if claim.tin().is_test() => {}
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
                    Some("admin") if claim.tin().is_test() => ("science", format!("Test TIN {tin} added, active")),
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
        "approved" => "verified",
        "rejected" => "cancel",
        "sent_back" => "assignment_return",
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
        "created" => format!("added {tin} without papers"),
        "approved" => format!("approved {tin}"),
        "rejected" => format!("rejected a claim on {tin}"),
        "sent_back" => format!("sent {tin} back for a fix"),
        "activated" => format!("activated {tin} without papers"),
        "auto_activated" => format!("activated {tin} automatically"),
        "auto_confirmed" => format!("confirmed the automatic activation of {tin}"),
        "paused" => format!("paused {tin}"),
        "resumed" => format!("resumed {tin}"),
        _ => format!("removed the {file} from {tin}"),
    };
    // Staff acting on their own TIN act as staff, not as its owner: these
    // actions are staff's alone, so the line's actor being the account
    // itself says it was their own.
    let own = match line.action.as_str() {
        "activated" => Some(format!("activated their own TIN {tin} without papers")),
        "approved" => Some(format!("approved their own claim on {tin}")),
        "rejected" => Some(format!("rejected their own claim on {tin}")),
        "sent_back" => Some(format!("sent their own claim on {tin} back for a fix")),
        "paused" => Some(format!("paused their own TIN {tin}")),
        "resumed" => Some(format!("resumed their own TIN {tin}")),
        "revoked" => Some(format!("revoked their own ownership of {tin}")),
        "auto_confirmed" => Some(format!("confirmed the automatic activation of their own TIN {tin}")),
        _ => None,
    }
    .filter(|_| line.source == "web" && line.actor_id == account_id);
    let (who, what) = match own {
        Some(own) => (format!("Staff {}", line.actor_email), own),
        None => (actor_text(line.actor_id, &line.actor_email, &line.source, Some(account_id)), what),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(tone: &'static str, text: &str) -> Flag {
        (tone, "dot", text.to_owned())
    }

    #[test]
    fn a_row_shows_its_most_serious_flags_and_every_bad_one() {
        let texts = |flags: Vec<Flag>| flags.into_iter().map(|(_, _, text)| text).collect::<Vec<_>>();
        let (shown, rest) = row_flags(vec![
            flag("good", "First claim"),
            flag("info", "Test"),
            flag("bad", "TIN differs"),
            flag("warn", "Paused"),
        ]);
        assert_eq!(
            (texts(shown), texts(rest)),
            (vec!["TIN differs".to_owned()], vec!["Paused".into(), "Test".into(), "First claim".into()])
        );
        // Two bad ones: both on the row, in their order.
        let (shown, rest) = row_flags(vec![
            flag("good", "Selfie"),
            flag("bad", "Another account"),
            flag("info", "Test"),
            flag("bad", "TIN differs"),
        ]);
        assert_eq!(texts(shown), ["Another account", "TIN differs"]);
        assert_eq!(texts(rest), ["Test", "Selfie"]);
        // None bad: the most serious one.
        let (shown, rest) = row_flags(vec![flag("good", "Selfie"), flag("info", "Test")]);
        assert_eq!((texts(shown), texts(rest)), (vec!["Test".to_owned()], vec!["Selfie".to_owned()]));
        assert_eq!(row_flags(Vec::new()), (Vec::new(), Vec::new()));
    }
}
