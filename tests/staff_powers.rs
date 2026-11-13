//! Staff roles and what each may do: Support and Admin kept on the account,
//! `ADMIN_EMAILS` as a break-glass list of Admins, and the account-level
//! powers (roles, signing out) with their history.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
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
    /// Waiting for review, on a TIN another account had paused.
    paused_before: Uuid,
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
    // Paused once on someone else's account, which then lost it.
    let (earlier, _) = site.person(&format!("earlier{n}@example.test")).await;
    let ended = add_claim(&site.db, earlier, &format!("7{n:08}"), "revoked").await;
    sqlx::query(
        "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
         VALUES ($1, $2, '***-***-000-00000', 'paused', 'approved', $2, 'staff@example.test', 'Complaint', 'web')",
    )
    .bind(ended)
    .bind(earlier)
    .execute(&site.db)
    .await
    .unwrap();
    let paused_before = add_claim(&site.db, account, &format!("7{n:08}"), "pending").await;
    app_token(&site.db, account).await;
    let grant = sqlx::query_scalar("SELECT id FROM oauth_grants WHERE account_id = $1")
        .bind(account)
        .fetch_one(&site.db)
        .await
        .unwrap();
    Target { account, open, unverified, spare, active, paused_before, document, grant }
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
    site: &Tracked,
    (who, cookie): (Who, &str),
    path: &str,
    form: &str,
    admin_only: bool,
    back_to: &str,
    done: &str,
) {
    site.hit(path);
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

/// Every staff route and page, as the router and the pages name them; the
/// matrix below exercises each.
const MATRIX: &[&str] = &[
    "/staff",
    "/staff/claims",
    "/staff/claims/{claim_id}",
    "/staff/claims/{claim_id}/decision",
    "/staff/claims/{claim_id}/delete",
    "/staff/claims/{claim_id}/revoke",
    "/staff/claims/{claim_id}/activate",
    "/staff/claims/{claim_id}/pause",
    "/staff/claims/{claim_id}/resume",
    "/staff/claims/{claim_id}/confirm",
    "/staff/documents/{document_id}",
    "/staff/users",
    "/staff/users/{account_id}",
    "/staff/users/{account_id}/sign-out",
    "/staff/users/{account_id}/apps/{grant_id}/sign-out",
    "/staff/users/{account_id}/role",
    "/staff/users/{account_id}/ban",
    "/staff/users/{account_id}/unban",
    "/staff/users/{account_id}/test",
    "/staff/users/new",
];

/// `path` with its ids put back as the route's placeholders.
fn route_of(path: &str) -> String {
    let path = path.split('?').next().unwrap();
    let names = ["claim_id", "document_id", "account_id", "grant_id"];
    let mut parts = Vec::new();
    for (index, part) in path.split('/').enumerate() {
        if Uuid::parse_str(part).is_ok() {
            let before = path.split('/').nth(index - 1).unwrap();
            let name = match before {
                "claims" => names[0],
                "documents" => names[1],
                "users" => names[2],
                _ => names[3],
            };
            parts.push(format!("{{{name}}}"));
        } else {
            parts.push(part.to_owned());
        }
    }
    parts.join("/")
}

/// Every staff page and action, for a signed-out visitor, a plain user,
/// Support, an Admin by role and an Admin through `ADMIN_EMAILS`. The
/// Admin by role is not in `ADMIN_EMAILS`, so any route still deciding by
/// that list alone fails here.
#[sqlx::test(migrations = "./migrations")]
async fn roles_on_the_account_decide_staff_powers(db: PgPool) {
    let site = Site::new(db);
    let site = &Tracked { site, hits: Default::default() };
    for (n, (who, cookie)) in actors(site).await.into_iter().enumerate() {
        // The old address of the queue moves to /staff, for everyone.
        site.hit("/staff/claims");
        let moved = site.get("/staff/claims", &cookie).await;
        assert_eq!((moved.status(), location(&moved)), (StatusCode::PERMANENT_REDIRECT, "/staff".to_owned()));
        let t = target(site, n as u32).await;
        let user = format!("/staff/users/{}", t.account);

        for page in [
            "/staff".to_owned(),
            "/staff?status=unverified".to_owned(),
            "/staff/users".to_owned(),
            user.clone(),
            format!("/staff/claims/{}", t.open),
            format!("/staff/documents/{}", t.document),
        ] {
            site.hit(&page);
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
        expect_post(site, (who, &cookie), &path, "decision=approve", false, &claim_page, "/staff?notice=claim_decided")
            .await;
        assert_eq!(site.claim_status(t.open).await == "approved", who.may(false), "{who:?}");

        // Approving a TIN that was paused before (on another account):
        // Admins only, and Support hears why.
        let claim_page = format!("/staff/claims/{}", t.paused_before);
        site.hit(&format!("{claim_page}/decision"));
        let response = site.post(&format!("{claim_page}/decision"), &cookie, "decision=approve").await;
        match who {
            Who::SignedOut => assert!(location(&response).starts_with("/login?"), "{who:?}"),
            Who::User => assert_eq!(response.status(), StatusCode::NOT_FOUND, "{who:?}"),
            Who::Support => assert_eq!(location(&response), format!("{claim_page}?notice=admin_must_decide")),
            Who::Admin | Who::ServerAdmin => assert_eq!(location(&response), "/staff?notice=claim_decided"),
        }
        assert_eq!(site.claim_status(t.paused_before).await == "approved", who.may(true), "{who:?}");

        // Delete a claim: Admins only.
        let claim_page = format!("/staff/claims/{}", t.unverified);
        let path = format!("{claim_page}/delete");
        expect_post(site, (who, &cookie), &path, "reason=Duplicate", true, &claim_page, "/staff?notice=claim_deleted")
            .await;
        assert_eq!(!claim_exists(&site.db, t.unverified).await, who.may(true), "{who:?}");

        // End an ownership: Admins only.
        let claim_page = format!("/staff/claims/{}", t.active);
        let path = format!("{claim_page}/revoke");
        let done = format!("{claim_page}?notice=ownership_revoked");
        expect_post(site, (who, &cookie), &path, "reason=Wrong+owner", true, &claim_page, &done).await;
        assert_eq!(site.claim_status(t.active).await == "revoked", who.may(true), "{who:?}");

        // Activate without papers: Admins only.
        let claim_page = format!("/staff/claims/{}", t.spare);
        let path = format!("{claim_page}/activate");
        let done = format!("{claim_page}?notice=tin_activated");
        expect_post(site, (who, &cookie), &path, "reason=Known+owner", true, &claim_page, &done).await;
        assert_eq!(site.claim_status(t.spare).await == "approved", who.may(true), "{who:?}");

        // Pause and resume: Admins only.
        let other = add_claim(&site.db, t.account, &format!("5{n:08}"), "approved").await;
        let claim_page = format!("/staff/claims/{other}");
        let paused = || async {
            sqlx::query_scalar::<_, bool>("SELECT paused_at IS NOT NULL FROM tin_claims WHERE id = $1")
                .bind(other)
                .fetch_one(&site.db)
                .await
                .unwrap()
        };
        let done = format!("{claim_page}?notice=tin_paused");
        expect_post(site, (who, &cookie), &format!("{claim_page}/pause"), "reason=Check", true, &claim_page, &done)
            .await;
        assert_eq!(paused().await, who.may(true), "{who:?}");
        let done = format!("{claim_page}?notice=tin_resumed");
        expect_post(site, (who, &cookie), &format!("{claim_page}/resume"), "reason=Ok", true, &claim_page, &done).await;
        assert!(!paused().await, "{who:?}");

        // Confirm an automatic activation: Support and Admins.
        let auto = add_claim(&site.db, t.account, &format!("6{n:08}"), "approved").await;
        sqlx::query("UPDATE tin_claims SET verified_by = 'auto', decided_by = NULL WHERE id = $1")
            .bind(auto)
            .execute(&site.db)
            .await
            .unwrap();
        let claim_page = format!("/staff/claims/{auto}");
        let done = format!("{claim_page}?notice=auto_confirmed");
        expect_post(site, (who, &cookie), &format!("{claim_page}/confirm"), "", false, &claim_page, &done).await;
        let confirmed: bool = sqlx::query_scalar("SELECT auto_confirmed_at IS NOT NULL FROM tin_claims WHERE id = $1")
            .bind(auto)
            .fetch_one(&site.db)
            .await
            .unwrap();
        assert_eq!(confirmed, who.may(false), "{who:?}");

        // Confirm one Buwiz paused for waiting too long: lifting that pause
        // is an Admin's, with a reason; Support hears why.
        let paused_auto = add_claim(&site.db, t.account, &format!("8{n:08}"), "approved").await;
        sqlx::query(
            "UPDATE tin_claims SET verified_by = 'auto', decided_by = NULL, paused_at = now(), pause_reason = 'Unconfirmed'
             WHERE id = $1",
        )
        .bind(paused_auto)
        .execute(&site.db)
        .await
        .unwrap();
        let claim_page = format!("/staff/claims/{paused_auto}");
        site.hit(&format!("{claim_page}/confirm"));
        let response = site.post(&format!("{claim_page}/confirm"), &cookie, "reason=Checked").await;
        match who {
            Who::SignedOut => assert!(location(&response).starts_with("/login?"), "{who:?}"),
            Who::User => assert_eq!(response.status(), StatusCode::NOT_FOUND, "{who:?}"),
            Who::Support => assert_eq!(location(&response), format!("{claim_page}?notice=admin_must_confirm")),
            Who::Admin | Who::ServerAdmin => {
                assert_eq!(location(&response), format!("{claim_page}?notice=auto_confirmed"))
            }
        }
        let (confirmed, paused): (bool, bool) =
            sqlx::query_as("SELECT auto_confirmed_at IS NOT NULL, paused_at IS NOT NULL FROM tin_claims WHERE id = $1")
                .bind(paused_auto)
                .fetch_one(&site.db)
                .await
                .unwrap();
        assert_eq!((confirmed, !paused), (who.may(true), who.may(true)), "{who:?}");

        // Change a role: Admins only.
        let path = format!("{user}/role");
        let done = format!("{user}?notice=role_changed");
        expect_post(site, (who, &cookie), &path, "role=support&reason=Joins+the+team", true, &user, &done).await;
        assert_eq!(stored_role(&site.db, t.account).await.is_some(), who.may(true), "{who:?}");

        // One app: Support and Admins.
        let path = format!("{user}/apps/{}/sign-out", t.grant);
        let done = format!("{user}?notice=app_signed_out");
        expect_post(site, (who, &cookie), &path, "reason=Lost+phone", false, &user, &done).await;
        assert_eq!(grant_revoked(&site.db, t.grant).await, who.may(false), "{who:?}");

        // Everywhere: Support and Admins.
        let path = format!("{user}/sign-out");
        let done = format!("{user}?notice=signed_out_everywhere");
        expect_post(site, (who, &cookie), &path, "reason=Lost+laptop", false, &user, &done).await;
        assert_eq!(live_sessions(&site.db, t.account).await == 0, who.may(false), "{who:?}");

        // The test label: Admins only.
        let done = format!("{user}?notice=test_marked");
        expect_post(site, (who, &cookie), &format!("{user}/test"), "test=on&reason=Trial", true, &user, &done).await;
        let labelled: bool = sqlx::query_scalar("SELECT test_account FROM accounts WHERE id = $1")
            .bind(t.account)
            .fetch_one(&site.db)
            .await
            .unwrap();
        assert_eq!(labelled, who.may(true), "{who:?}");

        // A new login: Admins only.
        let email = format!("new-{}@example.test", format!("{who:?}").to_lowercase());
        site.hit("/staff/users/new");
        let response =
            site.post("/staff/users/new", &cookie, &format!("email={}&reason=Joins", email.replace('@', "%40"))).await;
        let made: Option<Uuid> = sqlx::query_scalar("SELECT id FROM accounts WHERE email = $1")
            .bind(&email)
            .fetch_optional(&site.db)
            .await
            .unwrap();
        match (who, made) {
            (Who::SignedOut, None) => assert!(location(&response).starts_with("/login?"), "{who:?}"),
            (Who::User, None) => assert_eq!(response.status(), StatusCode::NOT_FOUND, "{who:?}"),
            (Who::Support, None) => assert_eq!(location(&response), "/staff/users?notice=admin_only"),
            (Who::Admin | Who::ServerAdmin, Some(id)) => {
                assert_eq!(location(&response), format!("/staff/users/{id}?notice=login_created"));
            }
            other => panic!("{other:?}"),
        }

        // Ban and lift: Admins only.
        let banned = || async {
            sqlx::query_scalar::<_, bool>("SELECT banned_at IS NOT NULL FROM accounts WHERE id = $1")
                .bind(t.account)
                .fetch_one(&site.db)
                .await
                .unwrap()
        };
        let done = format!("{user}?notice=account_banned_staff");
        expect_post(site, (who, &cookie), &format!("{user}/ban"), "reason=Abuse", true, &user, &done).await;
        assert_eq!(banned().await, who.may(true), "{who:?}");
        let done = format!("{user}?notice=ban_lifted");
        expect_post(site, (who, &cookie), &format!("{user}/unban"), "reason=Sorted", true, &user, &done).await;
        assert!(!banned().await, "{who:?}");
    }
    let hit = site.hits.lock().unwrap().clone();
    let listed: std::collections::BTreeSet<String> = MATRIX.iter().map(|route| (*route).to_owned()).collect();
    assert_eq!(hit, listed, "routes the matrix lists but never tried, or tried but does not list");
}

/// A [`Site`] that remembers which staff routes a test tried.
struct Tracked {
    site: Site,
    hits: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl std::ops::Deref for Tracked {
    type Target = Site;
    fn deref(&self) -> &Site {
        &self.site
    }
}

impl Tracked {
    fn hit(&self, path: &str) {
        self.hits.lock().unwrap().insert(route_of(path));
    }
}

/// The email list decides in one place only, [`roles`]: no route, page,
/// query or the seed tool checks membership of `ADMIN_EMAILS` on its own,
/// in Rust (`.is_admin(`, `admin == email`) or in SQL (`email = ANY(`).
#[test]
fn only_roles_reads_the_email_list() {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                if [".is_admin(", "admin == email", "email = ANY("].iter().any(|reads| source.contains(reads)) {
                    found.push(path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display().to_string());
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    assert_eq!(found, ["src/account/roles.rs"]);
}

/// Every staff route and page in the source, as written there.
fn staff_routes_in_the_source() -> std::collections::BTreeSet<String> {
    let mut routes = std::collections::BTreeSet::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/account");
    let mut files = vec![dir.join("web.rs")];
    for entry in std::fs::read_dir(dir.join("pages")).unwrap() {
        files.push(entry.unwrap().path());
    }
    files.push(dir.join("pages.rs"));
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for marker in [".route(\"", "#[page(\""] {
            for piece in source.split(marker).skip(1) {
                let path = &piece[..piece.find('"').unwrap()];
                if path == "/staff" || path.starts_with("/staff/") {
                    routes.insert(path.to_owned());
                }
            }
        }
    }
    routes
}

/// The roles matrix covers every staff route there is: a new one fails here
/// until the matrix lists it (and the matrix fails until it exercises it).
#[test]
fn the_roles_matrix_lists_every_staff_route() {
    let listed: std::collections::BTreeSet<String> = MATRIX.iter().map(|route| (*route).to_owned()).collect();
    assert_eq!(staff_routes_in_the_source(), listed);
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
    // Their own page opens, without the role form.
    let own = text(site.get(&format!("/staff/users/{ana}"), &ana_cookie).await).await;
    assert!(own.contains("This is your own account: another admin changes its role, ban or test label."), "{own}");
    assert!(!own.contains(&format!("action=\"/staff/users/{ana}/role\"")), "{own}");
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
    let response = site.post(&format!("/staff/users/{boss}/apps/{grant}/sign-out"), &support, "reason=Tidy").await;
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

/// Every Admin action checks its actor again inside its own transaction:
/// one whose page check passed just before the actor was demoted does
/// nothing.
#[sqlx::test(migrations = "./migrations")]
async fn every_admin_action_rechecks_its_actor_in_its_transaction(db: PgPool) {
    use buwiz_cloud::claims::{self, By};
    let site = Site::with_admins(db, &[]);
    let (db, config, store) = (&site.db, &site.state.config, &site.state.documents);
    let (boss, _) = site.staff_with_role("boss@example.test", "admin").await;
    site.staff_with_role("other@example.test", "admin").await;
    let (ana, _) = site.person("ana@example.test").await;
    let (dan, _) = site.person("dan@example.test").await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = $2, ban_reason = 'Fraud' WHERE id = $1")
        .bind(dan)
        .bind(boss)
        .execute(db)
        .await
        .unwrap();
    let unverified = add_claim(db, ana, "000123451", "unverified").await;
    let deletable = add_claim(db, ana, "000123452", "unverified").await;
    let active = add_claim(db, ana, "000123453", "approved").await;
    let revocable = add_claim(db, ana, "000123454", "approved").await;
    let paused = add_claim(db, ana, "000123455", "approved").await;
    sqlx::query("UPDATE tin_claims SET paused_at = now(), paused_by = $2, pause_reason = 'Check' WHERE id = $1")
        .bind(paused)
        .bind(boss)
        .execute(db)
        .await
        .unwrap();
    // The page saw an Admin; then another Admin demoted boss.
    let staff = as_staff(&site, boss).await;
    assert_eq!(staff.role, Role::Admin);
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1").bind(boss).execute(db).await.unwrap();

    let admin_only = |outcome: Result<Notice, DecideError>| match outcome {
        Err(DecideError::Refused(Notice::AdminOnly)) => {}
        other => panic!("not refused: {other:?}"),
    };
    let actor = &staff.account;
    admin_only(admin::ban(db, config, &staff, ana, "Abuse").await);
    admin_only(admin::unban(db, config, &staff, dan, "Sorted").await);
    admin_only(admin::mark_test(db, config, &staff, ana, true, "Trial").await);
    admin_only(claims::activate_without_papers(db, config, store, unverified, actor, "Known").await);
    admin_only(claims::pause(db, config, active, actor, "Check").await);
    admin_only(claims::resume(db, config, paused, actor, "Ok").await);
    admin_only(claims::delete(db, store, deletable, actor, By::Staff { reason: "Typo", config }).await);
    admin_only(claims::end_ownership(db, revocable, actor, By::Staff { reason: "Wrong owner", config }).await);

    // Nothing changed, and nothing was written to the history.
    let state: (bool, bool, bool) = sqlx::query_as(
        "SELECT (SELECT banned_at IS NOT NULL FROM accounts WHERE id = $1), (SELECT test_account FROM accounts WHERE id = $1),
                (SELECT banned_at IS NOT NULL FROM accounts WHERE id = $2)",
    )
    .bind(ana)
    .bind(dan)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(state, (false, false, true));
    for (claim, status) in
        [(unverified, "unverified"), (deletable, "unverified"), (active, "approved"), (revocable, "approved")]
    {
        assert_eq!(site.claim_status(claim).await, status);
    }
    let still_paused: bool = sqlx::query_scalar("SELECT paused_at IS NOT NULL FROM tin_claims WHERE id = $1")
        .bind(paused)
        .fetch_one(db)
        .await
        .unwrap();
    assert!(still_paused);
    assert!(claim_audit_lines(db, ana).await.is_empty());
    assert!(account_audit_lines(db, ana).await.is_empty() && account_audit_lines(db, dan).await.is_empty());
}

/// Support's own actions (confirm, sign out everywhere, sign one app out)
/// recheck their actor too: one demoted after the page's check does
/// nothing, and an Admin demoted to Support no longer signs out an Admin.
#[sqlx::test(migrations = "./migrations")]
async fn support_actions_recheck_their_actor_in_their_transaction(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (db, config, store) = (&site.db, &site.state.config, &site.state.documents);
    let (helper, _) = site.staff_with_role("helper@example.test", "support").await;
    let (boss, _) = site.staff_with_role("boss@example.test", "admin").await;
    let (other, _) = site.staff_with_role("other@example.test", "admin").await;
    let (ana, _) = site.person("ana@example.test").await;
    let auto = add_claim(db, ana, "000123451", "approved").await;
    sqlx::query("UPDATE tin_claims SET verified_by = 'auto', decided_by = NULL WHERE id = $1")
        .bind(auto)
        .execute(db)
        .await
        .unwrap();
    app_token(db, ana).await;
    let grant: Uuid =
        sqlx::query_scalar("SELECT id FROM oauth_grants WHERE account_id = $1").bind(ana).fetch_one(db).await.unwrap();
    app_token(db, other).await;
    let sessions = live_sessions(db, ana).await;

    let admin_only = |outcome: Result<Notice, DecideError>| match outcome {
        Err(DecideError::Refused(Notice::AdminOnly)) => {}
        other => panic!("not refused: {other:?}"),
    };
    // The page saw Support; then an Admin took the role away.
    let staff = as_staff(&site, helper).await;
    sqlx::query("UPDATE accounts SET role = NULL WHERE id = $1").bind(helper).execute(db).await.unwrap();
    admin_only(buwiz_cloud::account::verify::confirm(db, config, store, auto, &staff.account, "").await);
    admin_only(admin::sign_out(db, config, &staff, ana, "Lost laptop").await);
    admin_only(admin::sign_out_app(db, config, &staff, ana, grant, "Lost phone").await);
    // The page saw an Admin, now Support: an Admin is no longer theirs to
    // sign out.
    let staff = as_staff(&site, boss).await;
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1").bind(boss).execute(db).await.unwrap();
    admin_only(admin::sign_out(db, config, &staff, other, "Lost laptop").await);

    let confirmed: bool = sqlx::query_scalar("SELECT auto_confirmed_at IS NOT NULL FROM tin_claims WHERE id = $1")
        .bind(auto)
        .fetch_one(db)
        .await
        .unwrap();
    assert!(!confirmed);
    assert!(!grant_revoked(db, grant).await);
    assert_eq!(live_sessions(db, ana).await, sessions);
    let other_grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM oauth_grants WHERE account_id = $1 AND revoked_at IS NULL")
            .bind(other)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(other_grants, 1);
    assert!(claim_audit_lines(db, ana).await.is_empty());
    assert!(account_audit_lines(db, ana).await.is_empty() && account_audit_lines(db, other).await.is_empty());
}

/// A staff member who is also a claimant reads their own page in full,
/// with the reasons staff wrote about them: why an Admin activated their
/// TIN, where a TIN of theirs went.
#[sqlx::test(migrations = "./migrations")]
async fn staff_read_their_own_page_in_full(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (helper, helper_cookie) = site.staff_with_role("helper@example.test", "support").await;
    let claim = add_claim(&site.db, helper, "000123453", "unverified").await;
    let reason = "Known+to+the+owner+from+the+old+office";
    let response = site.post(&format!("/staff/claims/{claim}/activate"), &staff, &format!("reason={reason}")).await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_activated"));
    // The seed moved another of helper's TINs: only the audit line says where.
    let moved = add_claim(&site.db, helper, "000123454", "revoked").await;
    sqlx::query(
        "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
         VALUES ($1, $2, '***-***-454-00000', 'revoked', 'approved', $2, 'boss@example.test',
                 'seed 1: moved to carl@example.test by the seed file: Moves', 'seed')",
    )
    .bind(moved)
    .bind(helper)
    .execute(&site.db)
    .await
    .unwrap();

    for cookie in [&helper_cookie, &staff] {
        let response = site.get(&format!("/staff/users/{helper}"), cookie).await;
        assert_eq!(response.status(), StatusCode::OK);
        let history = text(response).await;
        assert!(history.contains("Known to the owner from the old office"), "{history}");
        assert!(history.contains("moved to carl@example.test"), "{history}");
    }
    // Support has no account powers, on their own page or anyone's.
    let own = text(site.get(&format!("/staff/users/{helper}"), &helper_cookie).await).await;
    assert!(own.contains("Only an admin can change roles, bans and test labels."), "{own}");
}

/// On the page of their own claim, staff read the note of the staff member
/// who approved it and the reasons it was paused and resumed, as everyone
/// on staff does.
#[sqlx::test(migrations = "./migrations")]
async fn staff_read_the_decision_note_and_pause_reasons_of_their_own_claim(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (helper, helper_cookie) = site.staff_with_role("helper@example.test", "support").await;
    let claim = add_claim(&site.db, helper, "000123453", "pending").await;
    let page = format!("/staff/claims/{claim}");
    let response = site.post(&format!("{page}/decision"), &staff, "decision=approve&note=Vouched+for+by+Ana").await;
    assert_eq!(location(&response), "/staff?notice=claim_decided");
    site.post(&format!("{page}/pause"), &staff, "reason=Complaint+from+a+client").await;
    site.post(&format!("{page}/resume"), &staff, "reason=Client+withdrew+it").await;

    for cookie in [&helper_cookie, &staff] {
        let body = text(site.get(&page, cookie).await).await;
        for note in ["Vouched for by Ana", "Complaint from a client", "Client withdrew it"] {
            assert!(body.contains(note), "{note}: {body}");
        }
    }
}

/// Signing one app out needs a reason, like every other staff action, and
/// leaves a line only when it ended a sign-in.
#[sqlx::test(migrations = "./migrations")]
async fn signing_one_app_out_needs_a_reason_and_says_when_nothing_changed(db: PgPool) {
    let site = Site::new(db);
    let (_, helper) = site.staff_with_role("helper@example.test", "support").await;
    let (ana, _) = site.person("ana@example.test").await;
    app_token(&site.db, ana).await;
    let grant: Uuid = sqlx::query_scalar("SELECT id FROM oauth_grants WHERE account_id = $1")
        .bind(ana)
        .fetch_one(&site.db)
        .await
        .unwrap();
    let user = format!("/staff/users/{ana}");
    let path = format!("{user}/apps/{grant}/sign-out");
    for form in ["", "reason=+"] {
        assert_eq!(location(&site.post(&path, &helper, form).await), format!("{user}?notice=reason_missing"));
    }
    assert!(!grant_revoked(&site.db, grant).await);
    assert_eq!(
        location(&site.post(&path, &helper, "reason=Lost+phone").await),
        format!("{user}?notice=app_signed_out")
    );
    assert!(grant_revoked(&site.db, grant).await);
    // Again, or a grant that is not this account's: nothing changed.
    let again = site.post(&path, &helper, "reason=Lost+phone").await;
    assert_eq!(location(&again), format!("{user}?notice=app_already_signed_out"));
    let missing = site.post(&format!("{user}/apps/{}/sign-out", Uuid::new_v4()), &helper, "reason=x").await;
    assert_eq!(location(&missing), format!("{user}?notice=app_already_signed_out"));
    let lines = account_audit_lines(&site.db, ana).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!((lines[0].0.as_str(), lines[0].5.as_deref()), ("app_signed_out", Some("Lost phone")));
}

/// Reasons are 500 characters at most, as on the page; a role change to
/// the same role says nothing changed.
#[sqlx::test(migrations = "./migrations")]
async fn reasons_are_short_and_no_op_or_own_actions_say_so(db: PgPool) {
    let site = Site::new(db);
    let (_, boss_cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let (ana, _) = site.staff_with_role("ana@example.test", "support").await;
    let unverified = add_claim(&site.db, ana, "000123451", "unverified").await;
    let active = add_claim(&site.db, ana, "000123452", "approved").await;
    let pending = add_claim(&site.db, ana, "000123453", "pending").await;
    let long = "a".repeat(501);
    let user = format!("/staff/users/{ana}");
    for (path, form, back) in [
        (format!("{user}/ban"), format!("reason={long}"), user.clone()),
        (format!("{user}/sign-out"), format!("reason={long}"), user.clone()),
        (format!("{user}/role"), format!("role=none&reason={long}"), user.clone()),
        (format!("/staff/claims/{active}/pause"), format!("reason={long}"), format!("/staff/claims/{active}")),
        (format!("/staff/claims/{active}/revoke"), format!("reason={long}"), format!("/staff/claims/{active}")),
        (
            format!("/staff/claims/{unverified}/activate"),
            format!("reason={long}"),
            format!("/staff/claims/{unverified}"),
        ),
        (format!("/staff/claims/{unverified}/delete"), format!("reason={long}"), format!("/staff/claims/{unverified}")),
        (
            format!("/staff/claims/{pending}/decision"),
            format!("decision=reject&note={long}"),
            format!("/staff/claims/{pending}"),
        ),
    ] {
        let response = site.post(&path, &boss_cookie, &form).await;
        assert_eq!(location(&response), format!("{back}?notice=reason_too_long"), "{path}");
    }
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
    assert!(claim_audit_lines(&site.db, ana).await.is_empty());
    // 500 is fine.
    let response =
        site.post(&format!("/staff/claims/{active}/pause"), &boss_cookie, &format!("reason={}", "a".repeat(500))).await;
    assert_eq!(location(&response), format!("/staff/claims/{active}?notice=tin_paused"));

    let response = site.post(&format!("{user}/role"), &boss_cookie, "role=support&reason=Again").await;
    assert_eq!(location(&response), format!("{user}?notice=role_unchanged"));
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
}

/// Every staff action, posted from another site (or with no `Origin` and a
/// cross-site fetch), is refused before anything else and changes nothing,
/// even for an Admin.
#[sqlx::test(migrations = "./migrations")]
async fn staff_actions_refuse_cross_site_posts(db: PgPool) {
    let site = Site::new(db);
    let (_, admin) = site.person(STAFF).await;
    let t = target(&site, 0).await;
    let user = format!("/staff/users/{}", t.account);
    let posts = staff_posts(&t, &user);
    let before = everything(&site.db).await;
    for (path, form) in &posts {
        for (origin, fetch) in [(Some("https://evil.example.test"), None), (None, Some("cross-site")), (None, None)] {
            let mut request = Request::post(path.as_str())
                .header(header::COOKIE, &admin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if let Some(fetch) = fetch {
                request = request.header("sec-fetch-site", fetch);
            }
            let response = site.send(request.body(Body::from(form.clone())).unwrap()).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path} {origin:?} {fetch:?}");
        }
    }
    assert_eq!(everything(&site.db).await, before);
}

/// A banned Support member has no website session: every staff action
/// asks them to sign in and changes nothing.
#[sqlx::test(migrations = "./migrations")]
async fn a_banned_support_member_cannot_use_staff_powers(db: PgPool) {
    let site = Site::new(db);
    let (helper, cookie) = site.staff_with_role("helper@example.test", "support").await;
    let (admin, _) = site.person(STAFF).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = $2, ban_reason = 'Left' WHERE id = $1")
        .bind(helper)
        .bind(admin)
        .execute(&site.db)
        .await
        .unwrap();
    let t = target(&site, 0).await;
    let user = format!("/staff/users/{}", t.account);
    let before = everything(&site.db).await;
    for (path, form) in staff_posts(&t, &user) {
        let response = site.post(&path, &cookie, &form).await;
        assert!(location(&response).starts_with("/login?"), "{path}: {} {}", response.status(), location(&response));
    }
    for page in ["/staff".to_owned(), "/staff/users".to_owned(), user.clone(), format!("/staff/claims/{}", t.open)] {
        assert!(location(&site.get(&page, &cookie).await).starts_with("/login?"), "{page}");
    }
    assert_eq!(everything(&site.db).await, before);
}

/// Every staff POST route, with a form that would succeed for an Admin.
fn staff_posts(t: &Target, user: &str) -> Vec<(String, String)> {
    let claim = |id: Uuid, action: &str| format!("/staff/claims/{id}/{action}");
    vec![
        (claim(t.open, "decision"), "decision=approve".into()),
        (claim(t.unverified, "delete"), "reason=Duplicate".into()),
        (claim(t.active, "revoke"), "reason=Wrong+owner".into()),
        (claim(t.spare, "activate"), "reason=Known".into()),
        (claim(t.active, "pause"), "reason=Check".into()),
        (claim(t.active, "resume"), "reason=Ok".into()),
        (claim(t.active, "confirm"), String::new()),
        (format!("{user}/sign-out"), "reason=Lost+laptop".into()),
        (format!("{user}/apps/{}/sign-out", t.grant), "reason=Lost+phone".into()),
        (format!("{user}/role"), "role=support&reason=Joins".into()),
        (format!("{user}/ban"), "reason=Abuse".into()),
        (format!("{user}/unban"), "reason=Sorted".into()),
        (format!("{user}/test"), "test=on&reason=Trial".into()),
        ("/staff/users/new".into(), "email=new%40example.test&role=support&reason=Joins".into()),
    ]
}

/// What a staff action could change: claims, accounts, sign-ins and both
/// audit tables.
async fn everything(db: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT concat_ws('|', 'claims', (SELECT string_agg(id || status || coalesce(paused_at::text, ''), ',' ORDER BY id) FROM tin_claims))
         UNION ALL SELECT concat_ws('|', 'accounts', (SELECT string_agg(id || coalesce(role, '') || test_account || coalesce(banned_at::text, ''), ',' ORDER BY id) FROM accounts))
         UNION ALL SELECT concat_ws('|', 'sessions', (SELECT count(*) FROM web_sessions WHERE revoked_at IS NULL))
         UNION ALL SELECT concat_ws('|', 'grants', (SELECT count(*) FROM oauth_grants WHERE revoked_at IS NULL))
         UNION ALL SELECT concat_ws('|', 'audit', (SELECT count(*) FROM claim_audit), (SELECT count(*) FROM account_audit))",
    )
    .fetch_all(db)
    .await
    .unwrap()
}
