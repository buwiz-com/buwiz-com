//! Staff on their own account: they read their own page in full; an Admin
//! acts on their own TINs with the usual reason, and each line says it was
//! their own; Support leaves their own claims to someone else; nobody bans
//! themselves, changes their own role or labels their own account.

mod common;

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

const SELF_ACTIVATED: &str = "Activated by its owner, an admin";

async fn papers(site: &Site, cookie: &str, tin: &'static str) -> Uuid {
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text(tin)),
        ("registered_name", Text("Juan Dela Cruz")),
        ("tin_id", File("tin.png", PNG)),
    ];
    assert_eq!(location(&site.upload("/tins/claims", cookie, &parts).await), "/tins?notice=claim_submitted");
    sqlx::query_scalar("SELECT id FROM tin_claims WHERE tin_root = $1").bind(tin).fetch_one(&site.db).await.unwrap()
}

/// The actor and the account of every claim line of `account`, with the
/// action: an Admin's own lines name the account itself as the actor.
async fn actors(db: &PgPool, account: Uuid) -> Vec<(String, Uuid)> {
    sqlx::query_as("SELECT action, actor_id FROM claim_audit WHERE account_id = $1 ORDER BY created_at, id")
        .bind(account)
        .fetch_all(db)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_opens_their_own_page_and_acts_on_their_own_tins(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (boss, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    site.staff_with_role("other@example.test", "admin").await;

    let response = site.get(&format!("/staff/users/{boss}"), &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("This is your own account: another admin changes its role, ban or test label."), "{page}");
    for withheld in ["role", "ban", "test"] {
        assert!(!page.contains(&format!("action=\"/staff/users/{boss}/{withheld}\"")), "{withheld}: {page}");
    }
    // Signing oneself out everywhere stays.
    assert!(page.contains(&format!("action=\"/staff/users/{boss}/sign-out\"")), "{page}");

    // Pause, resume and revoke their own active TIN, each with a reason.
    let active = add_claim(&site.db, boss, "000123451", "approved").await;
    let claim = format!("/staff/claims/{active}");
    let page = text(site.get(&claim, &cookie).await).await;
    assert!(page.contains(&format!("action=\"{claim}/pause\"")), "{page}");
    assert!(page.contains(&format!("action=\"{claim}/revoke\"")), "{page}");
    assert_eq!(
        location(&site.post(&format!("{claim}/pause"), &cookie, "reason=+").await),
        format!("{claim}?notice=reason_missing")
    );
    assert_eq!(
        location(&site.post(&format!("{claim}/pause"), &cookie, "reason=Check").await),
        format!("{claim}?notice=tin_paused")
    );
    assert_eq!(
        location(&site.post(&format!("{claim}/resume"), &cookie, "reason=Fine").await),
        format!("{claim}?notice=tin_resumed")
    );
    assert_eq!(
        location(&site.post(&format!("{claim}/revoke"), &cookie, "reason=Not+mine").await),
        format!("{claim}?notice=ownership_revoked")
    );

    // Reject and send back their own papers, with the usual note.
    let rejected = papers(&site, &cookie, "000123452").await;
    let path = format!("/staff/claims/{rejected}/decision");
    assert_eq!(
        location(&site.post(&path, &cookie, "decision=reject").await),
        format!("/staff/claims/{rejected}?notice=reject_needs_note")
    );
    assert_eq!(
        location(&site.post(&path, &cookie, "decision=reject&note=Typo").await),
        "/staff/queue?notice=claim_decided"
    );
    let fix = papers(&site, &cookie, "000123453").await;
    let path = format!("/staff/claims/{fix}/decision");
    assert_eq!(
        location(&site.post(&path, &cookie, "decision=fix&reasons=blurry").await),
        "/staff/queue?notice=claim_sent_back"
    );

    // Deleting their own claim is the owner's, on My TINs.
    let unverified = add_claim(&site.db, boss, "000123454", "unverified").await;
    let path = format!("/staff/claims/{unverified}/delete");
    assert_eq!(
        location(&site.post(&path, &cookie, "reason=Mine").await),
        format!("/staff/claims/{unverified}?notice=own_claim_delete")
    );
    let page = text(site.get(&format!("/staff/claims/{unverified}"), &cookie).await).await;
    assert!(page.contains("This is your own claim: delete it from My TINs."), "{page}");
    assert_eq!(site.claim_status(unverified).await, "unverified");

    // Every line names boss as the actor on boss's own account, and the
    // history says each was their own.
    let lines = actors(&site.db, boss).await;
    let actions: Vec<&str> = lines.iter().map(|(action, _)| action.as_str()).collect();
    assert_eq!(actions, ["paused", "resumed", "revoked", "rejected", "sent_back"]);
    assert!(lines.iter().all(|(_, actor)| *actor == boss), "{lines:?}");
    let history = text(site.get(&format!("/staff/users/{boss}"), &cookie).await).await;
    for line in [
        "Staff boss@example.test paused their own TIN ***-***-451-00000: Check",
        "Staff boss@example.test resumed their own TIN ***-***-451-00000: Fine",
        "Staff boss@example.test revoked their own ownership of ***-***-451-00000: Not mine",
        "Staff boss@example.test rejected their own claim on ***-***-452-00000: Typo",
        "Staff boss@example.test sent their own claim on ***-***-453-00000 back for a fix",
    ] {
        assert!(history.contains(line), "{line}: {history}");
    }
}

/// Nobody bans themselves or changes their own role, nor labels their own
/// account, even with a direct post; nothing changes.
#[sqlx::test(migrations = "./migrations")]
async fn nobody_bans_themselves_or_changes_their_own_role(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (boss, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    site.staff_with_role("other@example.test", "admin").await;
    let user = format!("/staff/users/{boss}");
    for (action, form) in
        [("ban", "reason=Mine"), ("role", "role=support&reason=Mine"), ("test", "test=on&reason=Mine")]
    {
        let response = site.post(&format!("{user}/{action}"), &cookie, form).await;
        assert_eq!(location(&response), format!("{user}?notice=own_account"), "{action}");
    }
    let (role, banned, test): (Option<String>, bool, bool) =
        sqlx::query_as("SELECT role, banned_at IS NOT NULL, test_account FROM accounts WHERE id = $1")
            .bind(boss)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!((role.as_deref(), banned, test), (Some("admin"), false, false));
    assert!(account_audit_lines(&site.db, boss).await.is_empty());
}

/// Support reads their own page and claim, but does not decide or confirm
/// their own claim.
#[sqlx::test(migrations = "./migrations")]
async fn support_reads_but_does_not_decide_their_own_claim(db: PgPool) {
    let site = Site::new(db);
    let (helper, cookie) = site.staff_with_role("helper@example.test", "support").await;
    let open = papers(&site, &cookie, "000123455").await;

    let response = site.get(&format!("/staff/users/{helper}"), &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(site.get(&format!("/staff/claims/{open}"), &cookie).await).await;
    assert!(page.contains("This is your own claim. Another staff member decides it."), "{page}");
    assert!(!page.contains(&format!("action=\"/staff/claims/{open}/decision\"")), "{page}");
    for form in ["decision=approve", "decision=reject&note=No", "decision=fix&reasons=blurry"] {
        let response = site.post(&format!("/staff/claims/{open}/decision"), &cookie, form).await;
        assert_eq!(location(&response), format!("/staff/claims/{open}?notice=own_claim"), "{form}");
    }
    assert_eq!(site.claim_status(open).await, "pending");
    assert!(claim_audit_lines(&site.db, helper).await.is_empty());
}

/// An automatic activation of their own TIN: an Admin confirms it, with a
/// written reason on its line, and the line names them; Support may not
/// (tests/auto_verify.rs). Like an Admin's own activation without papers,
/// the queue and the claim flag it.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_confirms_the_automatic_activation_of_their_own_tin(db: PgPool) {
    let site = Site::new(db);
    let (boss, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let claim: Uuid = sqlx::query_scalar(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ($1, 'personal', '000123456', '00000', 'Juan Dela Cruz', 'approved', now(), 'auto') RETURNING id",
    )
    .bind(boss)
    .fetch_one(&site.db)
    .await
    .unwrap();
    let path = format!("/staff/claims/{claim}");
    let page = text(site.get(&path, &cookie).await).await;
    assert!(page.contains(&format!("action=\"{path}/confirm\"")), "{page}");
    assert!(page.contains("This is your own TIN: write why; it goes in your history."), "{page}");
    assert!(!page.contains(SELF_ACTIVATED), "{page}");
    for form in ["", "reason=+"] {
        let response = site.post(&format!("{path}/confirm"), &cookie, form).await;
        assert_eq!(location(&response), format!("{path}?notice=own_needs_reason"), "{form}");
    }
    assert!(actors(&site.db, boss).await.is_empty());
    let response = site.post(&format!("{path}/confirm"), &cookie, "reason=My+own+ID").await;
    assert_eq!(location(&response), format!("{path}?notice=auto_confirmed"));
    assert_eq!(actors(&site.db, boss).await, [("auto_confirmed".to_owned(), boss)]);
    assert_eq!(claim_audit_lines(&site.db, boss).await[0].4.as_deref(), Some("My own ID"));
    let history = text(site.get(&format!("/staff/users/{boss}"), &cookie).await).await;
    assert!(
        history.contains(
            "Staff boss@example.test confirmed the automatic activation of their own TIN ***-***-456-00000: My own ID"
        ),
        "{history}"
    );
    let page = text(site.get(&path, &cookie).await).await;
    assert!(page.contains(SELF_ACTIVATED), "{page}");
    let queue = text(site.get("/staff/queue?status=approved", &cookie).await).await;
    assert!(queue.contains(SELF_ACTIVATED), "{queue}");
}

/// An Admin approving their own claim writes why, as one activating it
/// without papers does, and the queue and the claim flag it. Approving
/// someone else's claim needs no note, as before.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_approving_their_own_claim_writes_why_and_it_is_flagged(db: PgPool) {
    let site = Site::new(db);
    let (boss, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let own = papers(&site, &cookie, "000123457").await;
    let path = format!("/staff/claims/{own}");
    let page = text(site.get(&path, &cookie).await).await;
    assert!(page.contains("to approve it, write why in the note."), "{page}");
    for form in ["decision=approve", "decision=approve&note=+"] {
        let response = site.post(&format!("{path}/decision"), &cookie, form).await;
        assert_eq!(location(&response), format!("{path}?notice=own_needs_reason"), "{form}");
    }
    assert_eq!(site.claim_status(own).await, "pending");
    let page = text(site.get(&format!("{path}?notice=own_needs_reason"), &cookie).await).await;
    assert!(page.contains("This is your own TIN: write why you make it active."), "{page}");
    let response = site.post(&format!("{path}/decision"), &cookie, "decision=approve&note=Checked+my+own+ID").await;
    assert_eq!(location(&response), "/staff/queue?notice=claim_decided");
    assert_eq!(site.claim_status(own).await, "approved");
    assert_eq!(
        claim_audit_lines(&site.db, boss).await,
        [(
            "approved".into(),
            "pending".into(),
            "boss@example.test".into(),
            "web".into(),
            Some("Checked my own ID".into())
        )]
    );
    let page = text(site.get(&path, &cookie).await).await;
    assert!(page.contains(SELF_ACTIVATED), "{page}");
    let queue = text(site.get("/staff/queue?status=approved", &cookie).await).await;
    assert!(queue.contains(SELF_ACTIVATED), "{queue}");
    let history = text(site.get(&format!("/staff/users/{boss}"), &cookie).await).await;
    assert!(
        history.contains("Staff boss@example.test approved their own claim on ***-***-457-00000: Checked my own ID"),
        "{history}"
    );

    // Someone else's claim: approved without a note, and no flag.
    let (_, ana_cookie) = site.person("ana@example.test").await;
    let theirs = papers(&site, &ana_cookie, "000123458").await;
    let response = site.post(&format!("/staff/claims/{theirs}/decision"), &cookie, "decision=approve").await;
    assert_eq!(location(&response), "/staff/queue?notice=claim_decided");
    let page = text(site.get(&format!("/staff/claims/{theirs}"), &cookie).await).await;
    assert!(!page.contains(SELF_ACTIVATED), "{page}");
}
