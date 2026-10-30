//! The Buwiz app's installers, in the R2 bucket `buwiz-releases` served at
//! `DOWNLOADS_URL`. The release CI (buwiz-oss) uploads
//! `releases/v{version}/{file}` and that version's `manifest.json`; the
//! owner releases a version by copying its manifest to `latest.json` at the
//! root. Until `latest.json` exists nothing is released, and every download
//! link on the site says "coming soon".

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// How long a fetched `latest.json` is used before asking again.
const REFRESH: Duration = Duration::from_secs(60);

/// The bucket may be unreachable (its domain not connected yet); pages that
/// wait for it should not wait long.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Where the app has installers, as named in `/download/{platform}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
    Android,
}

impl Platform {
    pub const ALL: [Platform; 4] = [Platform::Mac, Platform::Windows, Platform::Linux, Platform::Android];

    /// The path segment of `/download/{platform}`.
    pub fn slug(self) -> &'static str {
        match self {
            Platform::Mac => "mac",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Android => "android",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|platform| platform.slug() == slug)
    }

    /// The manifest's name for it.
    fn from_manifest(name: &str) -> Option<Self> {
        match name {
            "macos" => Some(Platform::Mac),
            "windows" => Some(Platform::Windows),
            "linux" => Some(Platform::Linux),
            "android" => Some(Platform::Android),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X64,
    Universal,
}

impl Arch {
    const ALL: [Arch; 3] = [Arch::Arm64, Arch::X64, Arch::Universal];

    /// The manifest's name for it; also `?arch=` of `/download/{platform}`.
    pub fn name(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X64 => "x64",
            Arch::Universal => "universal",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arch| arch.name() == name)
    }

    /// What people call it on `platform`.
    pub fn label(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (Arch::Universal, Platform::Mac) => "Apple silicon and Intel",
            (Arch::Arm64, Platform::Mac) => "Apple silicon",
            (Arch::X64, Platform::Mac) => "Intel",
            (Arch::Universal, _) => "All processors",
            (Arch::Arm64, _) => "ARM64",
            (Arch::X64, _) => "x64",
        }
    }
}

/// The kind of installer; also `?kind=` of `/download/{platform}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dmg,
    Msi,
    Exe,
    AppImage,
    Deb,
    Rpm,
    /// A `.tar.gz` to unpack, for any other Linux.
    TarGz,
    /// The Linux install script, `curl … | sh`; always `universal`.
    Sh,
    Apk,
}

impl Kind {
    const ALL: [Kind; 9] =
        [Kind::Dmg, Kind::Msi, Kind::Exe, Kind::AppImage, Kind::Deb, Kind::Rpm, Kind::TarGz, Kind::Sh, Kind::Apk];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Dmg => "dmg",
            Kind::Msi => "msi",
            Kind::Exe => "exe",
            Kind::AppImage => "appimage",
            Kind::Deb => "deb",
            Kind::Rpm => "rpm",
            Kind::TarGz => "targz",
            Kind::Sh => "sh",
            Kind::Apk => "apk",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Dmg => "Disk image (.dmg)",
            Kind::Msi => "MSI package (.msi)",
            Kind::Exe => "Installer (.exe)",
            Kind::AppImage => "AppImage",
            Kind::Deb => "Ubuntu, Debian (.deb)",
            Kind::Rpm => "Fedora, Red Hat, openSUSE (.rpm)",
            Kind::TarGz => "Any Linux (.tar.gz)",
            Kind::Sh => "Install script (.sh)",
            Kind::Apk => "APK",
        }
    }

    /// The one platform this kind installs on.
    pub fn platform(self) -> Platform {
        match self {
            Kind::Dmg => Platform::Mac,
            Kind::Msi | Kind::Exe => Platform::Windows,
            Kind::AppImage | Kind::Deb | Kind::Rpm | Kind::TarGz | Kind::Sh => Platform::Linux,
            Kind::Apk => Platform::Android,
        }
    }
}

/// The released version, from `latest.json`.
#[derive(Debug)]
pub struct Release {
    /// `MAJOR.MINOR.PATCH`.
    pub version: String,
    pub published_at: Option<OffsetDateTime>,
    /// At least one, in the manifest's order.
    pub files: Vec<File>,
}

#[derive(Clone, Debug)]
pub struct File {
    pub platform: Platform,
    pub arch: Arch,
    pub kind: Kind,
    /// A plain file name under `releases/v{version}/`.
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// `{DOWNLOADS_URL}/releases/v{version}/{name}`.
    pub url: String,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    published_at: Option<String>,
    files: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Entry {
    platform: String,
    arch: String,
    kind: String,
    file: String,
    size: u64,
    sha256: String,
}

impl Release {
    /// Reads a manifest whose files are under `{base}/releases/v{version}/`.
    /// Files on platforms, processors or kinds this server does not know
    /// are left out, so a newer CI can add them; so are files whose name
    /// would point anywhere else.
    pub fn parse(json: &[u8], base: &str) -> anyhow::Result<Self> {
        let manifest: Manifest = serde_json::from_slice(json).context("latest.json is not a manifest")?;
        if !is_version(&manifest.version) {
            bail!("version `{}` is not MAJOR.MINOR.PATCH", manifest.version);
        }
        let version = manifest.version;
        let files: Vec<File> = manifest
            .files
            .into_iter()
            .filter_map(|value| serde_json::from_value::<Entry>(value).ok())
            .filter_map(|entry| {
                let platform = Platform::from_manifest(&entry.platform)?;
                let arch = Arch::from_name(&entry.arch)?;
                let kind = Kind::from_name(&entry.kind).filter(|kind| kind.platform() == platform)?;
                if kind == Kind::Sh && arch != Arch::Universal {
                    return None;
                }
                if !is_file_name(&entry.file) || !is_sha256(&entry.sha256) {
                    tracing::warn!(file = entry.file, "latest.json: left out a file with a bad name or SHA-256");
                    return None;
                }
                Some(File {
                    platform,
                    arch,
                    kind,
                    url: format!("{base}/releases/v{version}/{}", entry.file),
                    name: entry.file,
                    size: entry.size,
                    sha256: entry.sha256.to_ascii_lowercase(),
                })
            })
            .collect();
        if files.is_empty() {
            bail!("latest.json for {version} has no file this server can offer");
        }
        let published_at = manifest.published_at.and_then(|raw| OffsetDateTime::parse(&raw, &Rfc3339).ok());
        Ok(Self { version, published_at, files })
    }

    /// The files for `platform`, in the manifest's order.
    pub fn files_for(&self, platform: Platform) -> impl Iterator<Item = &File> {
        self.files.iter().filter(move |file| file.platform == platform)
    }

    /// The file `/download/{platform}` sends people to: of `kind` and for
    /// processor `arch` (a universal build runs on any) when asked,
    /// otherwise the one that suits most computers. The install script is
    /// only ever given when asked for.
    pub fn pick(&self, platform: Platform, kind: Option<Kind>, arch: Option<Arch>) -> Option<&File> {
        self.files_for(platform)
            .filter(|file| kind.map_or(file.kind != Kind::Sh, |kind| file.kind == kind))
            .filter(|file| arch.is_none_or(|arch| file.arch == arch || file.arch == Arch::Universal))
            .min_by_key(|file| rank(file))
    }
}

/// Lower is better: a Mac gets the universal build, Windows the x64
/// installer (EXE before MSI, which is for IT departments), Linux the x64
/// deb, Android the APK.
fn rank(file: &File) -> (usize, usize) {
    let arch = |order: &[Arch]| order.iter().position(|arch| *arch == file.arch).unwrap_or(order.len());
    let kind = |order: &[Kind]| order.iter().position(|kind| *kind == file.kind).unwrap_or(order.len());
    match file.platform {
        Platform::Mac => (arch(&[Arch::Universal, Arch::Arm64, Arch::X64]), 0),
        Platform::Windows => (arch(&[Arch::X64, Arch::Universal, Arch::Arm64]), kind(&[Kind::Exe, Kind::Msi])),
        Platform::Linux => (
            kind(&[Kind::Deb, Kind::Rpm, Kind::TarGz, Kind::AppImage]),
            arch(&[Arch::X64, Arch::Universal, Arch::Arm64]),
        ),
        Platform::Android => (arch(&[Arch::Universal, Arch::Arm64, Arch::X64]), 0),
    }
}

/// `MAJOR.MINOR.PATCH`, digits only.
fn is_version(raw: &str) -> bool {
    let parts: Vec<&str> = raw.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| !part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit()))
}

/// A plain file name: letters, digits, `.`, `_`, `-` and `+`, not starting
/// with a dot. No `/`, `\`, `..`, `%`, `:` or `?`, so the URL built from it
/// stays under its version's folder.
fn is_file_name(raw: &str) -> bool {
    (1..=200).contains(&raw.len())
        && !raw.starts_with('.')
        && !raw.contains("..")
        && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
}

fn is_sha256(raw: &str) -> bool {
    raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What the visitor is on, as far as `User-Agent` tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// One the app has installers for.
    Platform(Platform),
    /// An iPhone, iPad or iPod (which say "like Mac OS X"). An iPad asking
    /// for desktop sites says Macintosh; only the page's script can tell.
    Ios,
    /// A Chromebook, which runs the Linux deb once Linux is turned on.
    Chromebook,
}

/// A guess at the visitor's device from `User-Agent`; `None` when it says
/// nothing we know.
pub fn guess_device(user_agent: &str) -> Option<Device> {
    if user_agent.contains("Android") {
        Some(Device::Platform(Platform::Android))
    } else if ["iPhone", "iPad", "iPod"].iter().any(|name| user_agent.contains(name)) {
        Some(Device::Ios)
    } else if user_agent.contains("CrOS") {
        Some(Device::Chromebook)
    } else if user_agent.contains("Windows") {
        Some(Device::Platform(Platform::Windows))
    } else if user_agent.contains("Macintosh") || user_agent.contains("Mac OS X") {
        Some(Device::Platform(Platform::Mac))
    } else if user_agent.contains("Linux") || user_agent.contains("X11") {
        Some(Device::Platform(Platform::Linux))
    } else {
        None
    }
}

/// A guess at the visitor's platform from `User-Agent`, to highlight its
/// download. iPhones, iPads and Chromebooks get `None`.
pub fn guess_platform(user_agent: &str) -> Option<Platform> {
    match guess_device(user_agent) {
        Some(Device::Platform(platform)) => Some(platform),
        _ => None,
    }
}

/// `latest.json`, fetched when asked for and kept for [`REFRESH`].
pub struct Downloads {
    base: String,
    http: reqwest::Client,
    refresh: Duration,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    release: Option<Arc<Release>>,
    checked: Option<Instant>,
}

impl Downloads {
    /// `base` is `DOWNLOADS_URL`, without a trailing slash.
    pub fn new(base: &str) -> Self {
        Self::with_refresh(base, REFRESH)
    }

    fn with_refresh(base: &str, refresh: Duration) -> Self {
        let http = reqwest::Client::builder().timeout(TIMEOUT).build().expect("client");
        Self { base: base.to_owned(), http, refresh, cache: Mutex::default() }
    }

    /// The released version, or `None` while nothing is. A failed refresh
    /// keeps the last good manifest; one request asks at a time, and the
    /// others meanwhile get what is kept.
    pub async fn latest(&self) -> Option<Arc<Release>> {
        let due = {
            let mut cache = self.cache.lock().expect("downloads cache");
            let due = cache.checked.is_none_or(|at| at.elapsed() >= self.refresh);
            if due {
                cache.checked = Some(Instant::now());
            }
            due
        };
        if due {
            match self.fetch().await {
                Ok(release) => self.cache.lock().expect("downloads cache").release = release.map(Arc::new),
                Err(error) => tracing::warn!(error = format!("{error:#}"), "could not read latest.json"),
            }
        }
        self.cache.lock().expect("downloads cache").release.clone()
    }

    /// `Ok(None)` when there is no `latest.json`: nothing is released, or a
    /// release was taken back.
    async fn fetch(&self) -> anyhow::Result<Option<Release>> {
        let response = self.http.get(format!("{}/latest.json", self.base)).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = response.error_for_status()?.bytes().await?;
        Release::parse(&body, &self.base).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://downloads.buwiz.com";
    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn entry(platform: &str, arch: &str, kind: &str, file: &str) -> serde_json::Value {
        serde_json::json!({"platform": platform, "arch": arch, "kind": kind, "file": file, "size": 1000, "sha256": SHA})
    }

    fn manifest(version: &str, files: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::json!({"version": version, "published_at": "2026-10-30T08:00:00Z", "files": files})
            .to_string()
            .into_bytes()
    }

    fn release(files: Vec<serde_json::Value>) -> Release {
        Release::parse(&manifest("1.2.0", files), BASE).unwrap()
    }

    #[test]
    fn a_manifest_reads_with_urls_under_its_version() {
        let release = release(vec![entry("macos", "universal", "dmg", "Buwiz_1.2.0_universal.dmg")]);
        assert_eq!(release.version, "1.2.0");
        assert_eq!(release.published_at.unwrap().date().to_string(), "2026-10-30");
        let file = &release.files[0];
        assert_eq!((file.platform, file.arch, file.kind, file.size), (Platform::Mac, Arch::Universal, Kind::Dmg, 1000));
        assert_eq!(file.url, "https://downloads.buwiz.com/releases/v1.2.0/Buwiz_1.2.0_universal.dmg");
    }

    #[test]
    fn file_names_that_leave_their_folder_are_left_out() {
        for name in [
            "../latest.json",
            "..",
            "sub/Buwiz.dmg",
            "/Buwiz.dmg",
            "https://evil.example/Buwiz.dmg",
            "//evil.example/x",
            "Buwiz.dmg?x=1",
            "Buwiz%2F.dmg",
            "back\\slash.dmg",
            ".hidden.dmg",
            "",
            "Buwiz 1.2.0.dmg",
        ] {
            let json = manifest(
                "1.2.0",
                vec![entry("macos", "universal", "dmg", name), entry("linux", "x64", "deb", "buwiz_1.2.0_amd64.deb")],
            );
            let release = Release::parse(&json, BASE).unwrap();
            assert_eq!(release.files.len(), 1, "{name:?}");
            assert_eq!(release.files[0].kind, Kind::Deb, "{name:?}");
        }
        let only_bad = manifest("1.2.0", vec![entry("macos", "universal", "dmg", "../x.dmg")]);
        assert!(Release::parse(&only_bad, BASE).is_err());
    }

    #[test]
    fn unknown_platforms_kinds_and_broken_entries_are_skipped() {
        let release = release(vec![
            entry("ios", "arm64", "ipa", "Buwiz.ipa"),
            entry("linux", "x64", "flatpak", "Buwiz.flatpak"),
            entry("linux", "riscv64", "deb", "buwiz_riscv64.deb"),
            // A kind on the wrong platform.
            entry("windows", "x64", "dmg", "Buwiz.dmg"),
            serde_json::json!({"platform": "linux", "kind": "deb"}),
            serde_json::json!({"platform": "linux", "arch": "x64", "kind": "deb", "file": "b.deb", "size": 1, "sha256": "nothex"}),
            entry("android", "universal", "apk", "Buwiz_1.2.0.apk"),
        ]);
        assert_eq!(release.files.len(), 1);
        assert_eq!(release.files[0].name, "Buwiz_1.2.0.apk");
    }

    #[test]
    fn versions_are_major_minor_patch() {
        for version in ["1.2.0", "0.0.1", "10.20.30"] {
            assert!(is_version(version), "{version}");
        }
        for version in ["1.2", "1.2.0.0", "v1.2.0", "1.2.0-beta", "1..0", "1.2.x", "../1.2.0", ""] {
            let json = manifest(version, vec![entry("android", "universal", "apk", "Buwiz.apk")]);
            assert!(Release::parse(&json, BASE).is_err(), "{version}");
        }
        assert!(Release::parse(b"not json", BASE).is_err());
    }

    #[test]
    fn each_platform_gets_a_sensible_default() {
        let release = release(vec![
            entry("macos", "arm64", "dmg", "mac-arm.dmg"),
            entry("macos", "universal", "dmg", "mac-universal.dmg"),
            entry("windows", "arm64", "msi", "win-arm.msi"),
            entry("windows", "x64", "exe", "win-x64.exe"),
            entry("windows", "x64", "msi", "win-x64.msi"),
            entry("linux", "x64", "deb", "linux.deb"),
            entry("linux", "arm64", "appimage", "linux-arm.AppImage"),
            entry("linux", "x64", "appimage", "linux-x64.AppImage"),
            entry("linux", "x64", "rpm", "linux.rpm"),
            entry("android", "arm64", "apk", "android-arm.apk"),
        ]);
        let pick = |platform, kind| release.pick(platform, kind, None).map(|file| file.name.as_str());
        assert_eq!(pick(Platform::Mac, None), Some("mac-universal.dmg"));
        assert_eq!(pick(Platform::Windows, None), Some("win-x64.exe"));
        assert_eq!(pick(Platform::Windows, Some(Kind::Msi)), Some("win-x64.msi"));
        assert_eq!(pick(Platform::Linux, None), Some("linux.deb"));
        assert_eq!(pick(Platform::Linux, Some(Kind::AppImage)), Some("linux-x64.AppImage"));
        assert_eq!(pick(Platform::Linux, Some(Kind::Rpm)), Some("linux.rpm"));
        assert_eq!(pick(Platform::Linux, Some(Kind::Apk)), None);
        assert_eq!(pick(Platform::Android, None), Some("android-arm.apk"));

        // What the release CI uploads today: the defaults fall back to it.
        let today = self::release(vec![
            entry("macos", "universal", "dmg", "Buwiz-1.2.0-macos-universal.dmg"),
            entry("windows", "x64", "exe", "Buwiz-1.2.0-windows-x64-setup.exe"),
            entry("linux", "x64", "deb", "buwiz_1.2.0_amd64.deb"),
        ]);
        let pick = |platform| today.pick(platform, None, None).map(|file| file.name.as_str());
        assert_eq!(pick(Platform::Mac), Some("Buwiz-1.2.0-macos-universal.dmg"));
        assert_eq!(pick(Platform::Windows), Some("Buwiz-1.2.0-windows-x64-setup.exe"));
        assert_eq!(pick(Platform::Linux), Some("buwiz_1.2.0_amd64.deb"));
        assert_eq!(pick(Platform::Android), None);

        let arm_only = self::release(vec![
            entry("macos", "arm64", "dmg", "mac-arm.dmg"),
            entry("windows", "arm64", "exe", "win-arm.exe"),
        ]);
        assert_eq!(arm_only.pick(Platform::Mac, None, None).unwrap().name, "mac-arm.dmg");
        assert_eq!(arm_only.pick(Platform::Windows, None, None).unwrap().name, "win-arm.exe");
        assert!(arm_only.pick(Platform::Linux, None, None).is_none());
    }

    #[test]
    fn linux_files_are_picked_by_kind_and_processor() {
        let release = release(vec![
            entry("macos", "universal", "dmg", "mac.dmg"),
            entry("linux", "x64", "deb", "amd64.deb"),
            entry("linux", "arm64", "deb", "arm64.deb"),
            entry("linux", "x64", "rpm", "x64.rpm"),
            entry("linux", "arm64", "targz", "arm64.tar.gz"),
            entry("linux", "universal", "sh", "install.sh"),
            // The script runs on any processor; any other is left out.
            entry("linux", "x64", "sh", "x64-install.sh"),
        ]);
        let pick = |kind, arch| release.pick(Platform::Linux, kind, arch).map(|file| file.name.as_str());
        assert_eq!(pick(Some(Kind::Deb), Some(Arch::X64)), Some("amd64.deb"));
        assert_eq!(pick(Some(Kind::Deb), Some(Arch::Arm64)), Some("arm64.deb"));
        assert_eq!(pick(Some(Kind::Rpm), Some(Arch::X64)), Some("x64.rpm"));
        assert_eq!(pick(Some(Kind::Rpm), Some(Arch::Arm64)), None);
        assert_eq!(pick(Some(Kind::TarGz), Some(Arch::Arm64)), Some("arm64.tar.gz"));
        assert_eq!(pick(Some(Kind::TarGz), Some(Arch::X64)), None);
        assert_eq!(pick(Some(Kind::Sh), Some(Arch::Arm64)), Some("install.sh"));
        assert_eq!(pick(Some(Kind::Sh), None), Some("install.sh"));
        // Never the script unless asked for it.
        assert_eq!(pick(None, Some(Arch::Arm64)), Some("arm64.deb"));
        let script_only = self::release(vec![entry("linux", "universal", "sh", "install.sh")]);
        assert!(script_only.pick(Platform::Linux, None, None).is_none());
        // A universal Mac build suits either processor.
        assert_eq!(release.pick(Platform::Mac, None, Some(Arch::Arm64)).unwrap().name, "mac.dmg");
        assert_eq!(release.files.iter().filter(|file| file.kind == Kind::Sh).count(), 1);
    }

    #[test]
    fn the_visitors_platform_is_guessed_from_the_user_agent() {
        let cases = [
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15",
                Some(Platform::Mac),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36",
                Some(Platform::Windows),
            ),
            ("Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0", Some(Platform::Linux)),
            ("Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0", Some(Platform::Linux)),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/130.0 Mobile Safari/537.36",
                Some(Platform::Android),
            ),
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148", None),
            ("Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148", None),
            ("Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36", None),
            ("curl/8.7.1", None),
            ("", None),
        ];
        for (agent, platform) in cases {
            assert_eq!(guess_platform(agent), platform, "{agent}");
        }
        for (agent, device) in [
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148", Device::Ios),
            ("Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148", Device::Ios),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36",
                Device::Chromebook,
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
                Device::Platform(Platform::Linux),
            ),
        ] {
            assert_eq!(guess_device(agent), Some(device), "{agent}");
        }
        assert_eq!(guess_device("curl/8.7.1"), None);
    }

    /// Serves `body` (or a 404 when `None`) as `/latest.json` on a free port.
    async fn origin(body: Arc<Mutex<Option<Vec<u8>>>>) -> String {
        let app = axum::Router::new().route(
            "/latest.json",
            axum::routing::get(move || {
                let body = body.lock().unwrap().clone();
                async move {
                    use axum::response::IntoResponse;
                    match body {
                        Some(body) => body.into_response(),
                        None => http::StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_last_good_manifest_and_a_404_takes_it_back() {
        let good = manifest("1.2.0", vec![entry("android", "universal", "apk", "Buwiz.apk")]);
        let body = Arc::new(Mutex::new(Some(b"{broken".to_vec())));
        let base = origin(body.clone()).await;
        let downloads = Downloads::with_refresh(&base, Duration::ZERO);
        assert!(downloads.latest().await.is_none(), "never a good manifest: not released");

        *body.lock().unwrap() = Some(good);
        assert_eq!(downloads.latest().await.unwrap().version, "1.2.0");
        *body.lock().unwrap() = Some(b"{broken".to_vec());
        assert_eq!(downloads.latest().await.unwrap().version, "1.2.0", "kept the last good one");

        *body.lock().unwrap() = None;
        assert!(downloads.latest().await.is_none(), "latest.json removed: not released");
    }

    #[tokio::test]
    async fn the_manifest_is_asked_for_at_most_once_a_minute() {
        let body = Arc::new(Mutex::new(None));
        let base = origin(body.clone()).await;
        let downloads = Downloads::new(&base);
        assert!(downloads.latest().await.is_none());
        *body.lock().unwrap() = Some(manifest("1.2.0", vec![entry("android", "universal", "apk", "Buwiz.apk")]));
        assert!(downloads.latest().await.is_none(), "still the answer from a moment ago");
    }
}
