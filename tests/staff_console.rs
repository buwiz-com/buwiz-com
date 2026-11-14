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
        ("/staff?status=waiting", 2),
        ("/staff?status=sent_back", 1),
        ("/staff?status=auto", 1),
        ("/staff/users", 6),
        ("/staff/users?filter=staff", 2),
        ("/staff/users?filter=test", 1),
        ("/staff/users?filter=banned", 1),
    ] {
        let page = text(site.get(path, &staff).await).await;
        assert_eq!(before_inspector(&page).matches("<tr data-id=").count(), rows, "{path}");
    }
    // The sidebar shows them, the waiting count as hot.
    let page = text(site.get("/staff/overview", &staff).await).await;
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
    assert_eq!((numbers.accounts, numbers.accounts_week), (4, 4));
    assert!(numbers.oldest_waiting.is_some());

    let admin = text(site.get("/staff/overview", &staff).await).await;
    assert!(admin.contains("Claims waiting</span><span class=\"n\">2</span>"), "{admin}");
    assert!(admin.contains("Oldest 1 d 2 h"), "{admin}");
    assert!(admin.contains("Active TINs</span><span class=\"n\">2</span>"), "{admin}");
    assert!(admin.contains("Accounts</span><span class=\"n\">4</span>"), "{admin}");
    assert!(admin.contains("Activated automatically</span><span class=\"n\">1</span>"), "{admin}");
    // Today began at midnight in Manila: the three sends of the last seconds
    // fall in it (all but the newest only if the test runs at midnight).
    let start = admin.find("Returns sent today").expect("an Admin sees returns sent today");
    let sent: i64 =
        admin[start..].split("<span class=\"n\">").nth(1).unwrap().split('<').next().unwrap().parse().unwrap();
    assert!((1..=3).contains(&sent), "{sent}");
    // Returns are an Admin's number: no staff page lists them for Support.
    let support = text(site.get("/staff/overview", &support).await).await;
    assert!(!support.contains("Returns sent today"), "{support}");
    assert!(support.contains("Claims waiting</span><span class=\"n\">2</span>"), "{support}");
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
    let page = text(site.get("/staff/overview", &staff).await).await;
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

    // Nothing chosen: the oldest claim, closed on tablets and phones.
    let page = text(site.get("/staff", &staff).await).await;
    let side = inspector(&page);
    assert!(side.starts_with("id=\"inspector\" class=\"sx-insp\""), "{side}");
    assert!(side.contains("000-123-451-00000") && !side.contains("000-123-452-00000"), "{side}");
    assert!(side.contains(&format!("action=\"/staff/claims/{first}/decision\"")), "{side}");
    assert!(page.contains(&format!("<tr data-id=\"{first}\" class=\"sel auto\">")), "{page}");
    // `?open=` chooses the row and opens the panel, wherever it is in the list.
    let page = text(site.get(&format!("/staff?status=waiting&open={second}"), &staff).await).await;
    let side = inspector(&page);
    assert!(side.starts_with("id=\"inspector\" class=\"sx-insp is-open\""), "{side}");
    assert!(side.contains("000-123-452-00000") && side.contains("bob@example.test"), "{side}");
    assert!(side.contains(&format!("action=\"/staff/claims/{second}/decision\"")), "{side}");
    assert!(page.contains(&format!("<tr data-id=\"{second}\" class=\"sel\">")), "{page}");
    // Its close link is the same list without the choice.
    assert!(side.contains("href=\"/staff?status=waiting\" data-close-insp=\"\""), "{side}");
    // A claim that is not there leaves the panel empty.
    let page = text(site.get(&format!("/staff?open={}", Uuid::new_v4()), &staff).await).await;
    assert!(inspector(&page).contains("Nothing to review in this tab."));
    // The users list does the same with an account.
    let page = text(site.get(&format!("/staff/users?open={bob}"), &staff).await).await;
    let side = inspector(&page);
    assert!(side.contains("bob@example.test") && side.contains(&format!("href=\"/staff/users/{bob}\"")), "{side}");
    assert!(side.contains("000-123-452-00000"), "{side}");
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
    for path in ["/staff/users", "/staff/users?new=1", "/staff/overview"] {
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
    let overview = text(site.get("/staff/overview", &cookie).await).await;
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
        "/staff/overview".to_owned(),
        "/staff".to_owned(),
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
    }
    // The owner's pages keep their own look and no new policy, the 404
    // that people get under /staff included.
    let refused = site.get("/staff/overview", &ana_cookie).await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert!(refused.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
    let tins = site.get("/tins", &ana_cookie).await;
    assert!(tins.headers().get(header::CONTENT_SECURITY_POLICY).is_none());
    assert!(text(tins).await.contains("/assets/account.css"));
    // The console's files, served like the site's own.
    for (path, kind) in [
        ("/assets/staff.css", "text/css; charset=utf-8"),
        ("/assets/staff.js", "text/javascript; charset=utf-8"),
        ("/assets/staff-icons.svg", "image/svg+xml"),
        ("/assets/fonts/inter-variable.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-regular-latin1.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-medium-latin1.woff2", "font/woff2"),
        ("/assets/fonts/ibm-plex-mono-semibold-latin1.woff2", "font/woff2"),
    ] {
        let response = site.get(path, "").await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(content_type(&response), kind, "{path}");
    }
    // Every file staff.css names is served.
    let css = text(site.get("/assets/staff.css", "").await).await;
    for url in css.split("url(\"").skip(1).filter_map(|rest| rest.split('"').next()) {
        assert_eq!(site.get(url, "").await.status(), StatusCode::OK, "{url}");
    }
}
