use crate::bot_meta::{self, BotMeta, Desktop, Installer, NO_RELEASE, REPO_URL};
use leptos::prelude::*;

const BUTTON: &str = "inline-flex items-center justify-center gap-2 bg-gc-brand text-white px-5 py-2.5 rounded-md text-sm font-bold tracking-wide hover:bg-gc-brand-hover active:scale-[0.98] transition-all text-center";

/// The installers of the latest release, one card per platform that has a
/// build; before the first release, a line that says so.
#[component]
pub fn Download() -> impl IntoView {
    let release = use_context::<BotMeta>().unwrap_or_default().release;

    let body = match release {
        Some(release) => {
            let version = release.version.clone();
            let cards = release
                .installers()
                .into_iter()
                .map(|(desktop, installer)| card(desktop, installer, &version))
                .collect_view();
            view! {
                <p class="text-lg text-gc-text-muted mb-8">{format!("Version {version}.")}</p>
                <div class="grid gap-4 md:grid-cols-3">{cards}</div>
                <p class="mt-6 text-sm text-gc-text-muted">
                    <a href=format!("{REPO_URL}/releases") rel="noopener" class="hover:text-gc-brand transition-colors">
                        "Every release on GitHub"
                    </a>
                </p>
            }
            .into_any()
        }
        None => view! {
            <p class="text-lg text-gc-text-muted max-w-[60ch]">
                {NO_RELEASE}". You can "
                <a href=REPO_URL rel="noopener" class="text-gc-brand hover:underline">"build it from source"</a>
                " on GitHub."
            </p>
        }
        .into_any(),
    };

    view! {
        <section id="download" class="relative z-10 max-w-[1080px] mx-auto px-4 md:px-8 py-16 md:py-24 scroll-mt-24">
            <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                "Download Buwiz Bot"
            </h2>
            {body}
        </section>
    }
}

fn card(desktop: Desktop, installer: Installer, version: &str) -> impl IntoView {
    let details = format!("{} · v{version} · {}", installer.processor(desktop), bot_meta::size(installer.size));
    view! {
        <div class="flex flex-col gap-3 rounded-lg border border-gc-border bg-gc-bg-card/80 p-5">
            <h3 class="text-lg font-bold text-gc-text">{desktop.name()}</h3>
            <p class="text-sm text-gc-text-muted">{details}</p>
            <a href=installer.url.clone() class=BUTTON>{format!("Download for {}", desktop.name())}</a>
            <p class="text-xs text-gc-text-faint break-all">"SHA-256: "<code>{installer.sha256.clone()}</code></p>
        </div>
    }
}
