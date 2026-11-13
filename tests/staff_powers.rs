//! Staff roles and what each may do: Support and Admin kept on the account,
//! `ADMIN_EMAILS` as a break-glass list of Admins, and the account-level
//! powers (roles, signing out) with their history.

mod common;

use axum::http::StatusCode;
use buwiz_cloud::account::admin;
use buwiz_cloud::account::notice::Notice;
use buwiz_cloud::account::roles::{self, Role};
use buwiz_cloud::claims::DecideError;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

/// Who asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    SignedOut,
    User,
    Support,
    /// An account with the role `admin`, not in `ADMIN_EMAILS`.
    Admin,
    /// In `ADMIN_EMAILS` ([`STAFF`]), with no role on the account.
    ServerAdmin,
}

impl Who {
    /// May use a power that is Support's (`false`) or Admin's only (`true`).
    fn may(self, admin_only: bool) -> bool {
        match self {
            Self::Support => !admin_only,
            Self::Admin | Self::ServerAdmin => true,
            Self::SignedOut | Self::User => false,
        }
    }
}

/// What each actor acts on, made fresh for each so nothing one did is in
/// another's way.
struct Target {
    account: Uuid,
    /// Waiting for review, with its papers.
    open: Uuid,
    /// Saved without papers (deletable).
    unverified: Uuid,
    /// Saved without papers too, to activate.
    spare: Uuid,
    /// Active.
    active: Uuid,
    document: Uuid,
    grant: Uuid,
}

async fn target(site: &Site, n: u32) -> Target {
    let (account, cookie) = site.person(&format!("target{n}@example.test")).await;
    let tin = format!("1{n:08}");
    let tin: &'static str = Box::leak(tin.into_boxed_str());
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text(tin)),
        ("registered_name", Text("Juan Dela Cruz")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &cookie, &parts).await;
    let open = site.newest_claim(account).await;
    let document = sqlx::query_scalar("SELECT id FROM claim_documents WHERE claim_id = $1")
        .bind(open)
        .fetch_one(&site.db)
        .await
        .unwrap();
    let unverified = add_claim(&site.db, account, &format!("2{n:08}"), "unverified").await;
    let active = add_claim(&site.db, account, &format!("3{n:08}"), "approved").await;
    let spare = add_claim(&site.db, account, &format!("4{n:08}"), "unverified").await;
    app_token(&site.db, account).await;
    let grant = sqlx::query_scalar("SELECT id FROM oauth_grants WHERE account_id = $1")
        .bind(account)
        .fetch_one(&site.db)
        .await
        .unwrap();
    Target { account, open, unverified, spare, active, document, grant }
}

/// The actors of the matrix, each signed in (but the first).
async fn actors(site: &Site) -> Vec<(Who, String)> {
    vec![
        (Who::SignedOut, String::new()),
        (Who::User, site.person("plain@example.test").await.1),
        (Who::Support, site.staff_with_role("helper@example.test", "support").await.1),
        (Who::Admin, site.staff_with_role("boss@example.test", "admin").await.1),
        (Who::ServerAdmin, site.person(STAFF).await.1),
    ]
}

/// Posts `form` to `path` as `who` and checks the answer: a sign-in for
/// the signed out, a 404 for everyone not staff, `done` when allowed, and
/// back to `back_to` with `admin_only` for staff without the power.
async fn expect_post(
    site: &Site,
    (who, cookie): (Who, &str),
    path: &str,
    form: &str,
    admin_only: bool,
    back_to: &str,
    done: &str,
) {
    let response = site.post(path, cookie, form).await;
    match who {
        Who::SignedOut => assert!(location(&response).starts_with("/login?"), "{who:?} {path}"),
        Who::User => assert_eq!(response.status(), StatusCode::NOT_FOUND, "{who:?} {path}"),
        _ if who.may(admin_only) => assert_eq!(location(&response), done, "{who:?} {path}"),
        _ => assert_eq!(location(&response), format!("{back_to}?notice=admin_only"), "{who:?} {path}"),
    }
}

/// A staff member as a page saw them: the check a request passes before it
/// changes anything.
async fn as_staff(site: &Site, id: Uuid) -> roles::Staff {
    let email: String =
        sqlx::query_scalar("SELECT email FROM accounts WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    let account = buwiz_cloud::accounts::Account { id, email };
    roles::staff(&site.db, &site.state.config, &account).await.unwrap().unwrap()
}

async fn grant_revoked(db: &PgPool, grant: Uuid) -> bool {
    sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM oauth_grants WHERE id = $1")
        .bind(grant)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn stored_role(db: &PgPool, account: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT role FROM accounts WHERE id = $1").bind(account).fetch_one(db).await.unwrap()
}

async fn claim_exists(db: &PgPool, claim: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tin_claims WHERE id = $1)")
        .bind(claim)
        .fetch_one(db)
        .await
        .unwrap()
}

/// Every staff page and action, for a signed-out visitor, a plain user,
/// Support, an Admin by role and an Admin through `ADMIN_EMAILS`. The
/// Admin by role is not in `ADMIN_EMAILS`, so any route still deciding by
/// that list alone fails here.
#[sqlx::test(migrations = "./migrations")]
async fn roles_on_the_account_decide_staff_powers(db: PgPool) {
    let site = Site::new(db);
    for (n, (who, cookie)) in actors(&site).await.into_iter().enumerate() {
        let t = target(&site, n as u32).await;
        let user = format!("/staff/users/{}", t.account);

        for page in [
            "/staff".to_owned(),
            "/staff?status=unverified".to_owned(),
            "/staff/users".to_owned(),
            user.clone(),
            format!("/staff/claims/{}", t.open),
            format!("/staff/documents/{}", t.document),
        ] {
            let response = site.get(&page, &cookie).await;
            match who {
                Who::SignedOut => assert!(location(&response).starts_with("/login?"), "{who:?} {page}"),
                Who::User => assert_eq!(response.status(), StatusCode::NOT_FOUND, "{who:?} {page}"),
                _ => assert_eq!(response.status(), StatusCode::OK, "{who:?} {page}"),
            }
        }

        // Review: Support and Admins.
        let claim_page = format!("/staff/claims/{}", t.open);
        let path = format!("{claim_page}/decision");
        expect_post(
            &site,
            (who, &cookie),
            &path,
            "decision=approve",
            false,
            &claim_page,
            "/staff?notice=claim_decided",
        )
        .await;
        assert_eq!(site.claim_status(t.open).await == "approved", who.may(false), "{who:?}");

        // Delete a claim: Admins only.
        let claim_page = format!("/staff/claims/{}", t.unverified);
        let path = format!("{claim_page}/delete");
        expect_post(&site, (who, &cookie), &path, "reason=Duplicate", true, &claim_page, "/staff?notice=claim_deleted")
            .await;
        assert_eq!(!claim_exists(&site.db, t.unverified).await, who.may(true), "{who:?}");

        // End an ownership: Admins only.
        let claim_page = format!("/staff/claims/{}", t.active);
        let path = format!("{claim_page}/revoke");
        let done = format!("{claim_page}?notice=ownership_revoked");
        expect_post(&site, (who, &cookie), &path, "reason=Wrong+owner", true, &claim_page, &done).await;
        assert_eq!(site.claim_status(t.active).await == "revoked", who.may(true), "{who:?}");

        // Activate without papers: Admins only.
        let claim_page = format!("/staff/claims/{}", t.spare);
        let path = format!("{claim_page}/activate");
        let done = format!("{claim_page}?notice=tin_activated");
        expect_post(&site, (who, &cookie), &path, "reason=Known+owner", true, &claim_page, &done).await;
        assert_eq!(site.claim_status(t.spare).await == "approved", who.may(true), "{who:?}");

        // Change a role: Admins only.
        let path = format!("{user}/role");
        let done = format!("{user}?notice=role_changed");
        expect_post(&site, (who, &cookie), &path, "role=support&reason=Joins+the+team", true, &user, &done).await;
        assert_eq!(stored_role(&site.db, t.account).await.is_some(), who.may(true), "{who:?}");

        // One app: Support and Admins.
        let path = format!("{user}/apps/{}/sign-out", t.grant);
        let done = format!("{user}?notice=app_signed_out");
        expect_post(&site, (who, &cookie), &path, "", false, &user, &done).await;
        assert_eq!(grant_revoked(&site.db, t.grant).await, who.may(false), "{who:?}");

        // Everywhere: Support and Admins.
        let path = format!("{user}/sign-out");
        let done = format!("{user}?notice=signed_out_everywhere");
        expect_post(&site, (who, &cookie), &path, "reason=Lost+laptop", false, &user, &done).await;
        assert_eq!(live_sessions(&site.db, t.account).await == 0, who.may(false), "{who:?}");
    }
}

/// `Config::is_admin` (the email list) is read in one place only.
#[test]
fn only_roles_reads_the_email_list() {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                if source.contains(".is_admin(") {
                    found.push(path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display().to_string());
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    assert_eq!(found, ["src/account/roles.rs"]);
}

/// The break-glass list works with no role stored anywhere, wins over a
/// stored role, and the staff pages cannot change it.
#[sqlx::test(migrations = "./migrations")]
async fn the_server_setting_is_an_admin_with_no_role_and_cannot_be_demoted(db: PgPool) {
    let site = Site::new(db);
    let (server_admin, server_cookie) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let roles: i64 =
        sqlx::query_scalar("SELECT count(*) FROM accounts WHERE role IS NOT NULL").fetch_one(&site.db).await.unwrap();
    assert_eq!(roles, 0);
    assert_eq!(site.get("/staff", &server_cookie).await.status(), StatusCode::OK);
    let response =
        site.post(&format!("/staff/users/{ana}/role"), &server_cookie, "role=admin&reason=Second+admin").await;
    assert_eq!(location(&response), format!("/staff/users/{ana}?notice=role_changed"));
    // ana is an Admin by role now: she cannot demote the server's admin.
    let page = text(site.get(&format!("/staff/users/{server_admin}"), &ana_cookie).await).await;
    assert!(page.contains("Admin through the server setting ADMIN_EMAILS. Change it there."), "{page}");
    assert!(page.contains("Admin · server setting"), "{page}");
    let path = format!("/staff/users/{server_admin}/role");
    for form in ["role=none&reason=Demote", "role=support&reason=Demote"] {
        let response = site.post(&path, &ana_cookie, form).await;
        assert_eq!(location(&response), format!("/staff/users/{server_admin}?notice=role_from_server"));
    }
    assert_eq!(stored_role(&site.db, server_admin).await, None);
    // A role stored by other means does not take the setting's away.
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1")
        .bind(server_admin)
        .execute(&site.db)
        .await
        .unwrap();
    let response = site.post(&format!("/staff/users/{ana}/role"), &server_cookie, "role=support&reason=Back").await;
    assert_eq!(location(&response), format!("/staff/users/{ana}?notice=role_changed"));
    let lines = account_audit_lines(&site.db, ana).await;
    assert_eq!(
        lines
            .iter()
            .map(|line| (line.0.as_str(), line.1.as_deref(), line.2.as_deref(), line.5.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("role_changed", None, Some("admin"), Some("Second admin")),
            ("role_changed", Some("admin"), Some("support"), Some("Back")),
        ]
    );
    assert!(lines.iter().all(|line| line.3 == STAFF && line.4 == "web"));
    let history = text(site.get(&format!("/staff/users/{ana}"), &server_cookie).await).await;
    assert!(history.contains(&format!("Staff {STAFF} changed the role from Admin to Support: Back")), "{history}");
}

/// The staff pages refuse what would leave nobody in charge, and say why.
#[sqlx::test(migrations = "./migrations")]
async fn roles_cannot_lock_everyone_out(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (ana, ana_cookie) = site.staff_with_role("ana@example.test", "admin").await;
    let (bob, _) = site.staff_with_role("bob@example.test", "admin").await;
    let (carl, _) = site.person("carl@example.test").await;
    let path = |id: Uuid| format!("/staff/users/{id}/role");
    let back = |id: Uuid, notice: &str| format!("/staff/users/{id}?notice={notice}");

    assert_eq!(location(&site.post(&path(ana), &ana_cookie, "role=none&reason=x").await), back(ana, "own_account"));
    assert_eq!(location(&site.post(&path(carl), &ana_cookie, "role=boss&reason=x").await), back(carl, "role_unknown"));
    assert_eq!(
        location(&site.post(&path(carl), &ana_cookie, "role=support&reason=+").await),
        back(carl, "reason_missing")
    );
    let page = text(site.get(&format!("/staff/users/{ana}"), &ana_cookie).await).await;
    assert!(page.contains("This is you. Another admin can change your role."), "{page}");
    assert_eq!(stored_role(&site.db, carl).await, None);
    assert!(account_audit_lines(&site.db, carl).await.is_empty());

    // Two Admins, each demoting the other, both having passed the page's
    // check: the second must not leave Buwiz without an Admin.
    let config = &site.state.config;
    let ana_staff = as_staff(&site, ana).await;
    let bob_staff = as_staff(&site, bob).await;
    let first = admin::change_role(&site.db, config, &ana_staff, bob, None, "Leaves").await.unwrap();
    assert_eq!(first, Notice::RoleChanged);
    let second = admin::change_role(&site.db, config, &bob_staff, ana, None, "Leaves too").await;
    assert!(matches!(second, Err(DecideError::Refused(Notice::LastAdmin))), "{second:?}");
    assert_eq!(stored_role(&site.db, ana).await.as_deref(), Some("admin"));
    assert_eq!(roles::other_admins(&site.db, &[], None).await.unwrap(), 1);
}

/// The same race, run at once many times: exactly one demotion wins.
#[sqlx::test(migrations = "./migrations")]
async fn two_admins_demoting_each_other_at_once_leave_one(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let config = &site.state.config;
    for round in 0..10 {
        let (ana, _) = site.staff_with_role(&format!("ana{round}@example.test"), "admin").await;
        let (bob, _) = site.staff_with_role(&format!("bob{round}@example.test"), "admin").await;
        // Only these two are Admins this round.
        sqlx::query("UPDATE accounts SET role = NULL WHERE role = 'admin' AND id <> ALL($1)")
            .bind([ana, bob])
            .execute(&site.db)
            .await
            .unwrap();
        let ana_staff = as_staff(&site, ana).await;
        let bob_staff = as_staff(&site, bob).await;
        assert_eq!((ana_staff.role, bob_staff.role), (Role::Admin, Role::Admin));
        let (one, two) = tokio::join!(
            admin::change_role(&site.db, config, &ana_staff, bob, None, "Leaves"),
            admin::change_role(&site.db, config, &bob_staff, ana, None, "Leaves"),
        );
        let won = [&one, &two].iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(won, 1, "round {round}: {one:?} {two:?}");
        for lost in [&one, &two].into_iter().filter_map(|outcome| outcome.as_ref().err()) {
            assert!(matches!(lost, DecideError::Refused(Notice::LastAdmin | Notice::AdminOnly)), "{lost:?}");
        }
        assert_eq!(roles::other_admins(&site.db, &[], None).await.unwrap(), 1, "round {round}");
    }
}

/// Support signs people out, but never an Admin; Admins sign anyone out.
#[sqlx::test(migrations = "./migrations")]
async fn support_cannot_sign_an_admin_out(db: PgPool) {
    let site = Site::new(db);
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (boss, boss_cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let (server_admin, _) = site.person(STAFF).await;
    app_token(&site.db, boss).await;
    let grant: Uuid = sqlx::query_scalar("SELECT id FROM oauth_grants WHERE account_id = $1")
        .bind(boss)
        .fetch_one(&site.db)
        .await
        .unwrap();
    for admin in [boss, server_admin] {
        let user = format!("/staff/users/{admin}");
        let response = site.post(&format!("{user}/sign-out"), &support, "reason=Tidy").await;
        assert_eq!(location(&response), format!("{user}?notice=admin_only"));
        let page = text(site.get(&user, &support).await).await;
        assert!(!page.contains(&format!("action=\"{user}/sign-out\"")), "{page}");
    }
    let response = site.post(&format!("/staff/users/{boss}/apps/{grant}/sign-out"), &support, "").await;
    assert_eq!(location(&response), format!("/staff/users/{boss}?notice=admin_only"));
    assert!(!grant_revoked(&site.db, grant).await);
    assert_eq!(live_sessions(&site.db, boss).await, 1);
    assert!(account_audit_lines(&site.db, boss).await.is_empty());

    let response =
        site.post(&format!("/staff/users/{server_admin}/sign-out"), &boss_cookie, "reason=Shared+laptop").await;
    assert_eq!(location(&response), format!("/staff/users/{server_admin}?notice=signed_out_everywhere"));
    let lines = account_audit_lines(&site.db, server_admin).await;
    assert_eq!(lines.len(), 1);
    assert_eq!(
        (lines[0].0.as_str(), lines[0].3.as_str(), lines[0].5.as_deref()),
        ("signed_out", "boss@example.test", Some("Shared laptop"))
    );
}

/// TINs saved without papers have their own tab, count as a TIN in the
/// users list without proving anyone's identity, and read plainly in the
/// history.
#[sqlx::test(migrations = "./migrations")]
async fn staff_pages_show_tins_saved_without_papers(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let (_, _) = site.person("nobody@example.test").await;
    let claim = add_claim(&site.db, ana, "000123455", "unverified").await;

    let queue = text(site.get("/staff?status=unverified", &staff).await).await;
    assert!(queue.contains("Not verified") && queue.contains("000-123-455-00000"), "{queue}");
    assert!(!text(site.get("/staff", &staff).await).await.contains("000-123-455-00000"));

    let no_tin = text(site.get("/staff/users?filter=no_tin", &staff).await).await;
    assert!(no_tin.contains("nobody@example.test") && !no_tin.contains("ana@example.test"), "{no_tin}");
    let users = text(site.get("/staff/users", &staff).await).await;
    assert!(users.contains("000-123-455-00000") && users.contains("dot unverified"), "{users}");
    let row = &users[users.find("ana@example.test").unwrap()..];
    assert!(row[..row.find("</a>").unwrap()].contains("No ID yet"), "{row}");

    let response = site.post(&format!("/staff/claims/{claim}/delete"), &staff, "reason=Typo").await;
    assert_eq!(location(&response), "/staff?notice=claim_deleted");
    let history = text(site.get(&format!("/staff/users/{ana}"), &staff).await).await;
    assert!(
        history.contains(&format!("Staff {STAFF} deleted ***-***-455-00000, saved without papers: Typo")),
        "{history}"
    );
    assert!(!history.contains("a unverified"), "{history}");
}
