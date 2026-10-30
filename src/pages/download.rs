//! The download page: a card per platform with its installer's version,
//! size and SHA-256, and the platform's other files. The visitor's platform,
//! guessed from `User-Agent`, is outlined; every card still links. Until a
//! version is released (see [`crate::downloads`]) each card says "Coming
//! soon" and nothing links anywhere. Styles are in `src/home.css`.
//!
//! The Linux card asks which Linux and which processor, then offers the one
//! file that fits; without JavaScript its radios are a plain form to
//! `/download/linux`, and every Linux file is listed under it.
//! `/assets/download.js` keeps the button's label and facts in step with
//! the radios. `?kind=` and `?arch=` choose for the visitor, as
//! `/download/linux` does when sending someone here.

use std::sync::Arc;

use http::header;

use super::home::{Logo, PLATFORMS, footer, platform_logo};
use super::*;
use crate::canonical::is_app_host;
use crate::downloads::{Arch, File, Kind, Platform, Release, guess_platform};

pub(super) const TITLE: &str = "Download Buwiz for Windows, macOS, Linux and Android";
pub(super) const DESCRIPTION: &str = "Get the Buwiz app for your computer or Android phone, with each installer's SHA-256 to check it against. Buwiz for iOS is coming to the App Store.";

/// The Linux kinds to choose between, with who they are for, in order.
const LINUX_KINDS: [(Kind, &str); 3] = [
    (Kind::Deb, "Ubuntu, Debian, Linux Mint"),
    (Kind::Rpm, "Fedora, Red Hat, openSUSE"),
    (Kind::TarGz, "Any other Linux"),
];

/// The processors to choose between: (arch, name, what `uname -m` says).
const LINUX_ARCHES: [(Arch, &str, &str); 2] =
    [(Arch::X64, "64-bit Intel or AMD", "uname -m → x86_64"), (Arch::Arm64, "ARM64", "uname -m → aarch64")];

/// The visitor's choice of Linux file: `?kind=` and `?arch=`.
#[derive(Clone, Copy, Default)]
struct LinuxChoice {
    kind: Option<Kind>,
    arch: Option<Arch>,
}

#[page("/download")]
pub async fn download_page(cx: &Cx) -> Result<impl View> {
    let header = |name| headers(cx).get(name).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if is_app_host(header(header::HOST)) {
        return Err(topcoat::router::error::not_found().into());
    }
    let yours = guess_platform(header(header::USER_AGENT));
    let choice = LinuxChoice {
        kind: query(cx, "kind").and_then(|name| Kind::from_name(&name)),
        arch: query(cx, "arch").and_then(|name| Arch::from_name(&name)),
    };
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
                            choice: choice,
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
                    <script src="/assets/download.js" defer=(true)></script>
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
    choice: LinuxChoice,
) -> Result<impl View> {
    let picker = release
        .as_deref()
        .filter(|_| platform == Some(Platform::Linux))
        .and_then(|release| linux_picker(release, choice));
    let picked = release.as_deref().zip(platform).and_then(|(release, platform)| release.pick(platform, None, None));
    let others: Vec<File> = match (release.as_deref(), picked) {
        (Some(release), Some(picked)) => {
            release.files_for(picked.platform).filter(|file| file.name != picked.name).cloned().collect()
        }
        _ => Vec::new(),
    };
    let version = release.as_ref().map(|release| release.version.clone()).unwrap_or_default();
    let picked = picked.cloned();
    let id = platform.map_or("ios", Platform::slug);
    Ok(view! {
        <li class=(if yours { "panel dl-card yours" } else { "panel dl-card" }) id=(id)>
            <div class="dl-card-head">
                platform_logo(label: name.to_owned(), paths: paths)
                <h2>(name)</h2>
                if yours {
                    <span class="badge">"Your device"</span>
                }
            </div>
            if let Some(picker) = picker {
                linux_card(picker: picker, version: version)
            } else if let Some(file) = picked {
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
                                <a href=(other.url.clone())>(file_label(&other))</a>
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

/// What a file is, for a list of files. The MSI says who it is for, so
/// people at home take the installer above it.
fn file_label(file: &File) -> String {
    let label = format!("{} · {}", file.kind.label(), file.arch.label(file.platform));
    if file.kind == Kind::Msi { format!("{label} · for IT / managed installs") } else { label }
}

/// The Linux card's choices, from what is released.
struct LinuxPicker {
    /// The kinds to choose from (at least one), with who they are for.
    kinds: Vec<(Kind, &'static str)>,
    /// The processors to choose from (at least one).
    arches: Vec<(Arch, &'static str, &'static str)>,
    kind: Kind,
    arch: Arch,
    /// The file for `kind` and `arch`, unless that one is not released.
    file: Option<File>,
    /// Every Linux file, the install script too.
    files: Vec<File>,
    /// The install script, when released.
    script: bool,
}

/// `None` when no deb, rpm or tar.gz is released: the card then offers the
/// default Linux file like any other card.
fn linux_picker(release: &Release, choice: LinuxChoice) -> Option<LinuxPicker> {
    let files: Vec<File> = release.files_for(Platform::Linux).cloned().collect();
    let offered = |kind: Kind, arch: Arch| files.iter().find(|file| file.kind == kind && file.arch == arch);
    let kinds: Vec<_> = LINUX_KINDS
        .into_iter()
        .filter(|(kind, _)| LINUX_ARCHES.iter().any(|(arch, ..)| offered(*kind, *arch).is_some()))
        .collect();
    let arches: Vec<_> = LINUX_ARCHES
        .into_iter()
        .filter(|(arch, ..)| kinds.iter().any(|(kind, _)| offered(*kind, *arch).is_some()))
        .collect();
    let (first_kind, _) = *kinds.first()?;
    let kind = choice.kind.filter(|asked| kinds.iter().any(|(kind, _)| kind == asked)).unwrap_or(first_kind);
    let arch = choice
        .arch
        .filter(|asked| arches.iter().any(|(arch, ..)| arch == asked))
        .or_else(|| arches.iter().map(|(arch, ..)| *arch).find(|arch| offered(kind, *arch).is_some()))
        .unwrap_or(arches[0].0);
    let file = offered(kind, arch).cloned();
    let script = files.iter().any(|file| file.kind == Kind::Sh);
    Some(LinuxPicker { kinds, arches, kind, arch, file, files, script })
}

/// The Linux card's body: which Linux and processor (each only when there
/// is more than one), the one button for the file that fits and its facts,
/// the install script's terminal line when there is one, then every file.
#[component]
async fn linux_card(picker: LinuxPicker, version: String) -> Result<impl View> {
    let LinuxPicker { kinds, arches, kind, arch, file, files, script } = picker;
    let label = file
        .as_ref()
        .map_or_else(|| "Not available for this choice yet".to_owned(), |file| format!("Download {}", file.name));
    let curl = format!("curl -fsSL {}/install-linux.sh | sh", crate::seo::SITE);
    Ok(view! {
        <form class="dl-pick" method="get" action="/download/linux" data-pick="">
            if kinds.len() > 1 {
                <fieldset class="dl-choices">
                    <legend>"Which Linux do you use?"</legend>
                    for (option, who) in kinds {
                        <label class="dl-choice">
                            <input type="radio" name="kind" value=(option.name()) checked=(option == kind)>
                            <span>(who)</span>
                        </label>
                    }
                </fieldset>
            } else {
                <input type="hidden" name="kind" value=(kind.name())>
            }
            if arches.len() > 1 {
                <fieldset class="dl-choices">
                    <legend>"Processor"</legend>
                    for (option, name, hint) in arches {
                        <label class="dl-choice">
                            <input type="radio" name="arch" value=(option.name()) checked=(option == arch)>
                            <span>(format!("{name} ")) <code>(hint)</code></span>
                        </label>
                    }
                </fieldset>
            } else {
                <input type="hidden" name="arch" value=(arch.name())>
            }
            <button class="btn btn-solid" type="submit" disabled=(file.is_none())>
                icon(name: "download")
                <span data-pick-label="">(label)</span>
            </button>
            <dl class="dl-facts" data-pick-facts="" hidden=(file.is_none())>
                <div><dt>"Version"</dt><dd>(version)</dd></div>
                <div><dt>"File"</dt><dd data-fact="name">(file.as_ref().map(|file| file.name.clone()).unwrap_or_default())</dd></div>
                <div><dt>"Size"</dt><dd data-fact="size">(file.as_ref().map(|file| size(file.size)).unwrap_or_default())</dd></div>
                <div><dt>"SHA-256"</dt><dd><code data-fact="sha256">(file.as_ref().map(|file| file.sha256.clone()).unwrap_or_default())</code></dd></div>
            </dl>
        </form>
        if script {
            <div class="dl-terminal" data-copy-group="">
                <p>"Or install from a terminal:"</p>
                <div class="copy-row">
                    <code>(curl.clone())</code>
                    <button class="btn btn-ghost" type="button" data-copy=(curl) hidden=(true)>icon(name: "content_copy") "Copy"</button>
                </div>
                <a href="/install-linux.sh">"View the script"</a>
                <span class="copy-status" role="status"></span>
            </div>
        }
        <ul class="dl-others" aria-label="Every Linux file">
            for other in files {
                <li
                    data-kind=(other.kind.name())
                    data-arch=(other.arch.name())
                    data-name=(other.name.clone())
                    data-size=(size(other.size))
                    data-sha256=(other.sha256.clone())
                >
                    <a href=(other.url.clone())>(file_label(&other))</a>
                    <span class="faint">(format!("{} · {}", other.name, size(other.size)))</span>
                    <code>(other.sha256.clone())</code>
                </li>
            }
        </ul>
    })
}

/// `84.2 MB`, in decimal units as the Finder shows them.
pub(super) fn size(bytes: u64) -> String {
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
