//! BIR uptime as apps and people read it: the read routes, the `buwiz-bir`
//! header on responses apps already get, the status page, the My TINs
//! banner and the `buwiz-cloud bir` switch, at every `BIR_UPTIME` level.
//! Made-up data and the local test database only.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use buwiz_cloud::uptime::public::round_down;
use buwiz_cloud::uptime::sign::{Light, NoteKind, SignState};
use buwiz_cloud::uptime::{Level, Uptime};
use common::{FORMS_HOST, STAFF, Site, add_claim, app_token, text, token};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const LEVELS: [Level; 4] = [Level::Off, Level::Record, Level::Public, Level::Enforce];
const FILER: &str = "ana@example.test";
const ROOT: &str = "000123453";

fn site(db: &PgPool, level: Level) -> Site {
    let uptime = Uptime::new(level, vec![22], false, OffsetDateTime::now_utc());
    Site::with_sender(db.clone(), "http://127.0.0.1:9", "a made-up Sender secret, 32 characters or more", uptime)
}

/// Stores a sign in `light` (both windows alike, fresh evidence so the
/// clock keeps it), and lets the site's engine read it as a start does.
async fn sign_in(site: &Site, light: Light) {
    let now = OffsetDateTime::now_utc();
    let mut sign = SignState::new(now - Duration::minutes(20));
    sign.seq = 41;
    sign.revision = 1_000;
    sign.state = light;
    sign.since = now - Duration::minutes(20);
    sign.evaluated_at = now - Duration::seconds(20);
    sign.last_success_at = Some(now - Duration::minutes(3));
    sign.last_news_at = Some(now - Duration::minutes(1));
    for lamp in [&mut sign.dispatcher, &mut sign.upload] {
        lamp.state = light;
        lamp.since = now - Duration::minutes(20);
        lamp.last_ok_at = Some(now - Duration::minutes(3));
        lamp.last_evidence_at = Some(now - Duration::minutes(1));
    }
    sqlx::query(
        "INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, $1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET seq = EXCLUDED.seq, state = EXCLUDED.state, updated_at = EXCLUDED.updated_at",
    )
    .bind(sign.seq)
    .bind(serde_json::to_value(&sign).unwrap())
    .bind(now)
    .execute(&site.db)
    .await
    .unwrap();
    buwiz_cloud::uptime::start(&site.state).await;
    assert_eq!(site.state.uptime.live.sign().state, light);
}

async fn get(site: &Site, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut request = Request::get(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    site.send(request.body(Body::empty()).unwrap()).await
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).map(|value| value.to_str().unwrap())
}

async fn json(response: Response) -> Value {
    serde_json::from_str(&text(response).await).unwrap()
}

/// The response, as bytes, with what changes by itself (the date) left out.
async fn snapshot(response: Response) -> (u16, Vec<(String, String)>, String) {
    let status = response.status().as_u16();
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "date")
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or_default().to_owned()))
        .collect();
    headers.sort();
    (status, headers, text(response).await)
}

/// Below `public` every new address answers exactly what an address that
/// never existed does, and from `public` they exist.
#[sqlx::test]
async fn the_routes_exist_only_from_public(db: PgPool) {
    let (account, cookie) = Site::new(db.clone()).person(FILER).await;
    let bearer = format!("Bearer {}", app_token(&db, account).await);
    for level in LEVELS {
        let site = site(&db, level);
        let never = snapshot(get(&site, "/api/bir/never-was", &[]).await).await;
        let page_never = snapshot(site.on_host(FORMS_HOST, "/never-was-a-page").await).await;
        for path in ["/api/bir/status", "/api/bir/status/mine", "/api/bir/history", "/assets/bir-status.css"] {
            let response = get(&site, path, &[("authorization", &bearer)]).await;
            if level.is_public() {
                assert_eq!(response.status(), StatusCode::OK, "{path} at {level:?}");
            } else {
                let mut got = snapshot(response).await;
                let mut expected = never.clone();
                // The 404 page names no path; what differs is nothing.
                got.2 = got.2.replace(path, "");
                expected.2 = expected.2.replace("/api/bir/never-was", "");
                assert_eq!(got, expected, "{path} at {level:?}");
            }
        }
        let page = site.on_host(FORMS_HOST, "/bir-status").await;
        if level.is_public() {
            assert_eq!(page.status(), StatusCode::OK);
            // Not on the account host.
            assert_eq!(site.get("/bir-status", &cookie).await.status(), StatusCode::NOT_FOUND);
        } else {
            let mut got = snapshot(page).await;
            got.2 = got.2.replace("/bir-status", "");
            let mut expected = page_never.clone();
            expected.2 = expected.2.replace("/never-was-a-page", "");
            assert_eq!(got, expected, "the page at {level:?}");
        }
        // The Bot host sends it to the filing site from `public` only.
        let bot = site.on_host("buwiz.com", "/bir-status").await;
        if level.is_public() {
            assert_eq!(bot.status(), StatusCode::MOVED_PERMANENTLY);
            assert_eq!(header(&bot, "location"), Some("https://ebirforms.buwiz.com/bir-status"));
        } else {
            assert_ne!(bot.status(), StatusCode::MOVED_PERMANENTLY);
        }
    }
}

/// The public document: the same bytes for everyone, past times on 5-minute
/// marks, nothing personal, an ETag that answers 304, and short public
/// caching.
#[sqlx::test]
async fn the_public_sign_is_the_same_for_everyone(db: PgPool) {
    let site = site(&db, Level::Public);
    sign_in(&site, Light::Offline).await;
    let (account, cookie) = site.person(FILER).await;
    add_claim(&db, account, ROOT, "approved").await;
    let bearer = format!("Bearer {}", app_token(&db, account).await);
    let anonymous = get(&site, "/api/bir/status", &[("cf-connecting-ip", "203.0.113.7")]).await;
    let etag = header(&anonymous, "etag").unwrap().to_owned();
    assert_eq!(header(&anonymous, "cache-control"), Some("public, max-age=15, stale-while-revalidate=30"));
    assert_eq!(header(&anonymous, "vary"), None);
    let body = text(anonymous).await;
    for headers in [
        vec![("cf-connecting-ip", "198.51.100.9"), ("cookie", cookie.as_str())],
        vec![("cf-connecting-ip", "2001:db8::1"), ("authorization", bearer.as_str())],
    ] {
        let response = get(&site, "/api/bir/status", &headers).await;
        assert_eq!(header(&response, "etag"), Some(etag.as_str()));
        assert_eq!(text(response).await, body, "identical for every caller");
    }
    let document: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((document["v"].as_u64(), document["state"].as_str()), (Some(1), Some("offline")));
    assert_eq!((document["source"].as_str(), document["relay"].as_str()), (Some("tries"), Some("ok")));
    assert_eq!(document["links"]["status_page"], "https://ebirforms.buwiz.com/bir-status");
    assert!(document["words"]["title"].as_str().unwrap().contains("offline"));
    for planned in ["scout_at", "wait_until", "admission", "queue", "detail"] {
        assert!(document.get(planned).is_none(), "{planned} is not built: left out");
    }
    for at in [
        &document["since"],
        &document["evaluated_at"],
        &document["last_success_at"],
        &document["windows"]["upload"]["since"],
    ] {
        let at = OffsetDateTime::parse(at.as_str().unwrap(), &time::format_description::well_known::Rfc3339).unwrap();
        assert_eq!(at, round_down(at), "{at} is on a 5-minute mark");
    }
    for personal in [account.to_string().as_str(), FILER, ROOT, "203.0.113", "198.51.100", "2001:db8"] {
        assert!(!body.contains(personal), "{personal}");
    }
    // 304 for the current tag (and `*`), with the same caching.
    for tag in [etag.as_str(), "*", &format!("W/{etag}"), &format!("\"other\", {etag}")] {
        let again = get(&site, "/api/bir/status", &[("if-none-match", tag)]).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "{tag}");
        assert_eq!(header(&again, "etag"), Some(etag.as_str()));
        assert!(text(again).await.is_empty());
    }
    assert_eq!(get(&site, "/api/bir/status", &[("if-none-match", "\"b-1-1\"")]).await.status(), StatusCode::OK);
    // The tag is a hash of the bytes: no seq, no revision, no time in it.
    assert_eq!(etag.len(), 26, "{etag}");
    for part in [document["seq"].to_string(), site.state.uptime.live.sign().revision.to_string()] {
        assert!(!etag.contains(&part), "{etag}");
    }
    assert!(document.get("last_news_at").is_none(), "never public");
}

/// A made-up live try that got in, or that the mailbox refused.
fn observed(landed: bool) -> buwiz_cloud::uptime::observe::Observed {
    let mut observer = buwiz_cloud::uptime::observe::TryObserver::new(&[22]);
    observer.sender_text(r#"{"type":"sender","mode":"live"}"#);
    observer.sender_text(r#"{"type":"open","stream":1,"host":"dispatcher.test","port":443}"#);
    observer.app_text(r#"{"type":"opened","stream":1}"#);
    observer.app_binary(&[1, b'H', b'T', b'T', b'P']);
    observer.sender_text(r#"{"type":"open","stream":2,"host":"upload.test","port":22}"#);
    if landed {
        observer.app_text(r#"{"type":"opened","stream":2}"#);
        observer.app_binary(&[2, b'S', b'S', b'H']);
        observer.finish("sent", None)
    } else {
        observer.app_text(r#"{"type":"refused","stream":2,"reason":"Connection refused (os error 61)"}"#);
        observer.finish("failed", Some("could not reach BIR's upload server: refused"))
    }
}

/// The signed-in read: a Bearer token of any scope or the website session;
/// the live sign for everyone signed in, the counts for staff only; a
/// banned account refused; never kept by any cache, never 304.
#[sqlx::test]
async fn the_signed_in_detail_is_for_staff_and_never_shared(db: PgPool) {
    let site = site(&db, Level::Public);
    sign_in(&site, Light::Busy).await;
    // Real tries: three accounts get in, one is refused.
    let now = OffsetDateTime::now_utc();
    for (account, landed) in [(1u128, true), (2, true), (3, true), (4, false)] {
        let relay = buwiz_cloud::uptime::store::RelayTry {
            account_id: Uuid::from_u128(account),
            network: buwiz_cloud::uptime::keys::Network::V4([198, 51, account as u8]),
            client: None,
            admission: "open".into(),
            started_at: now - Duration::seconds(20),
        };
        buwiz_cloud::uptime::store::record(&site.state.uptime, &db, &relay, &observed(landed), now).await;
    }
    let (account, cookie) = site.person(FILER).await;
    let (_, staff_cookie) = site.staff_with_role("support@example.test", "support").await;
    let (_, admin_cookie) = site.person(STAFF).await;
    let etag = header(&get(&site, "/api/bir/status", &[]).await, "etag").unwrap().to_owned();
    // Anonymous, with the public validator: 401, not 304.
    let anonymous = get(&site, "/api/bir/status/mine", &[("if-none-match", etag.as_str())]).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(header(&anonymous, "cache-control"), Some("no-store"));
    let bad = get(&site, "/api/bir/status/mine", &[("authorization", "Bearer bwa_made-up")]).await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    let mine = |headers: Vec<(String, String)>| {
        let site = &site;
        let etag = etag.clone();
        async move {
            let mut all: Vec<(&str, &str)> =
                headers.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            all.push(("if-none-match", etag.as_str()));
            let response = get(site, "/api/bir/status/mine", &all).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(header(&response, "cache-control"), Some("no-store"));
            assert_eq!(header(&response, "etag"), None);
            text(response).await
        }
    };
    // Anyone signed in: the live sign (Online now, after the landings),
    // times on 5-minute marks, no counts.
    let mut auth = vec![("cookie".to_owned(), cookie.clone())];
    for (client, scope) in [("buwiz-desktop", "app"), ("buwiz-books", "books"), ("made-up-agent", "mcp")] {
        auth.push(("authorization".to_owned(), format!("Bearer {}", token(&db, account, client, scope).await)));
    }
    for (name, value) in &auth {
        let body = mine(vec![(name.clone(), value.clone())]).await;
        let read: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(read["state"], site.state.uptime.live.sign().state.as_str(), "{name}: the live sign");
        assert!(read.get("detail").is_none(), "{name}: no counts for a filer");
        assert!(read.get("last_news_at").is_none(), "{name}");
        let at = OffsetDateTime::parse(read["since"].as_str().unwrap(), &time::format_description::well_known::Rfc3339)
            .unwrap();
        assert_eq!(at, round_down(at), "{name}: public times");
        assert!(!body.contains(&account.to_string()) && !body.contains(FILER));
    }
    // Staff, Support or Admin: times to the minute and the counts, real.
    for staff in [&staff_cookie, &admin_cookie] {
        let read: Value = serde_json::from_str(&mine(vec![("cookie".into(), staff.clone())]).await).unwrap();
        assert_eq!(read["detail"]["today"]["tries"], 4);
        assert_eq!(read["detail"]["today"]["got_in"], 3);
        assert_eq!(read["detail"]["today"]["accounts"], 4);
        assert_eq!(read["detail"]["last_10"]["of"], 4);
        assert_eq!(read["detail"]["last_10"]["got_in"], 3);
        assert!(read["last_news_at"].is_string());
        assert!(read["detail"].get("yours").is_none(), "nothing else personal in this release");
    }
    // A banned account: 403, by its session as by its token.
    let banned_token = format!("Bearer {}", app_token(&db, account).await);
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Made-up' WHERE id = $1")
        .bind(account)
        .execute(&db)
        .await
        .unwrap();
    for (name, value) in [("cookie", cookie.as_str()), ("authorization", banned_token.as_str())] {
        let response = get(&site, "/api/bir/status/mine", &[(name, value)]).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{name}");
        assert_eq!(header(&response, "cache-control"), Some("no-store"));
        assert_eq!(json(response).await["error"]["code"], "account_banned");
    }
}

/// 120 public reads a minute per address (an IPv6 /64 is one), 60 signed-in
/// reads per account; `429 rate_limited` with `Retry-After`, never cached.
#[sqlx::test]
async fn reads_are_limited_per_address_and_per_account(db: PgPool) {
    let site = site(&db, Level::Public);
    for _ in 0..120 {
        let response = get(&site, "/api/bir/status", &[("cf-connecting-ip", "203.0.113.7")]).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let limited = get(&site, "/api/bir/history", &[("cf-connecting-ip", "203.0.113.7")]).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(header(&limited, "retry-after").unwrap().parse::<u32>().unwrap() >= 1);
    assert_eq!(header(&limited, "cache-control"), Some("no-store"));
    assert_eq!(json(limited).await["error"]["code"], "rate_limited");
    assert_eq!(get(&site, "/api/bir/status", &[("cf-connecting-ip", "203.0.113.8")]).await.status(), StatusCode::OK);
    // One /64.
    for host in 1..=120 {
        let address = format!("2001:db8:7:8::{host:x}");
        let response = get(&site, "/api/bir/status", &[("cf-connecting-ip", &address)]).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let same = get(&site, "/api/bir/status", &[("cf-connecting-ip", "2001:db8:7:8:abcd::1")]).await;
    assert_eq!(same.status(), StatusCode::TOO_MANY_REQUESTS);
    let next = get(&site, "/api/bir/status", &[("cf-connecting-ip", "2001:db8:7:9::1")]).await;
    assert_eq!(next.status(), StatusCode::OK);
    // Per account, whatever the address.
    let (_, cookie) = site.person(FILER).await;
    for n in 0..60 {
        let address = format!("192.0.2.{n}");
        let response = get(&site, "/api/bir/status/mine", &[("cookie", &cookie), ("cf-connecting-ip", &address)]).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let over = get(&site, "/api/bir/status/mine", &[("cookie", &cookie), ("cf-connecting-ip", "192.0.2.200")]).await;
    assert_eq!(over.status(), StatusCode::TOO_MANY_REQUESTS);
}

async fn call(site: &Site, method: &str, path: &str, bearer: &str) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", bearer)
        .header("content-type", "application/json")
        .body(Body::from(if method == "POST" { "{}" } else { "" }))
        .unwrap();
    site.send(request).await
}

/// The header rides on the responses the contract lists, whatever their
/// status, from `public` only; nothing else about them changes.
#[sqlx::test]
async fn the_compact_sign_rides_on_existing_responses(db: PgPool) {
    let (account, _) = Site::new(db.clone()).person(FILER).await;
    add_claim(&db, account, ROOT, "approved").await;
    let bearer = format!("Bearer {}", app_token(&db, account).await);
    let calls: Vec<(&str, &str)> = vec![
        ("GET", "/.well-known/buwiz-trust.json"),
        ("GET", "/.well-known/buwiz-hpke.json"),
        ("POST", "/api/tickets"),
        ("POST", "/api/v2/submissions"),
        ("POST", "/api/submissions"),
        ("GET", "/api/tax-profiles"),
    ];
    let record = site(&db, Level::Record);
    let public = site(&db, Level::Public);
    sign_in(&public, Light::Busy).await;
    for (method, path) in &calls {
        let before = call(&record, method, path, &bearer).await;
        assert_eq!(header(&before, "buwiz-bir"), None, "{path} at record");
        let after = call(&public, method, path, &bearer).await;
        let compact = header(&after, "buwiz-bir").unwrap_or_else(|| panic!("{path}")).to_owned();
        assert!(compact.starts_with("v=1, seq="), "{compact}");
        assert!(compact.contains("state=busy, dispatcher=busy, upload=busy, relay=ok, since="), "{compact}");
        assert!(compact.ends_with("note=?0"), "{compact}");
        // Body, status and every other header as they were.
        let (mut before, mut after) = (snapshot(before).await, snapshot(after).await);
        after.1.retain(|(name, _)| name != "buwiz-bir");
        for snapshot in [&mut before, &mut after] {
            // Request ids and the like that differ per call.
            snapshot.2 = snapshot.2.split("\"request_id\"").next().unwrap().to_owned();
        }
        assert_eq!(before, after, "{method} {path}");
    }
    // Not on the submissions list, which the contract does not name.
    let list = get(&public, "/api/submissions", &[("authorization", &bearer)]).await;
    assert_eq!(header(&list, "buwiz-bir"), None);
    // Newest wins: a change raises `seq`, so a client keeps the newer sign.
    // Signed-in answers carry the live sign at once; those that need no
    // sign-in carry the sign of the last 5-minute mark.
    let seq = |compact: &str| -> i64 {
        compact.split(", ").find_map(|part| part.strip_prefix("seq=")).unwrap().parse().unwrap()
    };
    let mark = round_down(OffsetDateTime::now_utc());
    let first = header(&get(&public, "/.well-known/buwiz-hpke.json", &[]).await, "buwiz-bir").unwrap().to_owned();
    let first_status = text(get(&public, "/api/bir/status", &[]).await).await;
    let admin = admin(&public).await;
    let note = buwiz_cloud::uptime::notes::NewNote {
        kind: NoteKind::Maintenance,
        text: "Made-up maintenance",
        until: OffsetDateTime::now_utc() + Duration::hours(2),
    };
    let uptime = &public.state.uptime;
    buwiz_cloud::uptime::notes::set(uptime, &db, &public.state.config, &admin, note, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let live = header(&call(&public, "POST", "/api/tickets", &bearer).await, "buwiz-bir").unwrap().to_owned();
    assert!(seq(&live) > seq(&first));
    assert!(live.contains("state=offline") && live.ends_with("note=?1"), "{live}");
    let open = header(&get(&public, "/.well-known/buwiz-hpke.json", &[]).await, "buwiz-bir").unwrap().to_owned();
    let status = text(get(&public, "/api/bir/status", &[]).await).await;
    if round_down(OffsetDateTime::now_utc()) == mark {
        assert_eq!(open, first, "the public sign moves only on a 5-minute mark");
        assert_eq!(status, first_status);
    }
    // At the next mark, everything public has it.
    let next = mark + Duration::minutes(5);
    let published = uptime.published("https://ebirforms.buwiz.com", next);
    assert_eq!(published.compact, live);
    let status: Value = serde_json::from_slice(&published.body).unwrap();
    assert_eq!(status["seq"].as_i64(), Some(seq(&live)));
    assert_eq!(status["note"]["kind"], "maintenance");
    assert_eq!(status["source"], "staff");
}

async fn admin(site: &Site) -> buwiz_cloud::account::roles::Staff {
    let (id, _) = site.person(STAFF).await;
    let account = buwiz_cloud::account::accounts::Account { id, email: STAFF.into() };
    buwiz_cloud::account::roles::staff(&site.db, &site.state.config, &account).await.unwrap().unwrap()
}

async fn day_line(db: &PgPool, day: time::Date, worst: &str, hours: &str, minutes: [i32; 6]) {
    sqlx::query(
        "INSERT INTO bir_days (day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
             online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, unwatched_minutes, worst, hours, final)
         VALUES ($1, 7, 5, 2, 0, 0, 0, 0, 3, 2, $2, $3, $4, $5, $6, $7, $8, $9, true)",
    )
    .bind(day)
    .bind(minutes[0])
    .bind(minutes[1])
    .bind(minutes[2])
    .bind(minutes[3])
    .bind(minutes[4])
    .bind(minutes[5])
    .bind(worst)
    .bind(hours)
    .execute(db)
    .await
    .unwrap();
}

/// The history: 90 days at most, by default the last 90; the "not
/// watched" letter; durations and shares only, no counts; today live.
#[sqlx::test]
async fn the_history_gives_states_not_counts(db: PgPool) {
    let site = site(&db, Level::Public);
    sign_in(&site, Light::Online).await;
    let today = buwiz_cloud::uptime::keys::manila_day(OffsetDateTime::now_utc());
    let hours = format!("{}{}", "o".repeat(12), "x".repeat(12));
    day_line(&db, today - Duration::days(2), "offline", &hours, [722, 0, 718, 0, 0, 0]).await;
    day_line(&db, today - Duration::days(1), "none", &"w".repeat(24), [0, 0, 0, 0, 0, 1440]).await;
    let response = get(&site, "/api/bir/history", &[]).await;
    assert_eq!(header(&response, "cache-control"), Some("public, max-age=300"));
    let body = text(response).await;
    let history: Value = serde_json::from_str(&body).unwrap();
    let days = history["days"].as_array().unwrap();
    assert_eq!(days.len(), 90);
    assert_eq!(history["last_24h"].as_str().unwrap().len(), 96);
    let iso = |day: time::Date| buwiz_cloud::uptime::history::iso(day);
    assert_eq!(days[89]["day"], iso(today), "today, live");
    assert_eq!(days[0]["state"], "none");
    assert_eq!(days[0]["hours"], "-".repeat(24));
    let outage = &days[87];
    assert_eq!((outage["state"].as_str(), outage["hours"].as_str()), (Some("offline"), Some(hours.as_str())));
    assert_eq!((outage["offline_minutes"].as_i64(), outage["online_minutes"].as_i64()), (Some(720), Some(720)));
    assert_eq!((outage["offline_percent"].as_i64(), outage["online_percent"].as_i64()), (Some(50), Some(50)));
    let unwatched = &days[88];
    assert_eq!(unwatched["state"], "not_watched");
    assert_eq!(unwatched["hours"], "w".repeat(24));
    assert_eq!(unwatched["not_watched_minutes"], 1440);
    assert!(unwatched["online_percent"].is_null(), "no share of time not watched");
    for count in ["\"tries\"", "\"got_in\"", "\"failed\"", "\"accounts\"", "\"networks\"", "\"held\""] {
        assert!(!body.contains(count), "{count}");
    }
    // Ranges.
    let from = iso(today - Duration::days(9));
    let ten = json(get(&site, &format!("/api/bir/history?from={from}&to={}", iso(today)), &[]).await).await;
    assert_eq!(ten["days"].as_array().unwrap().len(), 10);
    for bad in [
        format!("?from={}", iso(today - Duration::days(90))),
        format!("?to={}", iso(today + Duration::days(1))),
        format!("?from={}&to={from}", iso(today)),
        "?from=yesterday".to_owned(),
        // No arithmetic on a year out of range, and a parameter given twice
        // is refused in the same shape.
        "?to=-9999-01-01".to_owned(),
        "?from=-9999-01-01".to_owned(),
        "?to=%2B99999-12-31".to_owned(),
        format!("?from={from}&from={from}"),
        format!("?to={0}&to={0}", iso(today)),
    ] {
        let response = get(&site, &format!("/api/bir/history{bad}"), &[]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(json(response).await["error"]["code"], "bad_range");
    }
}

/// The page: the sign in words, a calendar link per day, no personal data;
/// the footer links it from `public` only, and other pages are as they
/// were.
#[sqlx::test]
async fn the_status_page_and_the_footer_link(db: PgPool) {
    let record = site(&db, Level::Record);
    let public = site(&db, Level::Public);
    sign_in(&public, Light::Offline).await;
    let page = text(public.on_host(FORMS_HOST, "/bir-status").await).await;
    assert!(page.contains("Is the BIR taking returns?"));
    assert!(page.contains("<link rel=\"stylesheet\" href=\"/assets/bir-status.css\">"));
    assert!(!page.contains("noindex"), "a public page");
    assert!(page.contains("class=\"bir-word\">Offline<"), "the state in words");
    assert_eq!(page.matches("class=\"bir-date ").count(), 90, "one link per day");
    assert!(page.contains("aria-current=\"date\""));
    assert!(page.contains("Buwiz never tests the BIR"));
    for personal in [FILER, ROOT, STAFF] {
        assert!(!page.contains(personal));
    }
    // A day picked by link, without script.
    let today = buwiz_cloud::uptime::keys::manila_day(OffsetDateTime::now_utc());
    let day = buwiz_cloud::uptime::history::iso(today - Duration::days(3));
    let picked = text(public.on_host(FORMS_HOST, &format!("/bir-status?day={day}")).await).await;
    assert!(picked.contains(&format!("href=\"?day={day}#day\"")));
    assert_eq!(picked.matches(" sel\"").count(), 1);
    // The footer.
    for path in ["/", "/download"] {
        let before = text(record.on_host(FORMS_HOST, path).await).await;
        let after = text(public.on_host(FORMS_HOST, path).await).await;
        assert!(!before.contains("/bir-status"));
        let link = "<a href=\"/bir-status\">BIR status</a>";
        assert!(after.contains(link), "{path}");
        assert_eq!(after.replacen(link, "", 1), before, "{path}: nothing else changed");
    }
}

/// The My TINs line: Busy, Offline and an outage or maintenance set by hand;
/// not Online, not No recent news, not "declare Online"; never below
/// `public`.
#[sqlx::test]
async fn the_my_tins_banner_follows_the_sign(db: PgPool) {
    let (admin_id, _) = Site::new(db.clone()).person(STAFF).await;
    for (level, light, note, shown) in [
        (Level::Public, Light::Busy, None, Some("busy")),
        (Level::Public, Light::Offline, None, Some("offline")),
        (Level::Enforce, Light::Offline, None, Some("offline")),
        (Level::Public, Light::Online, None, None),
        (Level::Public, Light::NoRecentNews, None, None),
        (Level::Public, Light::Online, Some(NoteKind::Maintenance), Some("maintenance")),
        (Level::Public, Light::Online, Some(NoteKind::Outage), Some("offline")),
        (Level::Public, Light::Online, Some(NoteKind::Online), None),
        (Level::Record, Light::Offline, None, None),
    ] {
        sqlx::query("TRUNCATE bir_sign, bir_events, bir_notes").execute(&db).await.unwrap();
        let site = site(&db, level);
        sign_in(&site, light).await;
        if let Some(kind) = note {
            let account = buwiz_cloud::account::accounts::Account { id: admin_id, email: STAFF.into() };
            let admin = buwiz_cloud::account::roles::staff(&db, &site.state.config, &account).await.unwrap().unwrap();
            let until = OffsetDateTime::now_utc() + Duration::hours(3);
            let note = buwiz_cloud::uptime::notes::NewNote { kind, text: "Made-up note", until };
            buwiz_cloud::uptime::notes::set(
                &site.state.uptime,
                &db,
                &site.state.config,
                &admin,
                note,
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
        }
        let email = format!("{}@example.test", Uuid::new_v4().simple());
        let (_, cookie) = site.person(&email).await;
        let page = text(site.get("/tins", &cookie).await).await;
        match shown {
            Some(tone) => {
                assert!(page.contains(&format!("class=\"bir-banner {tone}\"")), "{level:?} {light:?} {note:?}");
                assert!(page.contains("href=\"https://ebirforms.buwiz.com/bir-status\">BIR status</a>"));
                assert!(page.contains("/assets/bir-status.css"));
            }
            None => {
                assert!(!page.contains("bir-banner"), "{level:?} {light:?} {note:?}");
                assert!(!page.contains("bir-status.css"));
            }
        }
    }
}

/// The status page and the history come from memory, made once a 5-minute
/// mark: a line written meanwhile shows from the next mark. The page is
/// limited per address with the reads, and cached publicly for a minute
/// when the request carries no cookie.
#[sqlx::test]
async fn the_page_and_the_history_cost_no_query_per_view(db: PgPool) {
    let site = site(&db, Level::Public);
    sign_in(&site, Light::Online).await;
    let mark = round_down(OffsetDateTime::now_utc());
    let first = text(get(&site, "/api/bir/history", &[]).await).await;
    let page = site.on_host(FORMS_HOST, "/bir-status").await;
    assert_eq!(header(&page, "cache-control"), Some("public, max-age=60"));
    let first_page = text(page).await;
    // A day line written now ...
    let today = buwiz_cloud::uptime::keys::manila_day(OffsetDateTime::now_utc());
    day_line(&db, today - Duration::days(5), "offline", &"x".repeat(24), [0, 0, 1440, 0, 0, 0]).await;
    // ... is not in what is served within this mark.
    let again = text(get(&site, "/api/bir/history", &[]).await).await;
    let again_page = text(site.on_host(FORMS_HOST, "/bir-status").await).await;
    if round_down(OffsetDateTime::now_utc()) == mark {
        assert_eq!(again, first, "from memory");
        assert_eq!(again_page, first_page, "from memory");
    }
    // The next mark's has it.
    let next = site.state.uptime.history.get(&db, mark + Duration::minutes(5)).await.unwrap();
    let line = next.days.iter().find(|line| line.day == today - Duration::days(5)).unwrap();
    assert_eq!(line.state, "offline");
    // With a cookie the page is the viewer's: never kept by a shared cache.
    let (_, cookie) = site.person(FILER).await;
    let mut request = Request::get("/bir-status").header("host", FORMS_HOST).header("cookie", cookie.as_str());
    request = request.header("cf-connecting-ip", "198.51.100.77");
    let viewer = site.send(request.body(Body::empty()).unwrap()).await;
    assert_eq!(header(&viewer, "cache-control"), Some("no-store"));
    // Limited per address, with the reads.
    for _ in 0..120 {
        let request = Request::get("/bir-status").header("host", FORMS_HOST).header("cf-connecting-ip", "203.0.113.9");
        assert_eq!(site.send(request.body(Body::empty()).unwrap()).await.status(), StatusCode::OK);
    }
    let over = get(&site, "/api/bir/status", &[("cf-connecting-ip", "203.0.113.9")]).await;
    assert_eq!(over.status(), StatusCode::TOO_MANY_REQUESTS);
    let request = Request::get("/bir-status").header("host", FORMS_HOST).header("cf-connecting-ip", "203.0.113.9");
    let limited = site.send(request.body(Body::empty()).unwrap()).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(header(&limited, "cache-control"), Some("no-store"));
}

/// At `record` (as at `off`) people and apps see nothing of the feature: My
/// TINs, the home page and the download page are byte for byte what they
/// are with the uptime module switched off, though the stored sign says
/// Offline, and nothing in them or their headers names it.
#[sqlx::test]
async fn at_record_the_pages_are_those_of_off(db: PgPool) {
    let record = site(&db, Level::Record);
    sign_in(&record, Light::Offline).await;
    let off = site(&db, Level::Off);
    let (account, cookie) = record.person(FILER).await;
    add_claim(&db, account, ROOT, "approved").await;
    let request = |host: &str, path: &str, cookie: Option<&str>| {
        let mut request = Request::get(path).header("host", host);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.body(Body::empty()).unwrap()
    };
    for (host, path, cookie) in [
        (common::ACCOUNT_HOST, "/tins", Some(cookie.as_str())),
        (FORMS_HOST, "/", None),
        (FORMS_HOST, "/download", None),
        (FORMS_HOST, "/bir-status", None),
        ("buwiz.com", "/", None),
    ] {
        let at_off = snapshot(off.send(request(host, path, cookie)).await).await;
        let at_record = snapshot(record.send(request(host, path, cookie)).await).await;
        assert_eq!(at_record, at_off, "{host}{path}");
        let (_, headers, body) = at_record;
        for (name, value) in &headers {
            assert!(!name.contains("bir") && !value.contains("bir-status"), "{host}{path}: {name}: {value}");
        }
        for trace in ["class=\"bir", " bir-", "/bir-status", "bir-status.css", "buwiz-bir", "BIR status"] {
            assert!(!body.contains(trace), "{host}{path}: {trace}");
        }
    }
}
