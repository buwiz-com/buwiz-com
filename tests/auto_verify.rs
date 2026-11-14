//! Automatic activation (`AUTO_VERIFY=on`), wired through a claim.
//!
//! These tests run without the `ocr` feature: every reading the server
//! takes ends "OCR not built in", which the check records as a doubt. To
//! stand for a clear TIN ID photo, a test then writes the reading row
//! itself, as OCR would have, and calls the hook that runs after a reading
//! is saved (`verify::consider_document`). The TINs, names and photos are
//! synthetic.

mod common;

use buwiz_cloud::account::verify::{self, Doubt, Outcome, Verification};
use buwiz_cloud::filing::gate;
use buwiz_cloud::tin::Tin;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

const NAME: &str = "Juana Santos Dela Cruz";

/// A personal claim with a TIN ID photo, sent by `email` for `root`, with
/// its server reading done: (account, cookie, claim, TIN ID document).
async fn sent(site: &Site, email: &str, root: &'static str) -> (Uuid, String, Uuid, Uuid) {
    let (account, cookie) = match sqlx::query_scalar::<_, Uuid>("SELECT id FROM accounts WHERE email = $1")
        .bind(email)
        .fetch_optional(&site.db)
        .await
        .unwrap()
    {
        Some(id) => {
            (id, format!("buwiz_session={}", buwiz_cloud::accounts::start_session(&site.db, id).await.unwrap()))
        }
        None => site.person(email).await,
    };
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text(root)),
        ("registered_name", Text(NAME)),
        ("tin_id", File("card.png", PNG)),
    ];
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &parts).await), "/tins?notice=claim_submitted");
    let claim = site.newest_claim(account).await;
    let document = tin_id(&site.db, claim).await;
    settle(site, claim).await;
    (account, cookie, claim, document)
}

async fn tin_id(db: &PgPool, claim: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM claim_documents WHERE claim_id = $1 AND kind = 'tin_id' AND purged_at IS NULL")
        .bind(claim)
        .fetch_one(db)
        .await
        .unwrap()
}

/// Waits for the server's own reading and, with automatic activation on,
/// for the check that runs after it.
async fn settle(site: &Site, claim: Uuid) {
    site.readings(claim).await;
    if site.state.verification == Verification::On {
        for _ in 0..100 {
            if auto_check(&site.db, claim).await.is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the check after the reading did not run");
    }
}

async fn auto_check(db: &PgPool, claim: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT auto_check FROM tin_claims WHERE id = $1").bind(claim).fetch_one(db).await.unwrap()
}

/// Writes the reading OCR would have stored for a clear photo of a TIN ID
/// showing `tin` and `name`.
async fn read_as(db: &PgPool, document: Uuid, tin: &str, name: &str, sure: f64) {
    let fields = json!({ "kind": "tin_id", "tin": tin, "name": name, "trade_name": null, "birth_date": "1990-01-15", "rdo": null });
    let confidence = json!({ "tin": sure, "name": sure, "trade_name": null, "birth_date": sure, "rdo": null });
    sqlx::query(
        "UPDATE document_readings SET status = 'read', engine = 'paddleocr', engine_version = 'test', error = NULL,
             fields = $2, confidence = $3, tin_match = 'match', name_match = 'match', read_at = now()
         WHERE document_id = $1",
    )
    .bind(document)
    .bind(fields)
    .bind(confidence)
    .execute(db)
    .await
    .unwrap();
}

async fn read_clearly(db: &PgPool, document: Uuid, root: &str) {
    let tin = format!("{}-{}-{}-000", &root[..3], &root[3..6], &root[6..]);
    read_as(db, document, &tin, "DELA CRUZ, JUANA SANTOS", 0.97).await;
}

/// (status, verified_by, decided_at set, decided_by set, auto_confirmed_at set)
async fn row(db: &PgPool, claim: Uuid) -> (String, Option<String>, bool, bool, bool) {
    sqlx::query_as(
        "SELECT status, verified_by, decided_at IS NOT NULL, decided_by IS NOT NULL, auto_confirmed_at IS NOT NULL
         FROM tin_claims WHERE id = $1",
    )
    .bind(claim)
    .fetch_one(db)
    .await
    .unwrap()
}

/// Files of the claim still stored, and readings left.
async fn papers(db: &PgPool, claim: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE d.purged_at IS NULL AND d.ciphertext IS NOT NULL), count(r.id)
         FROM claim_documents d LEFT JOIN document_readings r ON r.document_id = d.id WHERE d.claim_id = $1",
    )
    .bind(claim)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn consider(site: &Site, document: Uuid) -> Outcome {
    verify::consider(&site.db, site.state.verification, document).await.unwrap()
}

fn doubts(outcome: Outcome) -> Vec<Doubt> {
    match outcome {
        Outcome::LeftForStaff(_, doubts) => doubts,
        other => panic!("not left for staff: {other:?}"),
    }
}

/// The claims listed in the staff tab "Activated automatically".
async fn auto_tab(site: &Site, staff: &str) -> String {
    text(site.get("/staff/queue?status=auto", staff).await).await
}

#[sqlx::test(migrations = "./migrations")]
async fn a_clear_tin_id_activates_by_itself_and_waits_for_a_staff_check(db: PgPool) {
    let site = Site::with_auto(db);
    let (support, support_cookie) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _, claim, document) = sent(&site, "ana@example.test", "000123453").await;

    // The server's own reading failed (no OCR in this build): the check
    // ran after it and left the claim for staff, saying why.
    let check = auto_check(&site.db, claim).await.unwrap();
    assert_eq!(check["outcome"], "left_for_staff");
    assert_eq!(check["doubts"][0]["code"], "read_failed", "{check}");
    let page = text(site.get(&format!("/staff/claims/{claim}"), &support_cookie).await).await;
    assert!(page.contains("Not activated automatically"), "{page}");
    assert!(page.contains("the TIN ID couldn") && page.contains("t be read: OCR not built in"), "{page}");
    assert_eq!(row(&site.db, claim).await.0, "pending");

    // A clear reading of the photo.
    read_clearly(&site.db, document, "000123453").await;
    verify::consider_document(&site.state, document).await;
    assert_eq!(row(&site.db, claim).await, ("approved".into(), Some("auto".into()), true, false, false));
    let check = auto_check(&site.db, claim).await.unwrap();
    assert_eq!(check["outcome"], "activated");
    assert_eq!(check["document_id"], document.to_string());
    // The papers are kept for a person to look at.
    assert_eq!(papers(&site.db, claim).await, (1, 1));
    let lines = claim_audit_lines(&site.db, ana).await;
    assert_eq!(
        lines,
        [(
            "auto_activated".into(),
            "pending".into(),
            "ana@example.test".into(),
            "auto".into(),
            Some(verify::ACTIVATED_REASON.into())
        )]
    );
    // It sends.
    let tin = Tin::parse("000123453", "").unwrap();
    assert!(gate::own(&site.db, ana, &tin).await.unwrap().can_send());

    // Staff see it in its tab, with its count, and the card to confirm it.
    let tab = auto_tab(&site, &support_cookie).await;
    assert!(tab.contains(&format!("<tr data-id=\"{claim}\"")), "{tab}");
    assert!(tab.contains("Activated automatically · check"), "{tab}");
    let page = text(site.get(&format!("/staff/claims/{claim}"), &support_cookie).await).await;
    assert!(page.contains("TIN and name matched the TIN ID read on our server (97% and 97% sure)"), "{page}");
    assert!(page.contains(&format!("action=\"/staff/claims/{claim}/confirm\"")), "{page}");

    // Support confirms: the papers and their reading are deleted.
    let response = site.post(&format!("/staff/claims/{claim}/confirm"), &support_cookie, "").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=auto_confirmed"));
    assert_eq!(row(&site.db, claim).await, ("approved".into(), Some("auto".into()), true, false, true));
    assert_eq!(papers(&site.db, claim).await, (0, 0));
    let confirmed_by: Option<Uuid> = sqlx::query_scalar("SELECT auto_confirmed_by FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(confirmed_by, Some(support));
    let last = claim_audit_lines(&site.db, ana).await.pop().unwrap();
    assert_eq!((last.0.as_str(), last.2.as_str(), last.3.as_str()), ("auto_confirmed", "helper@example.test", "web"));
    assert!(!auto_tab(&site, &support_cookie).await.contains(&format!("<tr data-id=\"{claim}\"")));
    // Once is enough.
    let again = site.post(&format!("/staff/claims/{claim}/confirm"), &support_cookie, "").await;
    assert_eq!(location(&again), format!("/staff/claims/{claim}?notice=not_auto"));
}

#[sqlx::test(migrations = "./migrations")]
async fn with_auto_verify_unset_nothing_ever_activates(db: PgPool) {
    // The test runner sets no AUTO_VERIFY, as a server without it.
    assert!(std::env::var_os("AUTO_VERIFY").is_none());
    let site = Site::new(db);
    assert_eq!(site.state.verification, Verification::Off);
    let (_, _, claim, document) = sent(&site, "ana@example.test", "000123454").await;
    read_clearly(&site.db, document, "000123454").await;
    verify::consider_document(&site.state, document).await;
    // Nothing changed and nothing was even recorded.
    assert_eq!(row(&site.db, claim).await, ("pending".into(), None, false, false, false));
    assert!(auto_check(&site.db, claim).await.is_none());
    // Called directly with the setting off, the rule refuses as well.
    assert_eq!(doubts(consider(&site, document).await), [Doubt::Off]);
    assert_eq!(row(&site.db, claim).await.0, "pending");
}

#[sqlx::test(migrations = "./migrations")]
async fn anything_in_doubt_leaves_the_claim_for_staff(db: PgPool) {
    let site = Site::with_auto(db);

    // A banned claimant.
    let (ana, _, claim, document) = sent(&site, "ana@example.test", "000123461").await;
    read_clearly(&site.db, document, "000123461").await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Abuse.' WHERE id = $1")
        .bind(ana)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(doubts(consider(&site, document).await), [Doubt::Banned]);
    assert_eq!(row(&site.db, claim).await.0, "pending");

    // Another account saved the TIN without papers (as personal: a
    // business claim on it is a doubt of its own).
    let (carl, _) = site.person("carl@example.test").await;
    let carls = add_claim(&site.db, carl, "000123462", "unverified").await;
    sqlx::query("UPDATE tin_claims SET kind = 'personal' WHERE id = $1").bind(carls).execute(&site.db).await.unwrap();
    let (_, _, _, document) = sent(&site, "bob@example.test", "000123462").await;
    read_clearly(&site.db, document, "000123462").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::OtherAccount]);

    // The name only partly matches.
    let (_, _, _, document) = sent(&site, "dan@example.test", "000123463").await;
    read_as(&site.db, document, "000-123-463-000", "DELA CRUZ, JUANA S", 0.97).await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::NameDiffers]);

    // OCR was not sure enough.
    let (_, _, _, document) = sent(&site, "eve@example.test", "000123464").await;
    read_as(&site.db, document, "000-123-464-000", "DELA CRUZ, JUANA SANTOS", 0.89).await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::LowConfidence]);

    // Sent back once: a person finishes it, even after the fix.
    let (_, fay_cookie, claim, document) = sent(&site, "fay@example.test", "000123465").await;
    let (_, staff) = site.person(STAFF).await;
    site.post(&format!("/staff/claims/{claim}/decision"), &staff, "decision=fix&reasons=blurry").await;
    let fix = [("registered_name", Text(NAME)), ("tin_id", File("card2.png", PNG))];
    assert_eq!(
        location(&site.upload(&format!("/tins/claims/{claim}/resend"), &fay_cookie, &fix).await),
        "/tins?notice=claim_resent"
    );
    let _ = document;
    site.readings(claim).await;
    let document = tin_id(&site.db, claim).await;
    read_clearly(&site.db, document, "000123465").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::SentBackBefore]);

    // Business TINs never: they carry no TIN ID, and a 2303 is never
    // enough.
    let (gus, gus_cookie) = site.person("gus@example.test").await;
    let parts = [
        ("kind", Text("business")),
        ("tin", Text("000123466")),
        ("registered_name", Text(NAME)),
        ("form_2303", File("2303.png", PNG)),
        ("government_id", File("id.png", PNG)),
    ];
    assert_eq!(location(&site.upload("/tins/claims", &gus_cookie, &parts).await), "/tins?notice=claim_submitted");
    let business = site.newest_claim(gus).await;
    settle(&site, business).await;
    let form: Uuid = sqlx::query_scalar("SELECT id FROM claim_documents WHERE claim_id = $1 AND kind = 'form_2303'")
        .bind(business)
        .fetch_one(&site.db)
        .await
        .unwrap();
    read_clearly(&site.db, form, "000123466").await;
    assert_eq!(doubts(consider(&site, form).await), [Doubt::NotPersonal, Doubt::NoTinId]);

    // A second one on the same account while the first waits for a person.
    let (_, _, first, document) = sent(&site, "hal@example.test", "000123467").await;
    read_clearly(&site.db, document, "000123467").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(first));
    let (_, _, _, document) = sent(&site, "hal@example.test", "000123468").await;
    read_clearly(&site.db, document, "000123468").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::AnotherUnconfirmed]);

    // Each is shown to the reviewer in plain words.
    let page = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(page.contains("it was sent back before, so a person finishes it"), "{page}");
}

/// Readings that finish at once are counted one after another: the daily
/// limit and the one-per-account limit both hold.
#[sqlx::test(migrations = "./migrations")]
async fn the_limits_hold_when_readings_finish_at_once(db: PgPool) {
    let site = Site::with_auto(db);
    // 19 automatic activations already today.
    let (old, _) = site.person("old@example.test").await;
    for _ in 0..verify::DAILY_LIMIT - 1 {
        sqlx::query(
            "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
             VALUES (gen_random_uuid(), $1, '***-***-000-00000', 'auto_activated', 'pending', $1, 'old@example.test', 'x', 'auto')",
        )
        .bind(old)
        .execute(&site.db)
        .await
        .unwrap();
    }
    let mut documents = Vec::new();
    for (n, root) in ["000123471", "000123472", "000123473", "000123474", "000123475"].into_iter().enumerate() {
        let (_, _, _, document) = sent(&site, &format!("p{n}@example.test"), root).await;
        read_clearly(&site.db, document, root).await;
        documents.push(document);
    }
    let outcomes = futures_util::future::join_all(documents.iter().map(|document| consider(&site, *document))).await;
    let activated = outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Activated(_))).count();
    assert_eq!(activated, 1, "{outcomes:?}");
    for outcome in outcomes.into_iter().filter(|outcome| !matches!(outcome, Outcome::Activated(_))) {
        assert_eq!(doubts(outcome), [Doubt::DailyLimit]);
    }

    // One account, two TINs read at once: one activates.
    sqlx::query("DELETE FROM claim_audit WHERE account_id = $1").bind(old).execute(&site.db).await.unwrap();
    let (_, _, _, first) = sent(&site, "ivy@example.test", "000123476").await;
    let (_, _, _, second) = sent(&site, "ivy@example.test", "000123477").await;
    read_clearly(&site.db, first, "000123476").await;
    read_clearly(&site.db, second, "000123477").await;
    let (a, b) = tokio::join!(consider(&site, first), consider(&site, second));
    let outcomes = [a, b];
    assert_eq!(outcomes.iter().filter(|outcome| matches!(outcome, Outcome::Activated(_))).count(), 1, "{outcomes:?}");
    assert!(
        outcomes.iter().any(
            |outcome| matches!(outcome, Outcome::LeftForStaff(_, doubts) if *doubts == [Doubt::AnotherUnconfirmed])
        ),
        "{outcomes:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_document_replaced_while_it_was_read_activates_nothing(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, cookie, claim, old) = sent(&site, "ana@example.test", "000123481").await;
    // The person replaces the TIN ID while OCR still reads the old one.
    sqlx::query("UPDATE document_readings SET status = 'reading' WHERE document_id = $1")
        .bind(old)
        .execute(&site.db)
        .await
        .unwrap();
    let replace = [("tin_id", File("other.png", PNG))];
    let path = format!("/tins/claims/{claim}/documents/{old}/replace");
    assert_eq!(
        location(&site.upload(&path, &cookie, &replace).await),
        format!("/tins/claims/{claim}/files?notice=file_replaced")
    );
    // The old reading finishing finds its document gone.
    assert_eq!(consider(&site, old).await, Outcome::Gone);
    assert_eq!(row(&site.db, claim).await.0, "pending");
    // The new file is judged on its own reading.
    site.readings(claim).await;
    let new = tin_id(&site.db, claim).await;
    assert_ne!(new, old);
    sqlx::query("UPDATE document_readings SET status = 'reading' WHERE document_id = $1")
        .bind(new)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(doubts(consider(&site, new).await), [Doubt::NotRead]);
    read_clearly(&site.db, new, "000123481").await;
    assert_eq!(consider(&site, new).await, Outcome::Activated(claim));
}

#[sqlx::test(migrations = "./migrations")]
async fn an_activation_removed_or_revoked_stays_listed_with_its_papers(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, admin) = site.staff_with_role("boss@example.test", "admin").await;
    let (_, cookie, claim, document) = sent(&site, "ana@example.test", "000123491").await;
    read_clearly(&site.db, document, "000123491").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));

    // The owner files, then removes the TIN: the row and its papers stay
    // listed for a person.
    let removed = site.post(&format!("/tins/claims/{claim}/remove"), &cookie, "").await;
    assert_eq!(location(&removed), "/tins?notice=tin_removed");
    assert_eq!(row(&site.db, claim).await.0, "removed");
    assert_eq!(papers(&site.db, claim).await, (1, 1));
    assert!(auto_tab(&site, &admin).await.contains(&format!("<tr data-id=\"{claim}\"")));

    // Adding it again never activates by itself.
    let (_, _, again, document) = sent(&site, "ana@example.test", "000123491").await;
    read_clearly(&site.db, document, "000123491").await;
    let left = doubts(consider(&site, document).await);
    assert!(left.contains(&Doubt::RefusedBefore), "{left:?}");
    assert_eq!(row(&site.db, again).await.0, "pending");

    // Revoked by staff: the same.
    let (_, _, claim, document) = sent(&site, "bob@example.test", "000123492").await;
    read_clearly(&site.db, document, "000123492").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
    let revoked = site.post(&format!("/staff/claims/{claim}/revoke"), &admin, "reason=Not+the+owner").await;
    assert_eq!(location(&revoked), format!("/staff/claims/{claim}?notice=ownership_revoked"));
    assert_eq!(papers(&site.db, claim).await, (1, 1));
    assert!(auto_tab(&site, &admin).await.contains(&format!("<tr data-id=\"{claim}\"")));
    // An Admin confirms it after looking: then the papers go.
    let response = site.post(&format!("/staff/claims/{claim}/confirm"), &admin, "").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=auto_confirmed"));
    assert_eq!(papers(&site.db, claim).await, (0, 0));
    assert_eq!(row(&site.db, claim).await.0, "revoked");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_activation_nobody_confirms_in_30_days_is_paused_with_its_papers(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, admin) = site.staff_with_role("boss@example.test", "admin").await;
    let (ana, _, claim, document) = sent(&site, "ana@example.test", "000123501").await;
    read_clearly(&site.db, document, "000123501").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
    // Not yet due.
    assert!(verify::upkeep(&site.db).await.unwrap().is_empty());
    sqlx::query("UPDATE tin_claims SET decided_at = now() - interval '31 days' WHERE id = $1")
        .bind(claim)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(verify::upkeep(&site.db).await.unwrap(), [claim]);
    let (paused, paused_by, reason): (bool, Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT paused_at IS NOT NULL, paused_by, pause_reason FROM tin_claims WHERE id = $1")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!((paused, paused_by, reason.as_deref()), (true, None, Some(verify::UNCONFIRMED_PAUSE)));
    let line = claim_audit_lines(&site.db, ana).await.pop().unwrap();
    assert_eq!(
        (line.0.as_str(), line.3.as_str(), line.4.as_deref()),
        ("paused", "auto", Some(verify::UNCONFIRMED_PAUSE))
    );
    assert_eq!(papers(&site.db, claim).await, (1, 1));
    let tin = Tin::parse("000123501", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::Paused);
    // Once.
    assert!(verify::upkeep(&site.db).await.unwrap().is_empty());
    let page = text(site.get(&format!("/staff/claims/{claim}"), &admin).await).await;
    assert!(page.contains("Paused by Buwiz on"), "{page}");

    // An Admin confirms, with a reason: the pause Buwiz made is lifted and
    // the papers go.
    let response = site.post(&format!("/staff/claims/{claim}/confirm"), &admin, "").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=reason_missing"));
    let response = site.post(&format!("/staff/claims/{claim}/confirm"), &admin, "reason=Papers+checked").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=auto_confirmed"));
    assert!(gate::own(&site.db, ana, &tin).await.unwrap().can_send());
    assert_eq!(papers(&site.db, claim).await, (0, 0));
    let mut actions: Vec<String> = claim_audit_lines(&site.db, ana).await.into_iter().map(|line| line.0).collect();
    // Confirming writes its two lines in one transaction, at one time.
    actions[2..].sort();
    assert_eq!(actions, ["auto_activated", "paused", "auto_confirmed", "resumed"]);

    // A staff pause is the staff's: confirming leaves it.
    let (bob, _, claim, document) = sent(&site, "bob@example.test", "000123502").await;
    read_clearly(&site.db, document, "000123502").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
    site.post(&format!("/staff/claims/{claim}/pause"), &admin, "reason=Complaint").await;
    site.post(&format!("/staff/claims/{claim}/confirm"), &admin, "").await;
    let tin = Tin::parse("000123502", "").unwrap();
    assert_eq!(gate::own(&site.db, bob, &tin).await.unwrap(), gate::Standing::Paused);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_an_automatic_activation_of_someone_else_can_be_confirmed(db: PgPool) {
    let site = Site::with_auto(db);
    let (helper, helper_cookie) = site.staff_with_role("helper@example.test", "support").await;
    // Not activated automatically.
    let other = add_claim(&site.db, helper, "000123511", "approved").await;
    let response = site.post(&format!("/staff/claims/{other}/confirm"), &helper_cookie, "").await;
    assert_eq!(location(&response), format!("/staff/claims/{other}?notice=not_auto"));
    // One's own.
    let session = helper_cookie.clone();
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123512")),
        ("registered_name", Text(NAME)),
        ("tin_id", File("card.png", PNG)),
    ];
    site.upload("/tins/claims", &session, &parts).await;
    let own = site.newest_claim(helper).await;
    settle(&site, own).await;
    let document = tin_id(&site.db, own).await;
    read_clearly(&site.db, document, "000123512").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(own));
    let response = site.post(&format!("/staff/claims/{own}/confirm"), &helper_cookie, "").await;
    assert_eq!(location(&response), format!("/staff/claims/{own}?notice=own_claim"));
    let page = text(site.get(&format!("/staff/claims/{own}"), &helper_cookie).await).await;
    assert!(!page.contains(&format!("action=\"/staff/claims/{own}/confirm\"")));
    // Not staff: no such page.
    let (_, plain) = site.person("plain@example.test").await;
    let response = site.post(&format!("/staff/claims/{own}/confirm"), &plain, "").await;
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Saves `root` without papers for `email`, as `kind`: (account, cookie,
/// claim).
async fn saved(site: &Site, email: &str, root: &'static str, kind: &'static str) -> (Uuid, String, Uuid) {
    let (account, cookie) = site.person(email).await;
    let parts = [("kind", Text(kind)), ("tin", Text(root)), ("registered_name", Text(NAME)), ("skip", Text("1"))];
    site.upload("/tins/claims", &cookie, &parts).await;
    let claim = site.newest_claim(account).await;
    assert_eq!(site.claim_status(claim).await, "unverified");
    (account, cookie, claim)
}

async fn delete_own(site: &Site, cookie: &str, claim: Uuid) {
    let response = site.post(&format!("/tins/claims/{claim}/delete"), cookie, "").await;
    assert!(location(&response).starts_with("/tins?notice="), "{}", location(&response));
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// What the rule looks back on survives a deleted claim: the audit lines
/// stay, and count as the rows did.
#[sqlx::test(migrations = "./migrations")]
async fn deleting_a_claim_does_not_erase_what_the_rule_looks_back_on(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, staff) = site.person(STAFF).await;

    // Rejected, deleted by its owner, sent again with a clean photo.
    let (_, cookie, claim, _) = sent(&site, "ana@example.test", "000123531").await;
    site.post(&format!("/staff/claims/{claim}/decision"), &staff, "decision=reject&note=Not+hers").await;
    assert_eq!(site.claim_status(claim).await, "rejected");
    delete_own(&site, &cookie, claim).await;
    let (_, _, _, document) = sent(&site, "ana@example.test", "000123531").await;
    read_clearly(&site.db, document, "000123531").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::RefusedBefore]);

    // Sent back, deleted, added again.
    let (_, cookie, claim, _) = sent(&site, "bob@example.test", "000123532").await;
    site.post(&format!("/staff/claims/{claim}/decision"), &staff, "decision=fix&reasons=blurry").await;
    assert_eq!(site.claim_status(claim).await, "needs_fix");
    delete_own(&site, &cookie, claim).await;
    let (_, _, _, document) = sent(&site, "bob@example.test", "000123532").await;
    read_clearly(&site.db, document, "000123532").await;
    // Sent back before, and deleted before: both are named.
    assert_eq!(doubts(consider(&site, document).await), [Doubt::SentBackBefore, Doubt::RefusedBefore]);

    // Another account saved it once (the real owner, say) and deleted it.
    let (_, cookie, claim) = saved(&site, "carl@example.test", "000123533", "personal").await;
    delete_own(&site, &cookie, claim).await;
    let (_, _, _, document) = sent(&site, "dan@example.test", "000123533").await;
    read_clearly(&site.db, document, "000123533").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::OtherAccount]);
}

/// A TIN ever paused, on any account, is never activated by itself: as
/// with Support, an Admin decides it. A resume and a removal since do not
/// change that.
#[sqlx::test(migrations = "./migrations")]
async fn a_tin_paused_before_is_left_for_an_admin(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, admin) = site.staff_with_role("boss@example.test", "admin").await;
    let (ana, cookie) = site.person("ana@example.test").await;
    let active = add_claim(&site.db, ana, "000123541", "approved").await;
    sqlx::query("UPDATE tin_claims SET kind = 'personal' WHERE id = $1").bind(active).execute(&site.db).await.unwrap();
    site.post(&format!("/staff/claims/{active}/pause"), &admin, "reason=Complaint").await;
    site.post(&format!("/staff/claims/{active}/resume"), &admin, "reason=Cleared").await;
    let removed = site.post(&format!("/tins/claims/{active}/remove"), &cookie, "").await;
    assert_eq!(location(&removed), "/tins?notice=tin_removed");
    let (_, _, _, document) = sent(&site, "ana@example.test", "000123541").await;
    read_clearly(&site.db, document, "000123541").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::PausedBefore]);
    assert!(Doubt::PausedBefore.words().contains("paused"));
}

/// "Personal" is the claimant's own word: a TIN that ever had a business
/// claim, on any account, deleted ones included, is left for staff.
#[sqlx::test(migrations = "./migrations")]
async fn a_tin_ever_claimed_as_a_business_is_left_for_staff(db: PgPool) {
    let site = Site::with_auto(db);
    // This account claimed it as a business once, then withdrew.
    let (hal, _) = site.person("hal@example.test").await;
    let withdrawn = add_claim(&site.db, hal, "000123551", "withdrawn").await;
    let _ = withdrawn;
    let (_, _, _, document) = sent(&site, "hal@example.test", "000123551").await;
    read_clearly(&site.db, document, "000123551").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::BusinessBefore]);

    // Another account saved it as a business, then deleted it.
    let (_, cookie, claim) = saved(&site, "fay@example.test", "000123552", "business").await;
    delete_own(&site, &cookie, claim).await;
    let (_, _, _, document) = sent(&site, "gil@example.test", "000123552").await;
    read_clearly(&site.db, document, "000123552").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::OtherAccount, Doubt::BusinessBefore]);
}

/// Another account holding the same 9-digit root at a branch is another
/// account on this TIN.
#[sqlx::test(migrations = "./migrations")]
async fn another_account_at_another_branch_counts(db: PgPool) {
    let site = Site::with_auto(db);
    let (ivy, _) = site.person("ivy@example.test").await;
    sqlx::query(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
         VALUES ($1, 'personal', '000123561', '00001', 'Ivy', 'unverified')",
    )
    .bind(ivy)
    .execute(&site.db)
    .await
    .unwrap();
    let (_, _, _, document) = sent(&site, "jo@example.test", "000123561").await;
    read_clearly(&site.db, document, "000123561").await;
    assert_eq!(doubts(consider(&site, document).await), [Doubt::OtherAccount]);
}

/// Confirming lifts Buwiz's own pause, and only an Admin reopens a paused
/// TIN: Support confirms only one that is not paused, and an Admin lifting
/// Buwiz's pause gives a reason.
#[sqlx::test(migrations = "./migrations")]
async fn only_an_admin_confirms_a_paused_activation(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, admin) = site.staff_with_role("boss@example.test", "admin").await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _, claim, document) = sent(&site, "ana@example.test", "000123571").await;
    read_clearly(&site.db, document, "000123571").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
    sqlx::query("UPDATE tin_claims SET decided_at = now() - interval '31 days' WHERE id = $1")
        .bind(claim)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(verify::upkeep(&site.db).await.unwrap(), [claim]);
    let page = format!("/staff/claims/{claim}");
    let confirm = format!("{page}/confirm");

    // Support: refused, the page says why and offers no button.
    let response = site.post(&confirm, &support, "reason=Looks+fine").await;
    assert_eq!(location(&response), format!("{page}?notice=admin_must_confirm"));
    let shown = text(site.get(&page, &support).await).await;
    assert!(!shown.contains(&format!("action=\"{confirm}\"")), "{shown}");
    assert!(shown.contains("an admin confirms it"), "{shown}");
    assert!(!row(&site.db, claim).await.4);
    let tin = Tin::parse("000123571", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::Paused);

    // An Admin: the reason is asked for, and kept on the resume line.
    let shown = text(site.get(&page, &admin).await).await;
    assert!(shown.contains(&format!("action=\"{confirm}\"")) && shown.contains("name=\"reason\""), "{shown}");
    let response = site.post(&confirm, &admin, "reason=+").await;
    assert_eq!(location(&response), format!("{page}?notice=reason_missing"));
    let response = site.post(&confirm, &admin, "reason=Papers+checked").await;
    assert_eq!(location(&response), format!("{page}?notice=auto_confirmed"));
    assert!(gate::own(&site.db, ana, &tin).await.unwrap().can_send());
    let resumed = claim_audit_lines(&site.db, ana).await.into_iter().find(|line| line.0 == "resumed").unwrap();
    assert_eq!((resumed.2.as_str(), resumed.3.as_str()), ("boss@example.test", "web"));
    assert!(resumed.4.as_deref().unwrap().contains("Papers checked"), "{resumed:?}");

    // A staff pause: Support cannot confirm either; an Admin can, without
    // a reason, and the pause stays.
    let (bob, _, claim, document) = sent(&site, "bob@example.test", "000123572").await;
    read_clearly(&site.db, document, "000123572").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
    site.post(&format!("/staff/claims/{claim}/pause"), &admin, "reason=Complaint").await;
    let page = format!("/staff/claims/{claim}");
    let response = site.post(&format!("{page}/confirm"), &support, "").await;
    assert_eq!(location(&response), format!("{page}?notice=admin_must_confirm"));
    let response = site.post(&format!("{page}/confirm"), &admin, "").await;
    assert_eq!(location(&response), format!("{page}?notice=auto_confirmed"));
    let tin = Tin::parse("000123572", "").unwrap();
    assert_eq!(gate::own(&site.db, bob, &tin).await.unwrap(), gate::Standing::Paused);
}

/// A test TIN's audit lines mask it as ***-***-000-…, as a real TIN ending
/// in 000 is masked: they are no other account on that TIN, and no
/// business before it.
#[sqlx::test(migrations = "./migrations")]
async fn a_test_tins_lines_cast_no_doubt_on_a_real_tin_ending_in_000(db: PgPool) {
    let site = Site::with_auto(db);
    let (_, staff) = site.person(STAFF).await;
    let (tester, _) = site.person("tester@example.test").await;
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(tester).execute(&site.db).await.unwrap();
    let form = "tin=000-000-000-00000&kind=business&registered_name=Test+Shop&reason=Trying+the+app";
    let response = site.post(&format!("/staff/users/{tester}/test-tin"), &staff, form).await;
    assert_eq!(location(&response), format!("/staff/users/{tester}?notice=test_tin_added"));
    let test_claim: Uuid = sqlx::query_scalar("SELECT id FROM tin_claims WHERE account_id = $1")
        .bind(tester)
        .fetch_one(&site.db)
        .await
        .unwrap();
    site.post(&format!("/staff/claims/{test_claim}/revoke"), &staff, "reason=Done").await;
    let masked: Vec<String> = sqlx::query_scalar("SELECT tin_masked FROM claim_audit WHERE account_id = $1")
        .bind(tester)
        .fetch_all(&site.db)
        .await
        .unwrap();
    assert_eq!(masked, ["***-***-000-00000", "***-***-000-00000"]);

    let (_, _, claim, document) = sent(&site, "ana@example.test", "123456000").await;
    read_clearly(&site.db, document, "123456000").await;
    assert_eq!(consider(&site, document).await, Outcome::Activated(claim));
}
