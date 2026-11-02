//! The filing site against Postgres: the installers, the download and
//! home pages, and the routes on its own host.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

mod common;

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use buwiz_cloud::sites::downloads::{self, Arch, Kind, Platform, Total};
use serde_json::Value;
use sqlx::PgPool;

use common::*;

/// A stand-in for the `buwiz-releases` bucket: the root `/latest.json` and
/// the app's own `/ebirforms/latest.json` are whatever the test sets, or a
/// 404 while unset.
#[derive(Clone, Default)]
struct FakeReleases {
    root: Arc<Mutex<Option<Vec<u8>>>>,
    own: Arc<Mutex<Option<Vec<u8>>>>,
}

impl FakeReleases {
    /// Serves the bucket on a free port; returns it and its URL.
    async fn start() -> (Self, String) {
        let releases = Self::default();
        let serve = |shared: Arc<Mutex<Option<Vec<u8>>>>| {
            axum::routing::get(move || {
                let latest = shared.lock().unwrap().clone();
                async move {
                    match latest {
                        Some(bytes) => bytes.into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            })
        };
        let app = axum::Router::new()
            .route("/latest.json", serve(releases.root.clone()))
            .route("/ebirforms/latest.json", serve(releases.own.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (releases, url)
    }

    /// The root `latest.json`, as before the bucket had a folder per app.
    fn set_latest(&self, latest: &[u8]) {
        *self.root.lock().unwrap() = Some(latest.to_vec());
    }

    /// The app's own `ebirforms/latest.json`.
    fn set_own_latest(&self, latest: &[u8]) {
        *self.own.lock().unwrap() = Some(latest.to_vec());
    }
}

const SHA_MAC: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SHA_DEB: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const SHA_RPM: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// One file of a manifest.
fn release_file(platform: &str, arch: &str, kind: &str, file: &str, size: u64, sha: &str) -> Value {
    serde_json::json!({"platform": platform, "arch": arch, "kind": kind, "file": file, "size": size, "sha256": sha})
}

/// Version 1.2.0 as the release CI (eBIRForms) writes it, named as it names
/// them, with what the first release had: no Android file, one Linux
/// file, and an iOS entry this server does not know, plus `extra` files.
fn latest_json(extra: &[Value]) -> Vec<u8> {
    let other = "0".repeat(64);
    let mut files = vec![
        release_file("macos", "universal", "dmg", "Buwiz-1.2.0-macos-universal.dmg", 84_213_000, SHA_MAC),
        release_file("windows", "x64", "exe", "Buwiz-1.2.0-windows-x64-setup.exe", 60_000_000, &other),
        release_file("linux", "x64", "deb", "buwiz_1.2.0_amd64.deb", 55_000_000, SHA_DEB),
        release_file("ios", "arm64", "ipa", "Buwiz.ipa", 1, &other),
    ];
    files.extend_from_slice(extra);
    serde_json::json!({"version": "1.2.0", "published_at": "2026-11-02T01:00:00Z", "files": files})
        .to_string()
        .into_bytes()
}

/// The rest of what the CI uploads: the MSI, ARM64 debs, rpms and tarballs
/// for both processors, the Linux install script and the signed APK.
fn later_files() -> Vec<Value> {
    let other = "0".repeat(64);
    vec![
        release_file("windows", "x64", "msi", "Buwiz-1.2.0-windows-x64.msi", 62_000_000, &other),
        release_file("linux", "arm64", "deb", "buwiz_1.2.0_arm64.deb", 54_000_000, &other),
        release_file("linux", "x64", "rpm", "Buwiz-1.2.0-linux-x64.rpm", 56_000_000, SHA_RPM),
        release_file("linux", "arm64", "rpm", "Buwiz-1.2.0-linux-arm64.rpm", 55_500_000, &other),
        release_file("linux", "x64", "targz", "Buwiz-1.2.0-linux-x64.tar.gz", 57_000_000, &other),
        release_file("linux", "arm64", "targz", "Buwiz-1.2.0-linux-arm64.tar.gz", 56_500_000, &other),
        release_file("linux", "universal", "sh", "Buwiz-1.2.0-linux-install.sh", 4_000, &other),
        release_file("android", "arm64", "apk", "Buwiz-1.2.0-android-arm64.apk", 40_000_000, &other),
    ]
}

async fn download_from(site: &Site, path: &str) -> Response {
    site.on_host(FORMS_HOST, path).await
}

/// Asks for `path` on the filing site as a browser does, with `User-Agent`.
async fn browse(site: &Site, path: &str, user_agent: &str) -> Response {
    browse_on(site, FORMS_HOST, path, user_agent).await
}

/// Asks for `path` on `host` as a browser does, with `User-Agent`.
async fn browse_on(site: &Site, host: &str, path: &str, user_agent: &str) -> Response {
    let request = Request::get(path)
        .header(header::HOST, host)
        .header(header::USER_AGENT, user_agent)
        .header(header::ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8");
    site.send(request.body(Body::empty()).unwrap()).await
}

fn cache_control(response: &Response) -> &str {
    response.headers().get(header::CACHE_CONTROL).map_or("", |value| value.to_str().unwrap())
}

/// Asks for each path and checks it redirects, uncached, to `target`.
async fn assert_redirects(site: &Site, cases: &[(&str, String)]) {
    for (path, target) in cases {
        let response = download_from(site, path).await;
        assert_eq!(response.status(), StatusCode::FOUND, "{path}");
        assert_eq!(&location(&response), target, "{path}");
        assert_eq!(cache_control(&response), "no-store", "{path}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn downloads_go_to_the_released_installer_without_caching(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let file = |name: &str| format!("{url}/releases/v1.2.0/{name}");
    assert_redirects(
        &site,
        &[
            ("/download/mac", file("Buwiz-1.2.0-macos-universal.dmg")),
            ("/download/mac?arch=arm64", file("Buwiz-1.2.0-macos-universal.dmg")),
            ("/download/windows", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            ("/download/windows?kind=exe&arch=x64", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            // Linux is the x64 deb unless asked otherwise, and anything it
            // does not know is too.
            ("/download/linux", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=deb&arch=x64", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=nonsense&arch=nonsense", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=msi", file("buwiz_1.2.0_amd64.deb")),
            // No such file: the platform on the download page, with the
            // choice made there.
            ("/download/linux?kind=rpm&arch=arm64", "/download?kind=rpm&arch=arm64#linux".into()),
            ("/download/linux?arch=arm64", "/download?kind=deb&arch=arm64#linux".into()),
            ("/download/linux?kind=appimage", "/download?kind=appimage&arch=x64#linux".into()),
            ("/download/windows?kind=msi", "/download#windows".into()),
            ("/download/android", "/download#android".into()),
            ("/download/mac?kind=nonsense", "/download#mac".into()),
        ],
    )
    .await;
    for path in ["/download/ios", "/download/macos", "/download/mac/extra"] {
        assert_eq!(download_from(&site, path).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    for host in ["account.buwiz.com", "api.buwiz.com"] {
        for path in ["/download/mac", "/download", "/install-linux.sh"] {
            assert_eq!(site.on_host(host, path).await.status(), StatusCode::NOT_FOUND, "{host}{path}");
        }
    }
    // No install script yet: a browser goes to the Linux downloads, and
    // curl gets a 404 rather than a web page to run.
    let script = browse(&site, "/install-linux.sh", "Mozilla/5.0 (X11; Linux x86_64)").await;
    assert_eq!((script.status(), location(&script)), (StatusCode::FOUND, "/download#linux".to_owned()));
    let curl = site
        .send(
            Request::get("/install-linux.sh")
                .header(header::HOST, FORMS_HOST)
                .header(header::ACCEPT, "*/*")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(curl.status(), StatusCode::NOT_FOUND);
    assert_eq!(cache_control(&curl), "no-store");

    // With every file the CI makes, each kind and processor is a query
    // away; Windows still defaults to the installer, not the MSI.
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db, &url);
    assert_redirects(
        &site,
        &[
            ("/download/windows", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            ("/download/windows?kind=msi", file("Buwiz-1.2.0-windows-x64.msi")),
            ("/download/linux", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=deb&arch=arm64", file("buwiz_1.2.0_arm64.deb")),
            ("/download/linux?kind=rpm", file("Buwiz-1.2.0-linux-x64.rpm")),
            ("/download/linux?kind=rpm&arch=x64", file("Buwiz-1.2.0-linux-x64.rpm")),
            ("/download/linux?kind=rpm&arch=arm64", file("Buwiz-1.2.0-linux-arm64.rpm")),
            ("/download/linux?kind=targz&arch=x64", file("Buwiz-1.2.0-linux-x64.tar.gz")),
            ("/download/linux?kind=targz&arch=arm64", file("Buwiz-1.2.0-linux-arm64.tar.gz")),
            ("/download/android", file("Buwiz-1.2.0-android-arm64.apk")),
            ("/install-linux.sh", file("Buwiz-1.2.0-linux-install.sh")),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn downloads_are_counted_per_file_and_day_without_crawlers(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let fetch = |method: Method, path: &str, agent: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, FORMS_HOST)
            .header(header::USER_AGENT, agent)
            .body(Body::empty())
            .unwrap()
    };
    let person = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
    for (method, path, agent) in [
        (Method::GET, "/download/mac", person),
        (Method::GET, "/download/mac?kind=dmg", person),
        (Method::GET, "/download/linux?kind=rpm&arch=arm64", person),
        (Method::GET, "/install-linux.sh", "curl/8.7.1"),
        // Not counted: crawlers and link previews, no User-Agent, a HEAD.
        (Method::GET, "/download/mac", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"),
        (Method::GET, "/download/mac", "Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)"),
        (Method::GET, "/download/mac", ""),
        (Method::HEAD, "/download/windows", person),
    ] {
        assert_eq!(site.send(fetch(method, path, agent)).await.status(), StatusCode::FOUND, "{path} {agent}");
    }
    let total = |platform, arch, kind, count| Total { platform, arch, kind, count };
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        vec![
            total(Platform::Mac, Arch::Universal, Kind::Dmg, 2),
            total(Platform::Linux, Arch::Arm64, Kind::Rpm, 1),
            total(Platform::Linux, Arch::Universal, Kind::Sh, 1),
        ]
    );
    // Kept per version and Manila day, and nothing about who asked.
    let rows: Vec<(String, String, String, String, i64, bool)> = sqlx::query_as(
        "SELECT version, platform, arch, kind, count, day = (now() AT TIME ZONE 'Asia/Manila')::date
         FROM download_counts ORDER BY platform, kind",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let row = |platform: &str, arch: &str, kind: &str, count| {
        ("1.2.0".to_owned(), platform.to_owned(), arch.to_owned(), kind.to_owned(), count, true)
    };
    assert_eq!(
        rows,
        vec![row("linux", "arm64", "rpm", 1), row("linux", "universal", "sh", 1), row("macos", "universal", "dmg", 2)]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ebirforms_reads_its_own_latest_json_or_else_the_root_one_and_counts_both_as_its_own(db: PgPool) {
    let host = FORMS_HOST;
    let (releases, url) = FakeReleases::start().await;
    // Only the root one, as before the bucket had a folder per app.
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let response = browse_on(&site, host, "/download/mac", MAC).await;
    assert_eq!(location(&response), format!("{url}/releases/v1.2.0/Buwiz-1.2.0-macos-universal.dmg"));
    assert!(text(browse_on(&site, host, "/download", MAC).await).await.contains("Version 1.2.0,"));

    // Its own one: read first, with the files in its folder.
    let own = String::from_utf8(latest_json(&[])).unwrap().replace("1.2.0", "1.3.0");
    releases.set_own_latest(own.as_bytes());
    let site = Site::with_downloads(db.clone(), &url);
    let response = browse_on(&site, host, "/download/mac", MAC).await;
    assert_eq!(location(&response), format!("{url}/ebirforms/releases/v1.3.0/Buwiz-1.3.0-macos-universal.dmg"));
    let page = text(browse_on(&site, host, "/download", MAC).await).await;
    assert!(page.contains("Version 1.3.0,") && !page.contains("1.2.0"), "{page}");
    assert!(!page.contains(&url), "never straight to the bucket");

    // One Download button per device: one per card, the visitor's marked once.
    let cards = platform_cards(&page);
    assert_eq!(cards.len(), 5);
    assert_eq!(cards.iter().filter(|card| card.starts_with(" yours\"")).count(), 1);
    for name in ["macOS", "Windows", "Linux"] {
        let card = card(&cards, name);
        assert_eq!(card.matches("btn-solid").count(), 1, "{card}");
        assert_eq!(card.matches("Download for").count(), 1, "{card}");
    }
    for name in ["Android", "iOS"] {
        assert!(!card(&cards, name).contains("btn-solid"), "{name}");
    }
    // The home page: one Download button for the device, nowhere else.
    for (agent, href) in [(MAC, "/download/mac"), (WINDOWS, "/download/windows"), (LINUX, "/download/linux")] {
        let home = text(browse_on(&site, host, "/", agent).await).await;
        assert_eq!(home.matches("btn-get").count(), 1, "{agent}");
        assert_eq!(home.matches("Download for").count(), 1, "{agent}");
        assert_eq!(home.matches(r#"href="/download/"#).count(), 1, "{agent}");
        assert!(home.contains(&format!(r#"href="{href}""#)), "{agent}");
        assert_eq!(home.matches(r#"href="/download""#).count(), 1, "Other platforms, once: {agent}");
    }

    // Counted as eBIRForms' downloads, per version.
    let fallback = Site::with_downloads(db.clone(), &url);
    *releases.own.lock().unwrap() = None;
    assert_eq!(browse_on(&fallback, host, "/download/mac", MAC).await.status(), StatusCode::FOUND);
    let total = |platform, arch, kind, count| Total { platform, arch, kind, count };
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        vec![total(Platform::Mac, Arch::Universal, Kind::Dmg, 3)]
    );
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT app, version, count FROM download_counts ORDER BY version")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(rows, vec![("ebirforms".into(), "1.2.0".into(), 2), ("ebirforms".into(), "1.3.0".into(), 1)]);
}

/// The download page's platform cards, each up to its end.
fn platform_cards(page: &str) -> Vec<&str> {
    page.split("<li class=\"panel dl-card").skip(1).map(|card| card.split("</li></ul>").next().unwrap()).collect()
}

fn card<'a>(cards: &[&'a str], name: &str) -> &'a str {
    cards.iter().find(|card| card.contains(&format!("<h2>{name}</h2>"))).unwrap()
}

/// The home page hero's download, up to the platform logos.
fn hero_downloads(home: &str) -> &str {
    let hero = &home[home.find(r#"<section id="hero""#).unwrap()..];
    &hero[..hero.find(r#"<div class="platforms"#).unwrap()]
}

/// The home page hero's row of platform logos.
fn platform_row(home: &str) -> &str {
    let row = &home[home.find(r#"<div class="platforms"#).unwrap()..];
    &row[..row.find("</div>").unwrap()]
}

/// The Linux install command's row: selectable text, and a Copy button the
/// script shows where it can copy.
const INSTALL_LINE: &str = r#"<div class="install-line" data-copy-group=""><code>curl -fsSL https://ebirforms.buwiz.com/install-linux.sh | sh</code><button type="button" data-copy="curl -fsSL https://ebirforms.buwiz.com/install-linux.sh | sh" hidden="">Copy</button><span class="copy-status" role="status"></span></div>"#;

const MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
const WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36";
const LINUX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const ANDROID: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Mobile Safari/537.36";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148";
const CHROMEBOOK: &str = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36";

#[sqlx::test(migrations = "./migrations")]
async fn the_download_page_lists_the_release_and_highlights_the_visitors_platform(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", MAC).await).await;
    assert!(page.contains("<title>Download Buwiz for Windows, macOS, Linux and Android</title>"), "{page}");
    assert!(page.contains(r#"<link rel="canonical" href="https://ebirforms.buwiz.com/download">"#));
    assert!(!page.contains("noindex"));
    assert!(page.contains("Version 1.2.0, released November 2, 2026."));
    // One card per platform, the Mac one marked as the visitor's, each with
    // one button through the counted route, its version and size, and its
    // SHA-256 in small print.
    let cards = platform_cards(&page);
    assert_eq!(cards.len(), 5);
    assert_eq!(cards.iter().filter(|card| card.starts_with(" yours\"")).count(), 1);
    let mac = card(&cards, "macOS");
    assert!(mac.starts_with(r#" yours" id="mac">"#), "{mac}");
    assert!(mac.contains(r#"<span class="dl-yours">Your device</span>"#), "{mac}");
    for (name, href, label, facts, sha) in [
        ("macOS", "/download/mac", "Download for Mac", "Version 1.2.0 · .dmg · 84.2 MB", SHA_MAC),
        (
            "Windows",
            "/download/windows",
            "Download for Windows",
            "Version 1.2.0 · .exe · x64 · 60.0 MB",
            &"0".repeat(64),
        ),
        ("Linux", "/download/linux", "Download for Linux", "Version 1.2.0 · .deb · x64 · 55.0 MB", SHA_DEB),
    ] {
        let card = card(&cards, name);
        assert_eq!(card.matches("<a ").count(), 1, "one link: {card}");
        assert!(card.contains(&format!(r#"<a class="btn btn-solid" href="{href}">"#)), "{card}");
        assert!(card.contains(&format!("</span>{label}</a>")), "{card}");
        assert!(card.contains(&format!(r#"<p class="dl-facts">{facts}</p>"#)), "{card}");
        assert!(card.contains(&format!("<code>{sha}</code>")), "{card}");
    }
    // Android has no file yet; iOS will come from the App Store.
    let android = card(&cards, "Android");
    assert!(
        android.contains(r#"<span class="soon-tag">Coming soon</span>"#) && !android.contains("href="),
        "{android}"
    );
    let ios = card(&cards, "iOS");
    assert!(ios.contains("Coming to the App Store") && !ios.contains("href=") && !ios.contains("btn"), "{ios}");
    assert!(!page.contains("Buwiz.ipa") && !page.contains("/download/android"));
    assert!(!page.contains(&url), "never straight to the bucket");
    for gone in
        ["<form", "<fieldset", r#"type="radio""#, "dl-terminal", "data-copy", "<script src=\"/assets/download.js"]
    {
        assert!(!page.contains(gone), "{gone}");
    }

    let sitemap = text(site.on_host(FORMS_HOST, "/sitemap.xml").await).await;
    assert!(
        sitemap.contains("<loc>https://ebirforms.buwiz.com/download</loc>\n    <lastmod>2026-11-02</lastmod>"),
        "{sitemap}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn the_home_page_has_one_download_button_for_the_visitors_device(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let button = |href: &str, label: &str| {
        format!(
            r#"<a class="btn btn-solid btn-get" href="{href}"><span class="icon" aria-hidden="true">download</span>{label}</a>"#
        )
    };
    let other = r#"<a class="get-other" href="/download">Other platforms</a>"#;
    let check = async |site: &Site, agent: &str, href: &str, label: &str| {
        let home = text(browse(site, "/", agent).await).await;
        let hero = hero_downloads(&home);
        assert!(hero.contains(&button(href, label)), "{agent}: {hero}");
        assert_eq!(hero.matches("<a ").count(), if href == "/download" { 1 } else { 2 }, "{agent}: {hero}");
        assert_eq!(hero.contains(other), href != "/download", "{agent}: {hero}");
        // The header has the account; the hero has none of it.
        // No install script in this release, so no command to copy either.
        for gone in
            ["/signup", "/login", "/tins", "Other downloads", "Not Ubuntu", "Chromebook", "data-copy", "<button"]
        {
            assert!(!hero.contains(gone), "{agent}: {gone}: {hero}");
        }
        // The logos only show where Buwiz runs: no links, nothing to click.
        let row = platform_row(&home);
        assert!(!row.contains("<a ") && !row.contains("href="), "{agent}: {row}");
        assert!(row.contains(r#"<li class="platform" title="Windows">"#), "{row}");
        assert!(row.contains(r#"<li class="platform soon" title="Android, coming soon">"#), "{row}");
        assert!(
            row.contains(
                r#"<p class="visually-hidden">Available for Windows, macOS and Linux; Android and iOS coming soon</p>"#
            ),
            "{row}"
        );
        assert!(!home.contains("download.js") && !home.contains("alert(") && !home.contains("confirm("), "{agent}");
    };
    check(&site, MAC, "/download/mac", "Download for Mac").await;
    check(&site, WINDOWS, "/download/windows", "Download for Windows").await;
    check(&site, LINUX, "/download/linux", "Download for Linux").await;
    // No file of their own: the download page.
    for agent in [IPHONE, CHROMEBOOK, ANDROID, "curl/8.7.1"] {
        check(&site, agent, "/download", "Download").await;
    }

    // With an APK out, Android gets its button.
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let home = text(browse(&site, "/", ANDROID).await).await;
    assert!(hero_downloads(&home).contains(&button("/download/android", "Download for Android")), "{home}");
    assert!(platform_row(&home).contains("Available for Windows, macOS, Linux and Android; iOS coming soon"));
    assert!(!home.contains("install-line"), "Linux only");
    // With the install script out, a Linux visitor gets its command to copy
    // under the button; nobody else does.
    let home = text(browse(&site, "/", LINUX).await).await;
    let hero = hero_downloads(&home);
    assert!(hero.contains(&button("/download/linux", "Download for Linux")), "{hero}");
    assert!(hero.contains(INSTALL_LINE), "{hero}");
    for agent in [MAC, WINDOWS, CHROMEBOOK] {
        assert!(!text(browse(&site, "/", agent).await).await.contains("install-line"), "{agent}");
    }

    // Signed in, the hero still has just the download; the header has My TINs.
    let (_, cookie) = site.person("owner@example.com").await;
    let request = Request::get("/")
        .header(header::HOST, FORMS_HOST)
        .header(header::USER_AGENT, MAC)
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let home = text(site.send(request).await).await;
    let hero = hero_downloads(&home);
    assert!(!hero.contains("/tins") && hero.matches("<a ").count() == 2, "{hero}");
    let header = &home[..home.find("<main").unwrap()];
    assert!(header.contains(r#"<a href="/tins" class="">My TINs</a>"#), "{header}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_download_page_lists_other_files_as_plain_links(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", LINUX).await).await;
    let cards = platform_cards(&page);

    // Linux: the x64 deb is the button; every other file a small link
    // through this site, which counts it. No choices, no terminal.
    let linux = card(&cards, "Linux");
    assert!(linux.starts_with(r#" yours" id="linux">"#), "{linux}");
    assert!(linux.contains(r#"<a class="btn btn-solid" href="/download/linux">"#), "{linux}");
    assert_eq!(linux.matches("btn-solid").count(), 1, "{linux}");
    for (label, route) in [
        (".deb · ARM64", "/download/linux?kind=deb&amp;arch=arm64"),
        (".rpm · x64", "/download/linux?kind=rpm&amp;arch=x64"),
        (".rpm · ARM64", "/download/linux?kind=rpm&amp;arch=arm64"),
        (".tar.gz · x64", "/download/linux?kind=targz&amp;arch=x64"),
        (".tar.gz · ARM64", "/download/linux?kind=targz&amp;arch=arm64"),
        ("Install script", "/install-linux.sh"),
    ] {
        assert!(linux.contains(&format!(r#"<a href="{route}">{label}</a>"#)), "{label}: {linux}");
    }
    assert!(!linux.contains("/download/linux?kind=deb&amp;arch=x64"), "the button's file is not listed again");
    for gone in ["<form", "<fieldset", r#"type="radio""#, "dl-terminal"] {
        assert!(!linux.contains(gone), "{gone}: {linux}");
    }
    // The install script's command, on one line, to copy.
    assert!(linux.contains(INSTALL_LINE), "{linux}");
    assert_eq!(page.matches("install-line").count(), 1, "Linux only: {page}");

    // Windows: the installer is the button, the MSI a small link.
    let windows = card(&cards, "Windows");
    assert!(windows.contains(r#"<a class="btn btn-solid" href="/download/windows">"#), "{windows}");
    assert!(
        windows.contains(r#"<a href="/download/windows?kind=msi&amp;arch=x64">.msi (for IT) · x64</a>"#),
        "{windows}"
    );
    let android = card(&cards, "Android");
    assert!(android.contains(r#"<a class="btn btn-solid" href="/download/android">"#), "{android}");
    assert!(!page.contains(&url), "never straight to the bucket");

    // A Linux release without an x64 deb: the button is its best file.
    let other = "0".repeat(64);
    let only_rpm = serde_json::json!({
        "version": "1.2.0",
        "files": [release_file("linux", "x64", "rpm", "Buwiz-1.2.0-linux-x64.rpm", 1, &other)],
    });
    releases.set_latest(only_rpm.to_string().as_bytes());
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", LINUX).await).await;
    let linux = card(&platform_cards(&page), "Linux");
    assert!(linux.contains(r#"<a class="btn btn-solid" href="/download/linux?kind=rpm&amp;arch=x64">"#), "{linux}");
    assert!(!linux.contains("install-line"), "no script released: {linux}");
    // And the home page sends a Linux visitor to the download page.
    let home = text(browse(&site, "/", LINUX).await).await;
    assert!(hero_downloads(&home).contains(r#"<a class="btn btn-solid btn-get" href="/download">"#), "{home}");
}

/// What every page says while nothing is released.
async fn assert_coming_soon(site: &Site, why: &str) {
    let response = download_from(site, "/download/windows").await;
    assert_eq!(response.status(), StatusCode::FOUND, "{why}");
    assert_eq!(location(&response), "/download", "{why}");
    assert_eq!(cache_control(&response), "no-store", "{why}");
    let script = browse(site, "/install-linux.sh", LINUX).await;
    assert_eq!(location(&script), "/download#linux", "{why}");

    let page = download_from(site, "/download").await;
    assert_eq!(page.status(), StatusCode::OK, "{why}");
    let page = text(page).await;
    assert!(page.contains("Buwiz is coming soon to Windows, macOS, Linux and Android"), "{why}");
    assert_eq!(page.matches(r#"<span class="soon-tag">Coming soon</span>"#).count(), 4, "{why}");
    assert!(page.contains("Coming to the App Store"), "{why}");
    assert!(!page.contains("href=\"/download/") && !page.contains("releases/v"), "{why}");
    assert!(page.contains(r#"<meta name="robots" content="noindex">"#), "{why}");

    let home = text(site.on_host(FORMS_HOST, "/").await).await;
    assert!(
        home.contains(r#"<div class="platforms rise d3"><span class="soon-tag">Coming soon</span><p class="visually-hidden">Coming soon to Windows, macOS, Linux, Android and iOS</p><ul aria-hidden="true"><li class="platform" title="Windows, coming soon"><svg"#),
        "{why}"
    );
    assert!(!home.contains("/download") && !home.contains("btn-get"), "{why}");

    let sitemap = text(site.on_host(FORMS_HOST, "/sitemap.xml").await).await;
    assert!(!sitemap.contains("/download"), "{why}");
}

#[sqlx::test(migrations = "./migrations")]
async fn nothing_is_released_until_latest_json_exists(db: PgPool) {
    let (_releases, url) = FakeReleases::start().await;
    assert_coming_soon(&Site::with_downloads(db, &url), "no latest.json").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_broken_or_unreachable_manifest_means_coming_soon(db: PgPool) {
    assert_coming_soon(&Site::new(db.clone()), "unreachable").await;
    let sha = "0".repeat(64);
    let path_trick = serde_json::json!({
        "version": "1.2.0",
        "files": [{"platform": "windows", "arch": "x64", "kind": "msi", "file": "../../evil.msi", "size": 1, "sha256": sha}],
    })
    .to_string();
    let bad_version = String::from_utf8(latest_json(&[])).unwrap().replace("\"1.2.0\"", "\"1.2.0/../..\"");
    for (latest, why) in
        [("{\"version\":", "broken JSON"), (path_trick.as_str(), "path trick"), (bad_version.as_str(), "bad version")]
    {
        let (releases, url) = FakeReleases::start().await;
        releases.set_latest(latest.as_bytes());
        assert_coming_soon(&Site::with_downloads(db.clone(), &url), why).await;
    }
}

/// What each of the filing site's routes answers on `host`: (path, status,
/// `Location`).
async fn answers(site: &Site, host: &str) -> Vec<(&'static str, StatusCode, String)> {
    let mut answers = Vec::new();
    for path in [
        "/",
        "/download",
        "/download/mac?arch=arm64",
        "/install-linux.sh",
        "/assets/demo.js",
        "/assets/screens/palette_dark.webp",
        "/assets/site.css",
        "/login",
        "/signup",
        "/tins",
        "/account",
        "/robots.txt",
        "/sitemap.xml",
        "/site.webmanifest",
    ] {
        let response = site.on_host(host, path).await;
        answers.push((path, response.status(), location(&response)));
    }
    answers
}

#[sqlx::test(migrations = "./migrations")]
async fn the_filing_site_answers_on_its_own_host(db: PgPool) {
    let site = Site::new(db);
    let ok = |path| (path, StatusCode::OK, String::new());
    let expected = vec![
        ok("/"),
        ok("/download"),
        ("/download/mac?arch=arm64", StatusCode::FOUND, "/download".to_owned()),
        ("/install-linux.sh", StatusCode::NOT_FOUND, String::new()),
        ok("/assets/demo.js"),
        ok("/assets/screens/palette_dark.webp"),
        ok("/assets/site.css"),
        ok("/login"),
        ok("/signup"),
        ("/tins", StatusCode::TEMPORARY_REDIRECT, "/login?next=%2Ftins&notice=sign_in_first".to_owned()),
        ("/account", StatusCode::PERMANENT_REDIRECT, "/tins".to_owned()),
        ok("/robots.txt"),
        ok("/sitemap.xml"),
        ok("/site.webmanifest"),
    ];
    for host in ["forms.buwiz.com", "ebirforms.buwiz.com"] {
        assert_eq!(answers(&site, host).await, expected, "{host}");
        let home = text(site.on_host(host, "/").await).await;
        assert!(home.contains("<title>Buwiz: Philippine tax filing, made modern</title>"), "{host}");
        assert!(!home.contains("Buwiz Bot"), "{host}");
        assert!(home.contains(r#"<link rel="canonical" href="https://ebirforms.buwiz.com/">"#), "{host}");
        assert!(
            home.contains(
                r#"<meta property="og:image" content="https://ebirforms.buwiz.com/assets/brand/og-image.png">"#
            ),
            "{host}"
        );
    }
    let robots = text(site.on_host("ebirforms.buwiz.com", "/robots.txt").await).await;
    assert!(robots.ends_with("Sitemap: https://ebirforms.buwiz.com/sitemap.xml\n"), "{robots}");
    let sitemap = text(site.on_host("ebirforms.buwiz.com", "/sitemap.xml").await).await;
    assert!(sitemap.contains("<loc>https://ebirforms.buwiz.com/</loc>"), "{sitemap}");
    // The account and API hosts are as they were.
    assert_eq!(location(&site.on_host("account.buwiz.com", "/").await), "/account");
    assert_eq!(site.on_host("account.buwiz.com", "/login").await.status(), StatusCode::OK);
    assert_eq!(site.on_host("api.buwiz.com", "/").await.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn sign_in_links_go_to_the_account_host(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.on_host(FORMS_HOST, "/").await).await;
    let header = home.split("<header").nth(1).unwrap().split("</header>").next().unwrap();
    assert!(header.contains(r#"<a class="btn btn-ghost" href="http://localhost:3008/login">Sign in</a>"#), "{header}");
    assert!(header.contains(r#"<a class="btn btn-solid" href="http://localhost:3008/signup">"#), "{header}");
    let account = home.split(r#"<section id="account""#).nth(1).unwrap().split("</section>").next().unwrap();
    assert!(account.contains(r#"href="http://localhost:3008/signup""#), "{account}");
    assert!(account.contains(r#"href="http://localhost:3008/login""#), "{account}");
    assert!(!account.contains(r#"href="/login""#) && !account.contains(r#"href="/signup""#), "{account}");
    // The sign-in pages' own switch links stay on their host.
    let login = text(site.get("/login", "").await).await;
    let card = login.split("</header>").nth(1).unwrap();
    assert!(card.contains(r#"href="/signup"#), "{card}");
}

#[sqlx::test(migrations = "./migrations")]
async fn download_counts_are_kept_per_app(db: PgPool) {
    let app: String =
        sqlx::query_scalar("SELECT column_default FROM information_schema.columns WHERE table_name = 'download_counts' AND column_name = 'app'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(app, "'ebirforms'::text");
    let key: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname::text FROM pg_index i
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
         WHERE i.indrelid = 'download_counts'::regclass AND i.indisprimary
         ORDER BY array_position(i.indkey, a.attnum)",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(key, ["app", "day", "version", "platform", "arch", "kind"]);

    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let person = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
    let request = Request::get("/download/mac").header(header::HOST, FORMS_HOST).header(header::USER_AGENT, person);
    assert_eq!(site.send(request.body(Body::empty()).unwrap()).await.status(), StatusCode::FOUND);
    // Another app's count of the same file and day is its own row, and not
    // in this app's totals.
    sqlx::query(
        "INSERT INTO download_counts (app, day, version, platform, arch, kind, count)
         SELECT 'bot', day, version, platform, arch, kind, 5 FROM download_counts",
    )
    .execute(&db)
    .await
    .unwrap();
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT app, count FROM download_counts ORDER BY app").fetch_all(&db).await.unwrap();
    assert_eq!(rows, [("bot".to_owned(), 5), ("ebirforms".to_owned(), 1)]);
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        [Total { platform: Platform::Mac, arch: Arch::Universal, kind: Kind::Dmg, count: 1 }]
    );
}
