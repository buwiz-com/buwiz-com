//! The download page: a card per platform with its installer's version,
//! size and SHA-256, and the platform's other files. The visitor's platform,
//! guessed from `User-Agent`, is outlined; every card still links. Until a
//! version is released (see [`crate::downloads`]) each card says "Coming
//! soon" and nothing links anywhere. Styles are in `src/home.css`.

use std::sync::Arc;

use http::header;

use super::home::{Logo, PLATFORMS, footer, platform_logo};
use super::*;
use crate::canonical::is_app_host;
use crate::downloads::{File, Platform, Release, guess_platform};

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
                        )
                    }
                </ul>
                if released {
                    <p class="dl-check">
                        icon(name: "verified_user")
                        <span>
                            "To check a download, compare its SHA-256 with the one above: "
                            <code>"shasum -a 256 <file>"</code>
                            " on macOS and Linux, "
                            <code>"certutil -hashfile <file> SHA256"</code>
                            " on Windows."
                        </span>
                    </p>
                }
            </section>
            footer()
        </main>
    })
}

/// One platform: its installer and other files, or when it will come.
#[component]
async fn platform_card(
    name: &'static str,
    platform: Option<Platform>,
    paths: Logo,
    release: Option<Arc<Release>>,
    yours: bool,
) -> Result<impl View> {
    let picked = release.as_deref().zip(platform).and_then(|(release, platform)| release.pick(platform, None));
    let others: Vec<File> = match (release.as_deref(), picked) {
        (Some(release), Some(picked)) => {
            release.files_for(picked.platform).filter(|file| file.name != picked.name).cloned().collect()
        }
        _ => Vec::new(),
    };
    let version = release.as_ref().map(|release| release.version.clone()).unwrap_or_default();
    let picked = picked.cloned();
    Ok(view! {
        <li class=(if yours { "panel dl-card yours" } else { "panel dl-card" })>
            <div class="dl-card-head">
                platform_logo(label: name.to_owned(), paths: paths)
                <h2>(name)</h2>
                if yours {
                    <span class="badge">"Your device"</span>
                }
            </div>
            if let Some(file) = picked {
                <a class="btn btn-solid" href=(format!("/download/{}", file.platform.slug()))>
                    icon(name: "download")
                    (format!("Download for {name}"))
                </a>
                <dl class="dl-facts">
                    <div><dt>"Version"</dt><dd>(version)</dd></div>
                    <div><dt>"File"</dt><dd>(format!("{} · {}", file.kind.label(), file.arch.label(file.platform)))</dd></div>
                    <div><dt>"Size"</dt><dd>(size(file.size))</dd></div>
                    <div><dt>"SHA-256"</dt><dd><code>(file.sha256.clone())</code></dd></div>
                </dl>
                if !others.is_empty() {
                    <ul class="dl-others" aria-label=(format!("Other downloads for {name}"))>
                        for other in others {
                            <li>
                                <a href=(other.url.clone())>(format!("{} · {}", other.kind.label(), other.arch.label(other.platform)))</a>
                                <span class="faint">(format!("{} · {}", other.name, size(other.size)))</span>
                                <code>(other.sha256.clone())</code>
                            </li>
                        }
                    </ul>
                }
            } else {
                <span class="soon-tag">(if platform.is_none() { "App Store, coming soon" } else { "Coming soon" })</span>
            }
        </li>
    })
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
