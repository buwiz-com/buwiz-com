//! The download page: a card per platform with one Download button for its
//! installer, its version and size, and its SHA-256 in small print. Any
//! other files for the platform (the MSI, the rpm, ARM64 builds) are plain
//! text links under it. The visitor's platform, guessed from `User-Agent`,
//! is marked. Until a version is released (see [`crate::downloads`]) each
//! card says "Coming soon" and nothing links anywhere. Every link goes
//! through this site's counted routes. Styles are in `src/home.css`.

use std::sync::Arc;

use http::header;

use super::home::{Logo, PLATFORMS, footer, has_install_script, install_line, platform_logo};
use super::*;
use crate::canonical::is_app_host;
use crate::downloads::{Arch, File, Kind, Platform, Release, guess_platform};

pub(super) const TITLE: &str = "Download Buwiz for Windows, macOS, Linux and Android";
pub(super) const DESCRIPTION: &str = "Get the Buwiz app for your computer or Android phone, with each installer's SHA-256 to check it against. Buwiz for iOS is coming to the App Store.";

#[page("/download")]
pub async fn download_page(cx: &Cx) -> Result<impl View> {
    let header = |name| headers(cx).get(name).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if is_app_host(header(header::HOST)) {
        return Err(topcoat::router::error::not_found().into());
    }
    let yours = guess_platform(header(header::USER_AGENT));
    let release = state(cx).downloads.latest().await;
    let lead = match release.as_deref() {
        Some(release) => match release.published_at {
            Some(at) => format!("Version {}, released {}. Free with your Buwiz account.", release.version, date(at)),
            None => format!("Version {}. Free with your Buwiz account.", release.version),
        },
        None => "Buwiz is coming soon to Windows, macOS, Linux and Android, and to the App Store for iOS. The installers will be here.".to_owned(),
    };
    let released = release.is_some();
    let forms_origin = state(cx).config.forms_origin.clone();
    Ok(view! {
        <main class="home landing">
            <section class="section downloads">
                <div class="section-head">
                    <span class="eyebrow">icon(name: "download") "Download"</span>
                    <h1>"Buwiz for your computer"</h1>
                    <p class="section-lead">(lead)</p>
                </div>
                <ul class="dl-grid">
                    for (name, platform, paths) in PLATFORMS {
                        platform_card(
                            name: name,
                            platform: platform,
                            paths: paths,
                            release: release.clone(),
                            yours: platform.is_some() && platform == yours,
                            forms_origin: forms_origin.clone(),
                        )
                    }
                </ul>
                if released {
                    <p class="dl-check">
                        "To check a download, compare its SHA-256: "
                        <code>"shasum -a 256 <file>"</code>
                        " on macOS and Linux, "
                        <code>"certutil -hashfile <file> SHA256"</code>
                        " on Windows."
                    </p>
                }
            </section>
            footer()
        </main>
    })
}

/// The file a platform's button gives, and the link for it. Linux's is the
/// x64 deb when there is one, which is what `/download/linux` sends; every
/// other platform's is what `/download/{platform}` sends.
fn main_file(release: &Release, platform: Platform) -> Option<(&File, String)> {
    if platform == Platform::Linux
        && let Some(deb) = release.pick(Platform::Linux, Some(Kind::Deb), Some(Arch::X64))
    {
        return Some((deb, "/download/linux".to_owned()));
    }
    let file = release.pick(platform, None, None)?;
    let href = if platform == Platform::Linux { file.route() } else { format!("/download/{}", platform.slug()) };
    Some((file, href))
}

/// One platform: its Download button and facts, or when it will come.
#[component]
async fn platform_card(
    name: &'static str,
    platform: Option<Platform>,
    paths: Logo,
    release: Option<Arc<Release>>,
    yours: bool,
    forms_origin: String,
) -> Result<impl View> {
    let main = release.as_deref().zip(platform).and_then(|(release, platform)| {
        let (file, href) = main_file(release, platform)?;
        let others: Vec<File> = release.files_for(platform).filter(|other| other.name != file.name).cloned().collect();
        Some((file.clone(), href, others, release.version.clone()))
    });
    let script = platform == Some(Platform::Linux) && release.as_deref().is_some_and(has_install_script);
    let id = platform.map_or("ios", Platform::slug);
    let button = format!("Download for {}", if platform == Some(Platform::Mac) { "Mac" } else { name });
    Ok(view! {
        <li class=(if yours { "panel dl-card yours" } else { "panel dl-card" }) id=(id)>
            <div class="dl-card-head">
                platform_logo(label: name.to_owned(), paths: paths)
                <h2>(name)</h2>
                if yours {
                    <span class="dl-yours">"Your device"</span>
                }
            </div>
            if let Some((file, href, others, version)) = main {
                <a class="btn btn-solid" href=(href)>
                    icon(name: "download")
                    (button)
                </a>
                <p class="dl-facts">(format!("Version {version} · {} · {}", short_label(&file), size(file.size)))</p>
                <p class="dl-sha"><span>"SHA-256"</span> <code>(file.sha256.clone())</code></p>
                if script {
                    install_line(origin: forms_origin)
                }
                if !others.is_empty() {
                    <ul class="dl-others" aria-label=(format!("Other files for {name}"))>
                        for other in others {
                            <li><a href=(other.route())>(short_label(&other))</a> <span>(size(other.size))</span></li>
                        }
                    </ul>
                }
            } else if platform.is_none() {
                <p class="dl-soon">"Coming to the App Store"</p>
            } else {
                <span class="soon-tag">"Coming soon"</span>
            }
        </li>
    })
}

/// What a file is, in a few words: `.deb · x64`, `.dmg`.
fn short_label(file: &File) -> String {
    let kind = match file.kind {
        Kind::Dmg => ".dmg",
        Kind::Msi => ".msi (for IT)",
        Kind::Exe => ".exe",
        Kind::AppImage => "AppImage",
        Kind::Deb => ".deb",
        Kind::Rpm => ".rpm",
        Kind::TarGz => ".tar.gz",
        Kind::Sh => "Install script",
        Kind::Apk => ".apk",
    };
    match file.arch {
        Arch::Universal => kind.to_owned(),
        arch => format!("{kind} · {}", arch.label(file.platform)),
    }
}

/// `84.2 MB`, in decimal units as the Finder shows them.
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes.div_ceil(1000))
    }
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(size(84_213_000), "84.2 MB");
        assert_eq!(size(1_000_000), "1.0 MB");
        assert_eq!(size(999_999), "1000 KB");
        assert_eq!(size(1), "1 KB");
    }
}
