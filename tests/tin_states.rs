//! Adding a TIN without its papers ("Skip for now"), verifying it later on
//! its own page, and the add link the apps open (`/tins/new?tin=&branch=`).
//! Adding without papers looks the same whether or not another account has
//! the TIN. Each test gets its own database from `#[sqlx::test]`.

mod common;

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

/// Step 3's "Skip for now" for a personal TIN, with a file chosen anyway.
fn skipped(tin: &'static str) -> Vec<(&'static str, Part)> {
    vec![
        ("kind", Text("personal")),
        ("tin", Text(tin)),
        ("branch", Text("00000")),
        ("registered_name", Text("SYNTHETIC TAXPAYER")),
        ("tin_id", File("id.png", PNG)),
        ("skip", Text("1")),
    ]
}

async fn scalar_i64(db: &PgPool, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql).bind(id).fetch_one(db).await.unwrap()
}

/// The claim row's (status, papers sent).
async fn standing(db: &PgPool, claim: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT status, submitted_at IS NOT NULL FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(db)
        .await
        .unwrap()
}

/// `html` with each tag's attributes in name order. Topcoat writes the
/// attributes of a tag with conditional ones in no fixed order, so two
/// renders of one page can differ only there.
fn canonical(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let mut quoted = false;
        let end = rest.char_indices().find(|&(_, ch)| {
            if ch == '"' {
                quoted = !quoted;
            }
            ch == '>' && !quoted
        });
        let Some((end, _)) = end else { break };
        let tag = &rest[1..end];
        if tag.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
            let mut parts = Vec::new();
            let (mut current, mut quoted) = (String::new(), false);
            for ch in tag.chars() {
                match ch {
                    '"' => {
                        quoted = !quoted;
                        current.push(ch);
                    }
                    ' ' if !quoted => parts.push(std::mem::take(&mut current)),
                    _ => current.push(ch),
                }
            }
            parts.push(current);
            parts.retain(|part| !part.is_empty());
            parts[1..].sort();
            out.push('<');
            out.push_str(&parts.join(" "));
            out.push('>');
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `<article class="tin-card">` on My TINs that shows `tin`.
fn card<'a>(page: &'a str, tin: &str) -> &'a str {
    page.split("<article class=\"tin-card\">")
        .skip(1)
        .map(|card| &card[..card.find("</article>").unwrap()])
        .find(|card| card.contains(tin))
        .unwrap_or_else(|| panic!("no card for {tin}: {page}"))
}

#[sqlx::test(migrations = "./migrations")]
async fn a_tin_added_without_papers_is_not_verified(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;

    // Step 3 offers it, and says what it means.
    let add = text(site.get("/tins/new", &cookie).await).await;
    assert!(
        add.contains(
            r#"<button class="btn btn-ghost-dark" type="submit" name="skip" value="1" formnovalidate="" data-skip="">Skip for now</button>"#
        ),
        "{add}"
    );
    assert!(add.contains("No papers at hand? Skip for now: the TIN is saved as Not verified. You can prepare returns for it, and verify it later to send them."));
    // Without JavaScript, Enter presses a form's first submit button: it
    // must be "Send for review", never "Skip for now".
    let form = &add[add.find("action=\"/tins/claims\"").unwrap()..];
    let first_submit = &form[form.find(r#"type="submit""#).unwrap()..];
    assert!(first_submit.starts_with(r#"type="submit" data-send="""#), "{first_submit}");

    let response = site.upload("/tins/claims", &cookie, &skipped("000-123-451")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/tins?notice=tin_saved");
    let claim = site.newest_claim(ana).await;
    assert_eq!(standing(&site.db, claim).await, ("unverified".to_owned(), false));
    // The file chosen before skipping was not kept, nor read.
    assert_eq!(scalar_i64(&site.db, "SELECT count(*) FROM claim_documents WHERE claim_id = $1", claim).await, 0);
    assert_eq!(
        scalar_i64(
            &site.db,
            "SELECT count(*) FROM document_readings r JOIN claim_documents d ON d.id = r.document_id WHERE d.claim_id = $1",
            claim
        )
        .await,
        0
    );

    let page = text(site.get("/tins?notice=tin_saved", &cookie).await).await;
    assert!(page.contains(
        "Saved as Not verified. You can prepare returns for this TIN now; verify it with its papers to send them."
    ));
    let shown = card(&page, "000-123-451-00000");
    assert!(shown.contains("status-unverified") && shown.contains(">help</span>Not verified</span>"), "{shown}");
    assert!(shown.contains(r#"<li class="done">Saved</li>"#) && shown.contains(">Papers</li>"), "{shown}");
    assert!(!shown.contains(">Sent</li>"), "{shown}");
    assert!(shown.contains(r#"<p class="tin-note unverified">Not verified yet. You can prepare and save returns for this TIN in the app. To send them to the BIR, verify it with your TIN ID.</p>"#), "{shown}");
    assert!(
        shown.contains(&format!(r#"href="/tins/claims/{claim}/verify""#)) && shown.contains("Verify to send"),
        "{shown}"
    );
    assert!(shown.contains(r#"data-confirm="Remove this TIN from My TINs?""#), "{shown}");
    assert!(shown.contains("It has no papers yet, so nothing else is kept. Returns you prepared stay in the app."));
    assert!(!shown.contains("Withdraw") && !shown.contains("/files"), "{shown}");

    // Not in the review queue: there is nothing to review.
    let (_, staff) = site.person(STAFF).await;
    assert!(!text(site.get("/staff", &staff).await).await.contains("000-123-451"));

    // Once on My TINs, adding it again, with or without papers, says so.
    let again = site.upload("/tins/claims", &cookie, &skipped("000123451")).await;
    assert_eq!(location(&again), "/tins/new?notice=tin_already_added");
    let mut with_papers = skipped("000123451");
    with_papers.pop();
    assert_eq!(
        location(&site.upload("/tins/claims", &cookie, &with_papers).await),
        "/tins/new?notice=tin_already_added"
    );
    let page = text(site.get("/tins/new?notice=tin_already_added", &cookie).await).await;
    assert!(page.contains("This TIN is already on My TINs. Verify it there to send returns."));

    // The TIN and the name are still checked.
    assert_eq!(
        location(&site.upload("/tins/claims", &cookie, &skipped("000-000-000")).await),
        "/tins/new?notice=tin_zero"
    );
    let mut nameless = skipped("000123452");
    nameless[3] = ("registered_name", Text(" "));
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &nameless).await), "/tins/new?notice=name_missing");

    // A business TIN asks for its own papers.
    let business = [
        ("kind", Text("business")),
        ("tin", Text("000123453")),
        ("registered_name", Text("SYNTHETIC TRADING")),
        ("skip", Text("1")),
    ];
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &business).await), "/tins?notice=tin_saved");
    let page = text(site.get("/tins", &cookie).await).await;
    assert!(card(&page, "000-123-453-00000").contains("verify it with your Form 2303 and a government ID."));

    // At most 20 waiting to be verified.
    for n in 0..18 {
        sqlx::query(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ($1, 'personal', $2, '00000', 'X', 'unverified')",
        )
        .bind(ana)
        .bind(format!("000200{n:03}"))
        .execute(&site.db)
        .await
        .unwrap();
    }
    let response = site.upload("/tins/claims", &cookie, &skipped("000123454")).await;
    assert_eq!(location(&response), "/tins/new?notice=too_many_unverified");
    let page = text(site.get("/tins/new?notice=too_many_unverified", &cookie).await).await;
    assert!(page.contains("You have 20 TINs waiting to be verified. Verify or remove some first."));
    // Papers can still be sent for a new one.
    let mut sent = skipped("000123454");
    sent.pop();
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &sent).await), "/tins?notice=claim_submitted");
}

/// REVEAL-QUIET: what a person sees when adding a TIN without papers tells
/// nothing about other accounts. The same TIN may be added that way by any
/// number of accounts; only its owner owns it.
#[sqlx::test(migrations = "./migrations")]
async fn adding_without_papers_looks_the_same_for_a_tin_owned_elsewhere(db: PgPool) {
    let site = Site::new(db);
    let (carl, _) = site.person("carl@example.test").await;
    let owned = add_claim(&site.db, carl, "000123461", "approved").await;
    // Someone else has it waiting for review too, and someone else added it
    // without papers.
    let (dora, _) = site.person("dora@example.test").await;
    add_claim(&site.db, dora, "000123461", "pending").await;
    let (ana, cookie) = site.person("ana@example.test").await;
    let (bob, bob_cookie) = site.person("bob@example.test").await;

    // The answers, header by header (apart from the date).
    let headers = |response: &axum::response::Response| {
        let mut headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter(|(name, _)| *name != "date")
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        headers.sort();
        headers
    };
    let elsewhere = site.upload("/tins/claims", &cookie, &skipped("000-123-461")).await;
    let free = site.upload("/tins/claims", &cookie, &skipped("000-123-462")).await;
    assert_eq!((elsewhere.status(), headers(&elsewhere)), (free.status(), headers(&free)));
    assert_eq!(location(&elsewhere), "/tins?notice=tin_saved");
    assert_eq!(bytes(elsewhere).await, bytes(free).await);
    let bob_elsewhere = site.upload("/tins/claims", &bob_cookie, &skipped("000123461")).await;
    assert_eq!(location(&bob_elsewhere), "/tins?notice=tin_saved");

    // The two cards on ana's My TINs differ only by the TIN and the claim.
    let claims: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, tin_root FROM tin_claims WHERE account_id = $1")
        .bind(ana)
        .fetch_all(&site.db)
        .await
        .unwrap();
    let id_of = |root: &str| claims.iter().find(|(_, stored)| stored == root).unwrap().0;
    let page = text(site.get("/tins?notice=tin_saved", &cookie).await).await;
    let plain = |root: &str, tin: &str| {
        canonical(&card(&page, tin).replace(&id_of(root).to_string(), "ID").replace(tin, "TIN"))
    };
    assert_eq!(plain("000123461", "000-123-461-00000"), plain("000123462", "000-123-462-00000"));

    // So do their verify pages, and the rows the app lists.
    let verify = |root: &str, tin: &str| {
        let id = id_of(root);
        let (site, cookie) = (&site, &cookie);
        let tin = tin.to_owned();
        async move {
            let response = site.get(&format!("/tins/claims/{id}/verify"), cookie).await;
            assert_eq!(response.status(), StatusCode::OK);
            canonical(&text(response).await.replace(&id.to_string(), "ID").replace(&tin, "TIN"))
        }
    };
    assert_eq!(verify("000123461", "000-123-461-00000").await, verify("000123462", "000-123-462-00000").await);
    let app = app_token(&site.db, ana).await;
    let listed = profiles(&site, &app, "/api/tax-profiles?include=all").await;
    let row = |root: &str, tin: &str| {
        let id = id_of(root).to_string();
        let row = listed.as_array().unwrap().iter().find(|row| row["id"] == id.as_str()).unwrap();
        row.to_string().replace(&id, "ID").replace(tin, "TIN").replace(&tin[8..11], "LAST")
    };
    assert_eq!(row("000123461", "000-123-461-00000"), row("000123462", "000-123-462-00000"));

    // Every one of them is a row of its own; carl still owns it.
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT account_id, status FROM tin_claims WHERE tin_root = '000123461' ORDER BY status, account_id",
    )
    .fetch_all(&site.db)
    .await
    .unwrap();
    let mut expected = vec![(carl, "approved".to_owned()), (dora, "pending".to_owned())];
    let mut unverified = vec![(ana, "unverified".to_owned()), (bob, "unverified".to_owned())];
    unverified.sort();
    expected.extend(unverified);
    assert_eq!(rows, expected);
    assert_eq!(site.claim_status(owned).await, "approved");
}

#[sqlx::test(migrations = "./migrations")]
async fn verifying_later_sends_the_same_claim_for_review(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, bob) = site.person("bob@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    site.upload("/tins/claims", &cookie, &skipped("000123471")).await;
    let claim = site.newest_claim(ana).await;
    // It was added days ago; another claim was sent for review since.
    sqlx::query("UPDATE tin_claims SET created_at = now() - interval '9 days' WHERE id = $1")
        .bind(claim)
        .execute(&site.db)
        .await
        .unwrap();
    let (carl, carl_cookie) = site.person("carl@example.test").await;
    let carl_papers = [
        ("kind", Text("personal")),
        ("tin", Text("000123472")),
        ("registered_name", Text("CARL EXAMPLE")),
        ("tin_id", File("carl.png", PNG)),
    ];
    assert_eq!(
        location(&site.upload("/tins/claims", &carl_cookie, &carl_papers).await),
        "/tins?notice=claim_submitted"
    );
    let carl_claim = site.newest_claim(carl).await;
    sqlx::query("UPDATE tin_claims SET created_at = now() - interval '1 day', submitted_at = now() - interval '1 day' WHERE id = $1")
        .bind(carl_claim)
        .execute(&site.db)
        .await
        .unwrap();

    let path = format!("/tins/claims/{claim}/verify");
    let page = text(site.get(&path, &cookie).await).await;
    for part in [
        "<title>Verify a TIN · Buwiz</title>",
        r#"<a class="back" href="/tins">"#,
        "<h1>Verify this TIN</h1>",
        "000-123-471-00000",
        "SYNTHETIC TAXPAYER",
        "Send the papers to start sending returns for this TIN. A person checks them, usually within one business day.",
        r#"name="tin_id""#,
        r#"name="selfie""#,
        r#"name="national_id""#,
        "It goes to the review queue.",
        "Send for review",
        "Files are encrypted and deleted after review.",
        &format!(r#"action="/tins/claims/{claim}/papers""#),
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    assert!(!page.contains(r#"name="form_2303""#));
    // Someone else's claim is not there for them.
    assert_eq!(site.get(&path, &bob).await.status(), StatusCode::NOT_FOUND);

    let papers = format!("/tins/claims/{claim}/papers");
    let no_file = [("registered_name", Text("SYNTHETIC TAXPAYER"))];
    assert_eq!(location(&site.upload(&papers, &cookie, &no_file).await), format!("{path}?notice=need_tin_id"));
    let with_file = [("registered_name", Text("SYNTHETIC M. TAXPAYER")), ("tin_id", File("card.png", PNG))];
    assert_eq!(location(&site.upload(&papers, &bob, &with_file).await), "/tins?notice=claim_missing");
    assert_eq!(standing(&site.db, claim).await, ("unverified".to_owned(), false));

    let response = site.upload(&papers, &cookie, &with_file).await;
    assert_eq!(location(&response), "/tins?notice=claim_submitted");
    // The same claim, so the app's profile keeps its id.
    assert_eq!(site.newest_claim(ana).await, claim);
    assert_eq!(standing(&site.db, claim).await, ("pending".to_owned(), true));
    let name: String = sqlx::query_scalar("SELECT registered_name FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(name, "SYNTHETIC M. TAXPAYER");
    assert_eq!(scalar_i64(&site.db, "SELECT count(*) FROM claim_documents WHERE claim_id = $1", claim).await, 1);
    // Its TIN ID is read like any claim's.
    assert_eq!(site.readings(claim).await.len(), 1);

    // It waits in the queue from when the papers came: after carl's.
    let queue = text(site.get("/staff", &staff).await).await;
    let (ours, carls) = (queue.find("000-123-471").unwrap(), queue.find("000-123-472").unwrap());
    assert!(carls < ours, "{queue}");

    // Its papers are in; the verify page and the post say so.
    assert_eq!(location(&site.upload(&papers, &cookie, &with_file).await), "/tins?notice=not_unverified");
    let response = site.get(&path, &cookie).await;
    assert!(response.status().is_redirection());
    assert_eq!(location(&response), "/tins?notice=not_unverified");
    let page = text(site.get("/tins?notice=not_unverified", &cookie).await).await;
    assert!(page.contains("This TIN already has its papers. See where it stands on My TINs."));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_business_tin_is_verified_with_its_2303_and_an_id(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let business = [
        ("kind", Text("business")),
        ("tin", Text("000123481")),
        ("registered_name", Text("SYNTHETIC TRADING")),
        ("skip", Text("1")),
    ];
    site.upload("/tins/claims", &cookie, &business).await;
    let claim = site.newest_claim(ana).await;
    let page = text(site.get(&format!("/tins/claims/{claim}/verify"), &cookie).await).await;
    for field in [
        r#"name="form_2303""#,
        r#"name="government_id""#,
        r#"name="authority""#,
        r#"name="supporting""#,
        r#"name="trade_name""#,
    ] {
        assert!(page.contains(field), "{field}");
    }
    assert!(!page.contains(r#"name="tin_id""#));
    let papers = format!("/tins/claims/{claim}/papers");
    let only_2303 = [("registered_name", Text("SYNTHETIC TRADING")), ("form_2303", File("2303.pdf", PDF))];
    assert_eq!(
        location(&site.upload(&papers, &cookie, &only_2303).await),
        format!("/tins/claims/{claim}/verify?notice=need_id")
    );
    let selfie = [
        ("registered_name", Text("SYNTHETIC TRADING")),
        ("form_2303", File("2303.pdf", PDF)),
        ("government_id", File("id.png", PNG)),
        ("selfie", File("me.png", PNG)),
    ];
    assert_eq!(
        location(&site.upload(&papers, &cookie, &selfie).await),
        format!("/tins/claims/{claim}/verify?notice=wrong_documents")
    );
    let good = [
        ("registered_name", Text("SYNTHETIC TRADING")),
        ("trade_name", Text("Synthetic Shop")),
        ("form_2303", File("2303.pdf", PDF)),
        ("government_id", File("id.png", PNG)),
    ];
    assert_eq!(location(&site.upload(&papers, &cookie, &good).await), "/tins?notice=claim_submitted");
    let row: (String, Option<String>) = sqlx::query_as("SELECT status, trade_name FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(row, ("pending".to_owned(), Some("Synthetic Shop".to_owned())));
}

/// The value attributes of the TIN and branch fields on Add a TIN.
fn fields(page: &str) -> (String, String) {
    let value = |id: &str| {
        let input = &page[page.find(&format!(r#"<input id="{id}""#)).unwrap()..];
        let input = &input[..input.find('>').unwrap()];
        input.split(" value=\"").nth(1).map(|rest| rest.split('"').next().unwrap().to_owned()).unwrap_or_default()
    };
    (value("tin"), value("branch"))
}

#[sqlx::test(migrations = "./migrations")]
async fn the_add_link_prefills_or_goes_to_the_tins_own_page(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;

    let page = text(site.get("/tins/new", &cookie).await).await;
    assert_eq!(fields(&page), (String::new(), "00000".to_owned()));
    let page = text(site.get("/tins/new?tin=000123491&branch=00002", &cookie).await).await;
    assert_eq!(fields(&page), ("000-123-491".to_owned(), "00002".to_owned()));
    let page = text(site.get("/tins/new?tin=000-123-491&branch=002", &cookie).await).await;
    assert_eq!(fields(&page), ("000-123-491".to_owned(), "00002".to_owned()));
    let page = text(site.get("/tins/new?tin=000-123-491-00003", &cookie).await).await;
    assert_eq!(fields(&page), ("000-123-491".to_owned(), "00003".to_owned()));
    // Anything that is not a TIN is ignored, and never put on the page.
    for query in [
        "tin=%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "tin=000000000",
        "tin=000-000-000",
        "tin=12345",
        "tin=000123491&branch=12",
    ] {
        let page = text(site.get(&format!("/tins/new?{query}"), &cookie).await).await;
        assert_eq!(fields(&page), (String::new(), "00000".to_owned()), "{query}");
        assert!(!page.contains("<script>alert") && !page.contains("value=\"000-000-000\""), "{query}");
    }

    // Signed out, it asks for a sign-in and comes back to the same link.
    let response = site.get("/tins/new?tin=000123491&branch=00000", "").await;
    assert!(response.status().is_redirection());
    assert_eq!(
        location(&response),
        "/login?next=%2Ftins%2Fnew%3Ftin%3D000123491%26branch%3D00000&notice=sign_in_first"
    );

    // A TIN already on the account goes to where it stands.
    site.upload("/tins/claims", &cookie, &skipped("000123492")).await;
    let unverified = site.newest_claim(ana).await;
    let fix = add_claim(&site.db, ana, "000123493", "needs_fix").await;
    add_claim(&site.db, ana, "000123494", "pending").await;
    add_claim(&site.db, ana, "000123495", "approved").await;
    let paused = add_claim(&site.db, ana, "000123496", "approved").await;
    sqlx::query("UPDATE tin_claims SET paused_at = now(), pause_reason = 'Checking.' WHERE id = $1")
        .bind(paused)
        .execute(&site.db)
        .await
        .unwrap();
    for (query, to) in [
        ("tin=000123492", format!("/tins/claims/{unverified}/verify")),
        ("tin=000-123-492&branch=00000", format!("/tins/claims/{unverified}/verify")),
        ("tin=000123493", format!("/tins/claims/{fix}/fix")),
        ("tin=000123494", "/tins?notice=claim_already_pending".to_owned()),
        ("tin=000123495", "/tins?notice=tin_already_active".to_owned()),
        // A paused TIN never reads as active (review finding 9).
        ("tin=000123496", "/tins?notice=tin_is_paused".to_owned()),
    ] {
        let response = site.get(&format!("/tins/new?{query}"), &cookie).await;
        assert!(response.status().is_redirection(), "{query}");
        assert_eq!(location(&response), to, "{query}");
    }
    // A branch of a TIN on the account is another TIN.
    let page = text(site.get("/tins/new?tin=000123492&branch=00001", &cookie).await).await;
    assert_eq!(fields(&page), ("000-123-492".to_owned(), "00001".to_owned()));

    // Someone else's TIN, active or ended there, is just prefilled, like a
    // TIN nobody has.
    let (bob, _) = site.person("bob@example.test").await;
    add_claim(&site.db, bob, "000123497", "approved").await;
    add_claim(&site.db, ana, "000123498", "rejected").await;
    let free = canonical(&text(site.get("/tins/new?tin=000123499", &cookie).await).await.replace("000-123-499", "TIN"));
    for (root, tin) in [("000123497", "000-123-497"), ("000123498", "000-123-498")] {
        let page = text(site.get(&format!("/tins/new?tin={root}"), &cookie).await).await;
        assert_eq!(canonical(&page.replace(tin, "TIN")), free, "{root}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unverified_tin_can_be_deleted(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, bob) = site.person("bob@example.test").await;
    site.upload("/tins/claims", &cookie, &skipped("000123501")).await;
    let claim = site.newest_claim(ana).await;
    let delete = format!("/tins/claims/{claim}/delete");
    assert_eq!(location(&site.post(&delete, &bob, "").await), "/tins?notice=claim_missing");
    assert_eq!(location(&site.post(&delete, &cookie, "").await), "/tins?notice=claim_deleted");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let audit: (String, String) = sqlx::query_as("SELECT action, claim_status FROM claim_audit WHERE claim_id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(audit, ("deleted".to_owned(), "unverified".to_owned()));
    // It can be added again.
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &skipped("000123501")).await), "/tins?notice=tin_saved");
}

/// The limit of 20 holds when many are saved at once.
#[sqlx::test(migrations = "./migrations")]
async fn the_limit_of_unverified_tins_holds_under_parallel_requests(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    for n in 0..19 {
        sqlx::query(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ($1, 'personal', $2, '00000', 'X', 'unverified')",
        )
        .bind(ana)
        .bind(format!("000200{n:03}"))
        .execute(&site.db)
        .await
        .unwrap();
    }
    let forms: Vec<Vec<(&'static str, Part)>> =
        (0..10).map(|n| skipped(Box::leak(format!("000300{n:03}").into_boxed_str()))).collect();
    let answers =
        futures_util::future::join_all(forms.iter().map(|form| site.upload("/tins/claims", &cookie, form))).await;
    let saved = answers.iter().filter(|response| location(response) == "/tins?notice=tin_saved").count();
    assert_eq!(saved, 1);
    let total =
        scalar_i64(&site.db, "SELECT count(*) FROM tin_claims WHERE account_id = $1 AND status = 'unverified'", ana)
            .await;
    assert_eq!(total, 20);
}
