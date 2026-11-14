//! The staff console's read paths: the Overview's numbers and activity,
//! the sidebar's counts, search everything, the inspector chosen by
//! `?open=`, what Support sees beside an Admin, one's own page in the
//! console, and that its pages load nothing from another host.

mod common;

use axum::http::{StatusCode, header};
use buwiz_cloud::account::staff as queries;
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

/// The part of a staff page from the inspector on.
fn inspector(page: &str) -> &str {
    &page[page.find("id=\"inspector\"").expect("an inspector")..]
}

/// The opening tag on `page` that holds `needle`. Topcoat writes conditional
/// attributes in no fixed order, so tests look inside the tag.
fn tag<'a>(page: &'a str, needle: &str) -> &'a str {
    let at = page.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    let start = page[..at].rfind('<').unwrap();
    &page[start..start + page[start..].find('>').unwrap() + 1]
}

/// The part of a staff page before the inspector: the sidebar and the list.
fn before_inspector(page: &str) -> &str {
    &page[..page.find("id=\"inspector\"").unwrap_or(page.len())]
}

async fn label_test(db: &PgPool, id: Uuid) {
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(db).await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn the_sidebar_counts_are_the_lengths_of_the_lists(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (helper, _) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let (cara, _) = site.person("cara@example.test").await;
    let (dan, _) = site.person("dan@example.test").await;
    add_claim(&site.db, ana, "000123451", "pending").await;
    add_claim(&site.db, bob, "000123452", "pending").await;
    add_claim(&site.db, cara, "000123453", "needs_fix").await;
    let auto = add_claim(&site.db, cara, "000123454", "approved").await;
    sqlx::query("UPDATE tin_claims SET verified_by = 'auto' WHERE id = $1").bind(auto).execute(&site.db).await.unwrap();
    label_test(&site.db, bob).await;
    site.post(&format!("/staff/users/{dan}/ban"), &staff, "reason=Spam").await;

    let counts = queries::counts(&site.db, &site.state.config.admin_emails).await.unwrap();
    // STAFF is an Admin through ADMIN_EMAILS, helper by role.
    assert_eq!(counts, queries::Counts { waiting: 2, sent_back: 1, auto: 1, users: 6, staff: 2, test: 1, banned: 1 });
    // Each count is what its list holds.
    for (path, rows) in [
        ("/staff/queue?status=waiting", 2),
        ("/staff/queue?status=sent_back", 1),
        ("/staff/queue?status=auto", 1),
        ("/staff/users", 6),
        ("/staff/users?filter=staff", 2),
        ("/staff/users?filter=test", 1),
        ("/staff/users?filter=banned", 1),
    ] {
        let page = text(site.get(path, &staff).await).await;
        assert_eq!(before_inspector(&page).matches("<tr data-id=").count(), rows, "{path}");
    }
    // The sidebar shows them, the waiting count as hot.
    let page = text(site.get("/staff", &staff).await).await;
    assert!(page.contains(r#"<span class="sx-count hot">2<span class="sx-vh"> waiting</span></span>"#), "{page}");
    assert!(page.contains(r#"<span class="t">All users</span><span class="sx-count">6</span>"#), "{page}");
    assert!(page.contains(r#"<span class="t">Banned</span><span class="sx-count">1</span>"#), "{page}");
    let _ = helper;
}

#[sqlx::test(migrations = "./migrations")]
async fn the_overview_counts_what_it_says(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let old = add_claim(&site.db, ana, "000123451", "pending").await;
    sqlx::query("UPDATE tin_claims SET created_at = now() - interval '26 hours' WHERE id = $1")
        .bind(old)
        .execute(&site.db)
        .await
        .unwrap();
    add_claim(&site.db, bob, "000123452", "pending").await;
    let active = add_claim(&site.db, ana, "000123453", "approved").await;
    let paused = add_claim(&site.db, bob, "000123454", "approved").await;
    site.post(&format!("/staff/claims/{paused}/pause"), &staff, "reason=A+complaint").await;
    // A test TIN is made up: not an active TIN of anyone's.
    label_test(&site.db, bob).await;
    let added = site
        .post(
            &format!("/staff/users/{bob}/test-tin"),
            &staff,
            "tin=000-000-000-00000&kind=personal&registered_name=Bob&reason=Trial",
        )
        .await;
    assert_eq!(location(&added), format!("/staff/users/{bob}?notice=test_tin_added"));
    let auto = add_claim(&site.db, bob, "000123455", "approved").await;
    sqlx::query("UPDATE tin_claims SET verified_by = 'auto' WHERE id = $1").bind(auto).execute(&site.db).await.unwrap();
    // Three returns sent today, one yesterday, one that failed.
    for (status, ago) in [
        ("sent", "0 seconds"),
        ("sent", "1 second"),
        ("sent", "2 seconds"),
        ("sent", "30 hours"),
        ("failed", "0 seconds"),
    ] {
        sqlx::query(
            "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, sent_at)
             VALUES ($1, $2, '2551Q', '2026-Q3', 'r.xml', '\\x00', $3, now() - $4::interval)",
        )
        .bind(ana)
        .bind(active)
        .bind(status)
        .bind(ago)
        .execute(&site.db)
        .await
        .unwrap();
    }

    let numbers = queries::overview(&site.db).await.unwrap();
    assert_eq!((numbers.waiting, numbers.active, numbers.active_week, numbers.auto), (2, 2, 2, 1));
    assert_eq!((numbers.paused, numbers.test), (1, 1));
    assert_eq!((numbers.accounts, numbers.accounts_week), (4, 4));
    assert!(numbers.oldest_waiting.is_some());

    let admin = text(site.get("/staff", &staff).await).await;
    assert!(admin.contains("Claims waiting</span><span class=\"n\">2</span>"), "{admin}");
    assert!(admin.contains("Oldest 1 d 2 h"), "{admin}");
    assert!(admin.contains("Active TINs</span><span class=\"n\">2</span>"), "{admin}");
    // The small line under it names what is paused and what is a test TIN.
    assert!(tile(&admin, "Active TINs").contains(&breakdown(&[("", 1, "paused"), ("", 1, "test")])), "{admin}");
    assert!(admin.contains("Accounts</span><span class=\"n\">4</span>"), "{admin}");
    assert!(admin.contains("Activated automatically</span><span class=\"n\">1</span>"), "{admin}");
    // Today began at midnight in Manila: the three sends of the last seconds
    // fall in it (all but the newest only if the test runs at midnight).
    let start = admin.find("Returns sent today").expect("an Admin sees returns sent today");
    let sent: i64 =
        admin[start..].split("<span class=\"n\">").nth(1).unwrap().split('<').next().unwrap().parse().unwrap();
    assert!((1..=3).contains(&sent), "{sent}");
    // The tile of returns is counts only, so Support sees it as well.
    let support = text(site.get("/staff", &support).await).await;
    assert!(support.contains("Returns sent today"), "{support}");
    assert!(support.contains("Claims waiting</span><span class=\"n\">2</span>"), "{support}");
}

/// The Overview tile whose label is `label`: up to its closing `</div>`.
fn tile<'a>(page: &'a str, label: &str) -> &'a str {
    let at = page.find(label).unwrap_or_else(|| panic!("no tile {label}"));
    &page[at..at + page[at..].find("</div>").unwrap()]
}

/// The small line of a tile as it renders: parts like `2 failed` joined by
/// a dot, each in its own class (`bad`, `warn`, or none) with a no-break
/// space between the number and its word.
fn breakdown(parts: &[(&str, i64, &str)]) -> String {
    let parts: Vec<String> =
        parts.iter().map(|(class, n, word)| format!("<span class=\"{class}\">{n}\u{a0}{word}</span>")).collect();
    format!("<span class=\"s\">{}</span>", parts.join(" · "))
}

/// A return in `status`: `sent_at` / `sending_since` / `created_at` as
/// offsets (a Postgres interval, or `None`) from midnight in Manila today.
async fn add_return(
    db: &PgPool,
    account: Uuid,
    claim: Uuid,
    status: &str,
    sent_at: Option<&str>,
    sending_since: Option<&str>,
    created_at: &str,
) {
    const MIDNIGHT: &str = "(date_trunc('day', now() AT TIME ZONE 'Asia/Manila') AT TIME ZONE 'Asia/Manila')";
    let at = |offset: Option<&str>| offset.map(|o| format!("{MIDNIGHT} + interval '{o}'"));
    sqlx::query(&format!(
        "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status,
             sent_at, sending_since, created_at)
         VALUES ($1, $2, '2551Q', '2026-Q3', 'r.xml', '\\x00', $3, {}, {}, {})",
        at(sent_at).unwrap_or("NULL".into()),
        at(sending_since).unwrap_or("NULL".into()),
        at(Some(created_at)).unwrap(),
    ))
    .bind(account)
    .bind(claim)
    .bind(status)
    .execute(db)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn the_returns_tile_counts_by_manila_days_for_admin_and_support(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123453", "approved").await;
    // Sent: just after Manila midnight (today), just before it (yesterday),
    // 3 days ago, the first minute of the 7-day window, one second before
    // the window, and 8 days ago. Only the first counts for today; the
    // first four for the week.
    for sent in ["1 second", "-1 second", "-3 days 12 hours", "-6 days 1 minute", "-6 days -1 second", "-8 days"] {
        add_return(&site.db, ana, claim, "sent", Some(sent), None, sent).await;
    }
    // Failed today (never attempted, so by creation), and failed whose last
    // attempt is today though it was made two days ago; failed yesterday.
    add_return(&site.db, ana, claim, "failed", None, None, "1 second").await;
    add_return(&site.db, ana, claim, "failed", None, Some("2 seconds"), "-2 days").await;
    add_return(&site.db, ana, claim, "failed", None, Some("-1 second"), "-1 day").await;
    // Waiting today: queued (`ready`), `sending`, and `unknown`; and a queued one from yesterday.
    add_return(&site.db, ana, claim, "ready", None, None, "1 second").await;
    add_return(&site.db, ana, claim, "sending", None, Some("1 second"), "-1 hour").await;
    add_return(&site.db, ana, claim, "unknown", None, Some("1 second"), "-1 hour").await;
    add_return(&site.db, ana, claim, "ready", None, None, "-1 second").await;
    // Neither sent, failed nor waiting: a dry run and an expired return.
    add_return(&site.db, ana, claim, "dry_run", Some("2 seconds"), None, "2 seconds").await;
    add_return(&site.db, ana, claim, "expired", None, None, "2 seconds").await;

    let midnight: time::OffsetDateTime =
        sqlx::query_scalar("SELECT date_trunc('day', now() AT TIME ZONE 'Asia/Manila') AT TIME ZONE 'Asia/Manila'")
            .fetch_one(&site.db)
            .await
            .unwrap();
    let tile_numbers = queries::returns_tile(&site.db, midnight, midnight - time::Duration::days(6)).await.unwrap();
    assert_eq!(tile_numbers, queries::ReturnsTile { sent_week: 4, failed: 2, waiting: 3 });

    for who in [&staff, &support] {
        let page = text(site.get("/staff", who).await).await;
        let returns = tile(&page, "Returns sent today");
        assert!(returns.contains("<span class=\"n\">1</span>"), "{returns}");
        assert!(returns.contains(&breakdown(&[("bad", 2, "failed"), ("warn", 3, "waiting")])), "{returns}");
        assert!(returns.contains("4 in the last 7 days"), "{returns}");
        // Counts only: no TIN, name or account in the tile.
        assert!(!returns.contains("000123453") && !returns.contains("ana@example.test"), "{returns}");
    }
    // No paused or test TIN here: the Active TINs tile keeps its one line.
    let page = text(site.get("/staff", &support).await).await;
    let active = tile(&page, "Active TINs");
    assert!(!active.contains("paused") && !active.contains("test"), "{active}");
    assert!(active.contains("+1 this week"), "{active}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_returns_tile_hides_a_part_that_is_zero(db: PgPool) {
    let site = Site::new(db);
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123453", "approved").await;
    add_return(&site.db, ana, claim, "failed", None, None, "1 second").await;
    let page = text(site.get("/staff", &support).await).await;
    let returns = tile(&page, "Returns sent today");
    assert!(returns.contains(&breakdown(&[("bad", 1, "failed")])), "{returns}");
    assert!(!returns.contains("waiting"), "{returns}");
    assert!(returns.contains("0 in the last 7 days"), "{returns}");
    // Nothing failed or waiting: no breakdown line at all.
    sqlx::query("DELETE FROM submissions").execute(&site.db).await.unwrap();
    let page = text(site.get("/staff", &support).await).await;
    let returns = tile(&page, "Returns sent today");
    assert!(!returns.contains("failed") && !returns.contains("waiting"), "{returns}");
}

#[sqlx::test(migrations = "./migrations")]
async fn recent_activity_names_staff_actions_without_their_reasons(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let open = add_claim(&site.db, ana, "000123451", "pending").await;
    let mine = add_claim(&site.db, ana, "000123452", "pending").await;
    site.post(&format!("/staff/claims/{open}/decision"), &staff, "decision=approve&note=Vouched+for").await;
    site.post(&format!("/staff/users/{ana}/test"), &staff, "test=on&reason=Owner+trial").await;
    // What Ana does to her own claim is hers, not staff activity.
    site.post(&format!("/tins/claims/{mine}/delete"), &ana_cookie, "").await;

    let lines = queries::recent_activity(&site.db, 8).await.unwrap();
    let actions: Vec<(&str, &str)> = lines.iter().map(|line| (line.kind.as_str(), line.action.as_str())).collect();
    assert_eq!(actions, [("account", "test_marked"), ("claim", "approved")]);
    let page = text(site.get("/staff", &staff).await).await;
    assert!(page.contains(&format!("<b>{STAFF}</b> approved")), "{page}");
    assert!(page.contains("***-***-451-00000 for ana@example.test"), "{page}");
    assert!(page.contains("marked as a test account"), "{page}");
    for reason in ["Vouched for", "Owner trial"] {
        assert!(!page.contains(reason), "{reason}: {page}");
    }
    // The TIN reads masked there, as in the account's history.
    assert!(!page.contains("000-123-451-00000"), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn recent_activity_keeps_only_the_newest_lines_of_both_tables(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let first = add_claim(&site.db, ana, "000123451", "pending").await;
    let second = add_claim(&site.db, bob, "000123452", "pending").await;
    site.post(&format!("/staff/claims/{first}/decision"), &staff, "decision=approve").await;
    site.post(&format!("/staff/users/{ana}/test"), &staff, "test=on&reason=Trial").await;
    site.post(&format!("/staff/claims/{second}/decision"), &staff, "decision=approve").await;
    site.post(&format!("/staff/users/{bob}/test"), &staff, "test=on&reason=Trial").await;
    let lines = queries::recent_activity(&site.db, 3).await.unwrap();
    let got: Vec<(&str, &str, Option<&str>)> =
        lines.iter().map(|line| (line.kind.as_str(), line.action.as_str(), line.account_email.as_deref())).collect();
    assert_eq!(
        got,
        [
            ("account", "test_marked", Some("bob@example.test")),
            ("claim", "approved", Some("bob@example.test")),
            ("account", "test_marked", Some("ana@example.test")),
        ]
    );
    // A line outlives its account: it reads as a deleted account.
    sqlx::query("UPDATE account_audit SET account_id = $1 WHERE account_id = $2")
        .bind(Uuid::new_v4())
        .bind(bob)
        .execute(&site.db)
        .await
        .unwrap();
    let lines = queries::recent_activity(&site.db, 1).await.unwrap();
    assert_eq!((lines.len(), lines[0].account_email.as_deref()), (1, None));
}

/// Support see the Overview's counts and the queue's own claims; what staff
/// did to which accounts is an Admin's, and its query does not run for them.
#[sqlx::test(migrations = "./migrations")]
async fn supports_overview_shows_counts_and_the_queue_but_no_staff_activity(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let waiting = add_claim(&site.db, ana, "000123451", "pending").await;
    let decided = add_claim(&site.db, bob, "000123452", "pending").await;
    site.post(&format!("/staff/claims/{decided}/decision"), &admin, "decision=approve").await;
    site.post(&format!("/staff/users/{bob}/test"), &admin, "test=on&reason=Trial").await;
    site.post(&format!("/staff/users/{bob}/ban"), &admin, "reason=Spam").await;

    // An Admin reads the feed: who did what to whom.
    let page = text(site.get("/staff", &admin).await).await;
    assert!(page.contains("Recent staff activity") && page.contains("class=\"sx-feed\""), "{page}");
    assert!(page.contains(&format!("<b>{STAFF}</b> banned")) && page.contains("bob@example.test"), "{page}");

    let page = text(site.get("/staff", &support).await).await;
    // The tiles, counts only.
    for label in ["Claims waiting", "Active TINs", "Accounts", "Returns sent today", "Activated automatically"] {
        assert!(page.contains(&format!("{label}</span>")), "{label}: {page}");
    }
    assert!(page.contains("Claims waiting</span><span class=\"n\">1</span>"), "{page}");
    // Waiting longest: the queue's own claim, with its link.
    assert!(page.contains("Waiting longest") && page.contains("000-123-451-00000"), "{page}");
    assert!(page.contains(&format!("open={waiting}")), "{page}");
    assert!(page.contains("class=\"sx-home2 one\""), "{page}");
    // No feed, no account acted on, no staff member named by it.
    assert!(!page.contains("Recent staff activity") && !page.contains("sx-feed"), "{page}");
    for email in ["ana@example.test", "bob@example.test", STAFF] {
        assert!(!page.contains(email), "{email}: {page}");
    }
    assert!(!page.contains("</b> banned") && !page.contains("</b> approved"), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn search_everything_finds_people_and_claims_by_the_queue_rules(db: PgPool) {
    let site = Site::new(db);
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana.reyes@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "pending").await;
    sqlx::query("UPDATE tin_claims SET registered_name = 'REYES PRINTING' WHERE id = $1")
        .bind(claim)
        .execute(&site.db)
        .await
        .unwrap();
    add_claim(&site.db, bob, "000999452", "approved").await;

    // By email, by registered name, by the TIN's digits with or without dashes.
    for search in ["ana.reyes", "printing", "123451", "000-123-451"] {
        let page = text(site.get(&format!("/staff/search?q={search}"), &support).await).await;
        assert!(page.contains(&format!("/staff/users/{ana}")), "{search}: {page}");
        assert!(page.contains(&format!("/staff/claims/{claim}")), "{search}: {page}");
        assert!(page.contains("000-123-451-00000"), "{search}: {page}");
        assert!(!page.contains("bob@example.test"), "{search}: {page}");
    }
    assert_eq!(queries::search_claims(&site.db, "999452").await.unwrap().len(), 1);
    // Wildcards are text, and an empty search finds nothing.
    assert!(queries::search_claims(&site.db, "%").await.unwrap().is_empty());
    assert!(queries::search_claims(&site.db, "  ").await.unwrap().is_empty());
    let empty = text(site.get("/staff/search", &support).await).await;
    assert!(!empty.contains("<tr data-id="), "{empty}");
    let none = text(site.get("/staff/search?q=nobody", &support).await).await;
    assert!(none.contains("No accounts match that search.") && none.contains("No claims match that search."));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_inspector_shows_the_row_in_the_link(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let first = add_claim(&site.db, ana, "000123451", "pending").await;
    let second = add_claim(&site.db, bob, "000123452", "pending").await;
    sqlx::query("UPDATE tin_claims SET created_at = now() - interval '1 hour' WHERE id = $1")
        .bind(first)
        .execute(&site.db)
        .await
        .unwrap();

    // Nothing chosen: the oldest claim, closed on tablets and phones, and
    // without its decision (see the test below).
    let page = text(site.get("/staff/queue", &staff).await).await;
    let side = inspector(&page);
    assert!(side.starts_with("id=\"inspector\" class=\"sx-insp\""), "{side}");
    assert!(side.contains("000-123-451-00000") && !side.contains("000-123-452-00000"), "{side}");
    assert!(!side.contains(&format!("action=\"/staff/claims/{first}/decision\"")), "{side}");
    assert!(page.contains(&format!("<tr data-id=\"{first}\" class=\"sel auto\">")), "{page}");
    // `?open=` chooses the row and opens the panel, wherever it is in the list.
    let page = text(site.get(&format!("/staff/queue?status=waiting&open={second}"), &staff).await).await;
    let side = inspector(&page);
    assert!(side.starts_with("id=\"inspector\" class=\"sx-insp is-open\""), "{side}");
    assert!(side.contains("000-123-452-00000") && side.contains("bob@example.test"), "{side}");
    assert!(side.contains(&format!("action=\"/staff/claims/{second}/decision\"")), "{side}");
    assert!(page.contains(&format!("<tr data-id=\"{second}\" class=\"sel\">")), "{page}");
    // Its close link is the same list without the choice.
    assert!(side.contains("href=\"/staff/queue?status=waiting\" data-close-insp=\"\""), "{side}");
    // A claim that is not there leaves the panel empty.
    let page = text(site.get(&format!("/staff/queue?open={}", Uuid::new_v4()), &staff).await).await;
    assert!(inspector(&page).contains("Nothing to review in this tab."));
    // The users list does the same with an account.
    let page = text(site.get(&format!("/staff/users?open={bob}"), &staff).await).await;
    let side = inspector(&page);
    assert!(side.contains("bob@example.test") && side.contains(&format!("href=\"/staff/users/{bob}\"")), "{side}");
    assert!(side.contains("000-123-452-00000"), "{side}");
}

/// Adds `n` files to `claim`, oldest first: photos and PDFs in turn, the
/// last `purged` of them deleted. Returns their ids in order.
async fn add_files(db: &PgPool, claim: Uuid, n: i32, purged: i32) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for at in 0..n {
        let (kind, content_type, name) =
            if at % 2 == 0 { ("form_2303", "image/png", "photo") } else { ("authority", "application/pdf", "scan") };
        let id = sqlx::query_scalar(
            "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, purged_at, created_at)
             VALUES ($1, $2, $3, $4, 2048, decode('00', 'hex'), CASE WHEN $5 THEN now() END,
                     now() - interval '1 hour' + $6 * interval '1 second')
             RETURNING id",
        )
        .bind(claim)
        .bind(kind)
        .bind(format!("{name}-{at}.{}", if at % 2 == 0 { "png" } else { "pdf" }))
        .bind(content_type)
        .bind(at >= n - purged)
        .bind(at)
        .fetch_one(db)
        .await
        .unwrap();
        ids.push(id);
    }
    ids
}

/// The viewer and the file list of a claim's review, from `page`.
fn papers(page: &str) -> (&str, &str) {
    let viewer = &page[page.find("class=\"sx-viewer viewer\"").expect("a viewer")..];
    let viewer = &viewer[..viewer.find("</section>").unwrap()];
    let list = &page[page.find("class=\"sx-files\"").expect("a file list")..];
    let list = &list[..list.find("</ul>").unwrap()];
    (viewer, list)
}

#[sqlx::test(migrations = "./migrations")]
async fn the_viewer_takes_any_number_of_files_and_lists_each(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "pending").await;
    // 14 files, the last 4 deleted (a business claim resent a few times).
    let ids = add_files(&site.db, claim, 14, 4).await;
    for path in [format!("/staff/claims/{claim}"), format!("/staff/queue?status=waiting&open={claim}")] {
        let page = text(site.get(&path, &staff).await).await;
        let (viewer, list) = papers(&page);
        // A radio and a pane for each, the first one chosen: no script and
        // no rule per position needed.
        assert_eq!(viewer.matches("class=\"sx-viewer-radio\"").count(), 14, "{path}");
        assert_eq!(viewer.matches("class=\"sx-viewer-pane\"").count(), 14, "{path}");
        assert_eq!(viewer.matches(" checked").count(), 1, "{path}");
        assert!(tag(viewer, "id=\"document-0\"").contains(" checked"), "{viewer}");
        assert!(
            tag(viewer, "id=\"document-13\"").contains("aria-label=\"Proof of authority (7) (14 of 14)\""),
            "{viewer}"
        );
        // Each pane marks its own tab, the 14th included.
        let panes: Vec<&str> = viewer.split("class=\"sx-viewer-pane\"").skip(1).collect();
        for (index, pane) in panes.iter().enumerate() {
            assert_eq!(pane.matches("<label ").count(), 14, "{path} pane {index}");
            assert_eq!(pane.matches("class=\"on\"").count(), 1, "{path} pane {index}");
            assert!(tag(pane, "class=\"on\"").contains(&format!("for=\"document-{index}\"")), "{path} pane {index}");
        }
        // Photos wait until their pane shows.
        let images: Vec<&str> = viewer.split("<img ").skip(1).collect();
        assert_eq!(images.len(), 5, "{path}");
        for image in images {
            assert!(image[..image.find('>').unwrap()].contains("loading=\"lazy\""), "{image}");
        }
        // The plain list names every file; each stored one has its link,
        // the deleted ones say so.
        assert_eq!(list.matches("<li>").count(), 14, "{path}");
        for (at, id) in ids.iter().enumerate() {
            let link = format!("href=\"/staff/documents/{id}\"");
            if at < 10 {
                assert!(list.contains(&link), "{path} file {at}");
            } else {
                assert!(!page.contains(&link), "{path} deleted file {at}");
            }
        }
        assert_eq!(list.matches("Open the original").count(), 10, "{path}");
        assert_eq!(list.matches(">Deleted<").count(), 4, "{path}");
    }
    // staff.css has no rule per position left.
    let css = text(site.get("/assets/staff.css", "").await).await;
    assert!(!css.contains("nth-of-type") && css.contains(".sx-viewer-radio:checked + .sx-viewer-pane"));
}

/// The opening `<a …>` of the link whose text holds `words`.
fn link_with<'a>(page: &'a str, words: &str) -> &'a str {
    let at = page.find(words).unwrap_or_else(|| panic!("no {words}"));
    let start = page[..at].rfind("<a ").unwrap();
    &page[start..start + page[start..].find('>').unwrap() + 1]
}

/// The `<form …>…</form>` on `page` that holds `needle`.
fn form_with<'a>(page: &'a str, needle: &str) -> &'a str {
    let at = page.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    let start = page[..at].rfind("<form").unwrap();
    &page[start..start + page[start..].find("</form>").unwrap() + "</form>".len()]
}

/// After a decision the queue reloads with the next claim in the inspector:
/// nobody chose it, so it shows its facts and papers but no decision, no
/// action and no thumb bar, only "Review this claim" to `?open=`. Chosen,
/// it has the decision as on its own page, field for field.
#[sqlx::test(migrations = "./migrations")]
async fn a_claim_nobody_chose_has_no_decision_in_the_inspector(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    let first = add_claim(&site.db, ana, "000123451", "pending").await;
    add_claim(&site.db, bob, "000123452", "pending").await;
    sqlx::query("UPDATE tin_claims SET created_at = now() - interval '1 hour' WHERE id = $1")
        .bind(first)
        .execute(&site.db)
        .await
        .unwrap();
    add_files(&site.db, first, 2, 0).await;
    let decision = format!("action=\"/staff/claims/{first}/decision\"");

    for path in ["/staff/queue?notice=claim_decided", "/staff/queue?status=waiting&q=000123451"] {
        let page = text(site.get(path, &staff).await).await;
        let side = inspector(&page);
        // Its facts and papers are there.
        assert!(side.contains("000-123-451-00000") && side.contains("ana@example.test"), "{path}: {side}");
        assert!(side.contains("class=\"sx-viewer viewer\"") && side.contains("Typed vs. on the document"), "{side}");
        assert!(side.contains("Checks"), "{side}");
        // No decision, no thumb bar, no action form of any kind.
        assert!(!side.contains(&decision) && !side.contains("name=\"decision\""), "{path}: {side}");
        assert!(!side.contains("sx-thumbbar") && !side.contains("<form"), "{path}: {side}");
        assert!(!side.contains("Approve") && !side.contains("Ask for a fix") && !side.contains(">Reject<"), "{side}");
        // One way on: "Review this claim", to the same list with ?open=.
        assert_eq!(side.matches("Review this claim").count(), 1, "{side}");
        let open = if path.contains("q=") {
            format!("/staff/queue?status=waiting&amp;q=000123451&amp;open={first}")
        } else {
            format!("/staff/queue?status=waiting&amp;open={first}")
        };
        assert!(link_with(side, "Review this claim").contains(&format!("href=\"{open}\"")), "{side}");
    }

    // Chosen with ?open=: the decision, its thumb bar, and the very form of
    // the claim's own page.
    let page = text(site.get(&format!("/staff/queue?status=waiting&open={first}"), &staff).await).await;
    let side = inspector(&page);
    assert!(!side.contains("Review this claim"), "{side}");
    assert!(side.contains("class=\"sx-thumbbar\""), "{side}");
    let chosen = form_with(side, &decision);
    let own_page = text(site.get(&format!("/staff/claims/{first}"), &staff).await).await;
    assert_eq!(chosen, form_with(&own_page, &decision));
    for field in [
        "name=\"decision\" value=\"approve\"",
        "name=\"decision\" value=\"fix\"",
        "name=\"decision\" value=\"reject\"",
        "name=\"reasons\"",
        "<textarea name=\"note\"",
    ] {
        assert!(chosen.contains(field), "{field}: {chosen}");
    }
    let thumb = &side[side.find("class=\"sx-thumbbar\"").unwrap()..];
    assert_eq!(thumb.matches("form=\"decision\"").count(), 3, "{thumb}");
}

/// An approved claim nobody chose shows no pause, revoke or activate either.
#[sqlx::test(migrations = "./migrations")]
async fn a_claim_nobody_chose_has_no_actions_in_the_inspector(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "approved").await;
    let page = text(site.get("/staff/queue?status=approved", &staff).await).await;
    let side = inspector(&page);
    assert!(side.contains("000-123-451-00000"), "{side}");
    assert!(!side.contains("<form") && !side.contains("sx-act"), "{side}");
    assert!(link_with(side, "Review this claim").contains(&format!("open={claim}")), "{side}");
    let page = text(site.get(&format!("/staff/queue?status=approved&open={claim}"), &staff).await).await;
    let side = inspector(&page);
    for action in ["pause", "revoke"] {
        assert!(side.contains(&format!("action=\"/staff/claims/{claim}/{action}\"")), "{action}: {side}");
    }
    assert!(!side.contains("Review this claim"), "{side}");
}

#[sqlx::test(migrations = "./migrations")]
async fn support_sees_no_admin_controls_in_the_console(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "approved").await;
    site.post(&format!("/staff/users/{ana}/test"), &admin, "test=on&reason=Trial").await;

    let page = text(site.get("/staff/users", &admin).await).await;
    assert!(page.contains("data-new-user=\"\"") && page.contains("id=\"new-user\""), "{page}");
    // ?new=1 opens the New user dialog without script.
    let open = text(site.get("/staff/users?new=1", &admin).await).await;
    assert!(open.contains(
        r#"<div class="sx-modal" id="new-user" role="dialog" aria-modal="true" aria-labelledby="new-user-title">"#
    ));
    assert!(page.contains(
        r#"<div class="sx-modal" id="new-user" role="dialog" aria-modal="true" aria-labelledby="new-user-title" hidden"#
    ));
    for path in ["/staff/users", "/staff/users?new=1", "/staff"] {
        let page = text(site.get(path, &support).await).await;
        assert!(!page.contains("New user") && !page.contains("/staff/users/new"), "{path}: {page}");
    }
    // On a claim, Support has no pause, revoke or activate.
    let review = text(site.get(&format!("/staff/claims/{claim}"), &support).await).await;
    for action in ["pause", "revoke", "activate", "delete"] {
        assert!(!review.contains(&format!("action=\"/staff/claims/{claim}/{action}\"")), "{action}: {review}");
    }
    let review = text(site.get(&format!("/staff/claims/{claim}"), &admin).await).await;
    for action in ["pause", "revoke"] {
        assert!(review.contains(&format!("action=\"/staff/claims/{claim}/{action}\"")), "{action}: {review}");
    }
    // On the account, Support signs out and reads; the rest is an Admin's.
    let user = text(site.get(&format!("/staff/users/{ana}"), &support).await).await;
    assert!(user.contains(&format!("action=\"/staff/users/{ana}/sign-out\"")), "{user}");
    for action in ["ban", "role", "test", "test-tin"] {
        assert!(!user.contains(&format!("action=\"/staff/users/{ana}/{action}\"")), "{action}: {user}");
    }
    let user = text(site.get(&format!("/staff/users/{ana}"), &admin).await).await;
    for action in ["ban", "role", "test", "test-tin"] {
        assert!(user.contains(&format!("action=\"/staff/users/{ana}/{action}\"")), "{action}: {user}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_open_their_own_account_from_the_console(db: PgPool) {
    let site = Site::new(db);
    let (boss, cookie) = site.person(STAFF).await;
    add_claim(&site.db, boss, "000123451", "approved").await;
    let overview = text(site.get("/staff", &cookie).await).await;
    // "My account" in the sidebar and "Me" in the phone's tab bar lead there.
    assert!(tag(&overview, "class=\"sx-me\"").contains(&format!("href=\"/staff/users/{boss}\"")), "{overview}");
    let tabbar = &overview[overview.find("class=\"sx-tabbar\"").unwrap()..];
    assert!(tabbar.contains(&format!("<a href=\"/staff/users/{boss}\"><span class=\"i\">")), "{tabbar}");
    let page = text(site.get(&format!("/staff/users/{boss}"), &cookie).await).await;
    assert!(tag(&page, "class=\"sx-me\"").contains("aria-current=\"page\""), "{page}");
    assert!(!tag(&overview, "class=\"sx-me\"").contains("aria-current"), "{overview}");
    assert!(page.contains("<h1 class=\"\">My account</h1>"), "{page}");
    assert!(page.contains("This is your own account: another admin changes its role, ban or test label."));
    assert!(page.contains("000-123-451-00000"), "{page}");
    // Every tab is in the page; ?tab= shows one, the rest are hidden.
    let tins = text(site.get(&format!("/staff/users/{boss}?tab=tins"), &cookie).await).await;
    assert!(tins.contains(r#"id="tab-tins" aria-labelledby="tab-tins-tab">"#), "{tins}");
    assert!(tins.contains(r#"id="tab-account" aria-labelledby="tab-account-tab" hidden"#), "{tins}");
    assert!(tins.contains(r#"aria-controls="tab-tins" aria-selected="true""#), "{tins}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_console_loads_only_its_own_files(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "pending").await;
    let pages = [
        "/staff".to_owned(),
        "/staff/queue".to_owned(),
        "/staff/users".to_owned(),
        format!("/staff/users/{ana}"),
        format!("/staff/claims/{claim}"),
        "/staff/search?q=ana".to_owned(),
    ];
    for path in &pages {
        let response = site.get(path, &staff).await;
        let csp = response.headers().get(header::CONTENT_SECURITY_POLICY).expect(path).to_str().unwrap().to_owned();
        assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"), "{path}: {csp}");
        let page = text(response).await;
        assert!(page.contains("/assets/staff.css") && page.contains("/assets/staff.js"), "{path}");
        assert!(!page.contains("/assets/account.css") && !page.contains("/assets/home.css"), "{path}");
        // No other host: no Google Fonts, no absolute URL to load from.
        for host in ["googleapis", "gstatic", "src=\"http", "href=\"http"] {
            assert!(!page.contains(host), "{path}: {host}");
        }
        // No inline script or style for the policy to refuse.
        assert!(!page.contains("<script>") && !page.contains(" style=\""), "{path}");
        // The icons are in the page once, and every icon uses that copy: no
        // other file for the policy to refuse.
        assert_eq!(page.matches("<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"sx-sprite\"").count(), 1, "{path}");
        assert!(page.contains("<use href=\"#search\"></use>"), "{path}");
        assert!(!page.contains("<use href=\"/") && !page.contains("staff-icons.svg"), "{path}");
        // The policy is exactly this: each kind of file from this site only.
        assert_eq!(
            csp,
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; \
             connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            "{path}"
        );
    }
    // The owner's pages keep their own look and no new policy, the 404
    // that people get under /staff included.
    let refused = site.get("/staff", &ana_cookie).await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert!(refused.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
    let tins = site.get("/tins", &ana_cookie).await;
    assert!(tins.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
    assert!(text(tins).await.contains("/assets/account.css"));
    // The console's files, served like the site's own.
    for (path, kind) in [
        ("/assets/staff.css", "text/css; charset=utf-8"),
        ("/assets/staff.js", "text/javascript; charset=utf-8"),
        ("/assets/fonts/inter-variable.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-regular-latin1.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-medium-latin1.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-semibold-latin1.woff2", "font/woff2"),
    ] {
        let response = site.get(path, "").await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(content_type(&response), kind, "{path}");
    }
    // The icons are no separate file any more.
    assert_eq!(site.get("/assets/staff-icons.svg", "").await.status(), StatusCode::NOT_FOUND);
    // Every file staff.css names is served.
    let css = text(site.get("/assets/staff.css", "").await).await;
    for url in css.split("url(\"").skip(1).filter_map(|rest| rest.split('"').next()) {
        assert_eq!(site.get(url, "").await.status(), StatusCode::OK, "{url}");
    }
}

/// The sidebar link, the phone's tab bar entry and the page's own heading
/// that mark where the staff member is.
fn marked(page: &str) -> (bool, bool, bool, bool) {
    let side = &page[page.find("class=\"sx-nav\"").expect("a sidebar")..];
    let tabbar = &page[page.find("class=\"sx-tabbar\"").expect("a tab bar")..];
    let on = |html: &str, needle: &str| tag(html, needle).contains("aria-current=\"page\"");
    (
        on(side, "title=\"Overview\""),
        on(side, "title=\"Waiting\""),
        on(tabbar, "href=\"/staff\""),
        on(tabbar, "href=\"/staff/queue?status=waiting\""),
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn a_bare_staff_address_is_the_overview_for_admin_and_support(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    for (who, cookie) in [("admin", &admin), ("support", &support)] {
        let response = site.get("/staff", cookie).await;
        assert_eq!(response.status(), StatusCode::OK, "{who}");
        let page = text(response).await;
        assert!(page.contains("<h1 class=\"\">Overview</h1>"), "{who}: {page}");
        assert!(page.contains("<title>Overview · Staff · Buwiz</title>"), "{who}");
        // Overview is marked in the sidebar and the tab bar, the queue is not.
        assert_eq!(marked(&page), (true, false, true, false), "{who}");
        // Its links into the queue name the queue's own address.
        assert!(page.contains("href=\"/staff/queue?status=waiting\""), "{who}");
        assert!(!page.contains("href=\"/staff?"), "{who}");
    }
    // The earlier address of the Overview leads to it.
    let response = site.get("/staff/overview", &admin).await;
    assert_eq!((response.status(), location(&response)), (StatusCode::TEMPORARY_REDIRECT, "/staff".to_owned()));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_queue_lives_at_its_own_address_and_old_links_reach_it(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    add_claim(&site.db, ana, "000123451", "pending").await;
    add_claim(&site.db, ana, "000123453", "needs_fix").await;

    for cookie in [&admin, &support] {
        // The queue itself, marked in the sidebar and the tab bar.
        let page = text(site.get("/staff/queue", cookie).await).await;
        assert!(page.contains("<h1 class=\"\">Review queue</h1>") && page.contains("000-123-451-00000"), "{page}");
        assert_eq!(marked(&page), (false, true, false, true), "{page}");

        // A notice carried by an old `/staff?notice=…` link lands on the queue.
        let old = site.get("/staff?notice=claim_decided", cookie).await;
        assert!(old.status().is_redirection());
        let new = location(&old);
        assert_eq!(new, "/staff/queue?notice=claim_decided");
        let page = text(site.get(&new, cookie).await).await;
        assert!(page.contains("Decision saved."), "{page}");
        assert!(page.contains("Review queue") && page.contains("000-123-451-00000"), "{page}");

        // So does a filter, with the whole query kept.
        let old =
            site.get("/staff?status=sent_back&q=000123453&open=00000000-0000-0000-0000-000000000000", cookie).await;
        let new = location(&old);
        assert_eq!(new, "/staff/queue?status=sent_back&q=000123453&open=00000000-0000-0000-0000-000000000000");
        let page = text(site.get(&new, cookie).await).await;
        assert!(page.contains("000-123-453-00000") && !page.contains("000-123-451-00000"), "{page}");
        let filtered = text(site.get("/staff/queue?status=sent_back", cookie).await).await;
        assert!(filtered.contains("000-123-453-00000") && !filtered.contains("000-123-451-00000"), "{filtered}");
        assert_eq!(marked(&filtered), (false, false, false, true), "{filtered}");
    }

    // `/staff/claims` and `/admin/claims` end at the queue, not the Overview.
    for old in ["/staff/claims", "/admin/claims", "/admin"] {
        let first = site.get(old, &admin).await;
        assert_eq!(first.status(), StatusCode::PERMANENT_REDIRECT, "{old}");
        let page = text(site.get(&location(&first), &admin).await).await;
        assert!(page.contains("<h1 class=\"\">Review queue</h1>"), "{old}: {page}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_decision_lands_on_the_queue_with_its_notice(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123451", "pending").await;
    let response = site.post(&format!("/staff/claims/{claim}/decision"), &admin, "decision=approve").await;
    let landing = location(&response);
    assert_eq!(landing, "/staff/queue?notice=claim_decided");
    let page = text(site.get(&landing, &admin).await).await;
    assert!(page.contains("<h1 class=\"\">Review queue</h1>") && page.contains("Decision saved."), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn non_staff_get_what_they_always_got_at_the_staff_address(db: PgPool) {
    let site = Site::new(db);
    let (_, ana) = site.person("ana@example.test").await;
    for path in ["/staff", "/staff?notice=claim_decided", "/staff?status=waiting", "/staff/queue"] {
        let response = site.get(path, &ana).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let response = site.get(path, "").await;
        assert!(response.status().is_redirection(), "{path}");
        let to = location(&response);
        let next = form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>();
        assert!(to.starts_with("/login?") && to.contains(&format!("next={next}")), "{path}: {to}");
    }
}
