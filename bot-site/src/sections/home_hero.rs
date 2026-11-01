use crate::bot_meta::{self, BotMeta, REPO_URL};
use crate::components::{GithubMark, HeroFilm};
use leptos::prelude::*;

const PRIMARY: &str = "w-full sm:w-auto inline-flex items-center justify-center gap-2 bg-gc-brand text-white px-7 py-3.5 rounded-md text-sm font-bold tracking-wide hover:bg-gc-brand-hover active:scale-[0.98] transition-all text-center";
const UNAVAILABLE: &str = "w-full sm:w-auto inline-flex items-center justify-center gap-2 px-7 py-3.5 rounded-md text-sm font-bold tracking-wide text-gc-text-muted bg-gc-bg-muted border border-gc-border cursor-not-allowed select-none text-center";
const SECONDARY: &str = "w-full sm:w-auto inline-flex items-center justify-center gap-2 px-7 py-3.5 rounded-md text-sm font-semibold text-gc-text border border-gc-border bg-gc-bg-card/80 hover:border-gc-brand/40 active:scale-[0.98] transition-all text-center";

/// First section: what Buwiz Bot is, its download and its source, then the
/// self-playing film in the agent window. The particle field behind it is
/// the layout's, behind every page.
#[component]
pub fn HomeHero() -> impl IntoView {
    let meta = use_context::<BotMeta>().unwrap_or_default();
    let desktop = bot_meta::guess_desktop();
    let release = meta.release.clone();
    let installer = desktop.zip(release.as_ref()).and_then(|(desktop, release)| release.installer(desktop).cloned());
    let target = desktop.map_or("desktop", |desktop| desktop.name());

    // The installer for this computer, or a button that says when.
    let download = match &installer {
        Some(installer) => {
            view! { <a href=installer.url.clone() class=PRIMARY>{format!("Download for {target}")}</a> }.into_any()
        }
        None => {
            let label = match (&release, desktop) {
                // Released, but this is a phone or a computer we cannot tell.
                (Some(_), None) => format!("Download for {target}"),
                _ => format!("Download for {target} · Coming soon"),
            };
            view! { <a aria-disabled="true" class=UNAVAILABLE>{label}</a> }.into_any()
        }
    };

    let mut note = String::from("Free and open source · Apache-2.0");
    if let Some(release) = &release {
        note.push_str(&format!(" · v{}", release.version));
        if let Some(installer) = &installer {
            note.push_str(&format!(" · {}", bot_meta::size(installer.size)));
        }
    }

    view! {
        <header class="relative min-h-[100dvh] block pt-20 md:pt-24 pb-8">
            <div class="relative z-10 w-full mx-auto px-4 md:px-8">
                <div class="max-w-3xl mx-auto mb-8 md:mb-10 text-center">
                    <div class="inline-flex items-center gap-2 rounded-full border border-gc-brand/25 bg-gc-brand/10 px-3.5 py-1 mb-5 text-xs font-semibold text-gc-text">
                        <span class="h-1.5 w-1.5 rounded-full bg-[#3fb58f]" aria-hidden="true"></span>
                        "Buwiz Bot · Free and open source"
                    </div>
                    <h1 class="text-4xl md:text-6xl font-black tracking-tighter leading-[1.08] text-gc-text mb-5">
                        "AI coworkers that work on a computer you control."
                    </h1>
                    <p class="text-base md:text-xl text-gc-text-muted mb-8 max-w-2xl mx-auto">
                        "Give each one a job, the tools it needs and a computer you host. Bring any model and keep your data. Free, open source, no lock-in."
                    </p>
                    <div class="flex flex-col sm:flex-row items-center justify-center gap-3">
                        {download}
                        <a href=REPO_URL rel="noopener" class=SECONDARY>
                            <GithubMark class="h-[18px] w-[18px]" />
                            "Star on GitHub"
                            {meta.star_label().map(|stars| view! {
                                <span class="ml-1 border-l border-gc-border pl-2.5 text-gc-text-muted tabular-nums">{stars}</span>
                            })}
                        </a>
                    </div>
                    <p class="mt-3 text-xs text-gc-text-faint">{note}</p>
                </div>

                <div id="models" class="max-w-[1080px] mx-auto mt-16 md:mt-24 mb-8 md:mb-10 scroll-mt-24">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Use the best model for the job."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[60ch]">
                        "No single model is best at everything. One paints, one writes code, one reads what it sees. Your coworker switches mid-thread, on your own plan or an API key, and keeps the same chat, files and computer. Each reply says which one answered and who paid."
                    </p>
                </div>

                <HeroFilm />
            </div>
        </header>
    }
}
