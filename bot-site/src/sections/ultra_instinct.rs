use leptos::prelude::*;

const LOOP: [(&str, &str); 4] = [
    ("Think", "New page. The model (JEV) picks an action."),
    ("Record", "Save what it did and what happened."),
    ("Replay", "Test the lesson on past runs, offline."),
    ("Instinct", "Only proven lessons are promoted. No model call next time."),
];

const FLAVORS: [(&str, &str, &str, &str); 3] = [
    ("Browser", "Web pages. The counterpart to JEV UltraFast.", "ultra-instinct", "first"),
    ("Native apps", "GPUI apps, driven from inside the app.", "gpui-agent", "later"),
    ("Whole desktop", "Any screen, mouse and keyboard.", "ultra-instinct-cua", "later"),
];

/// Coming soon: compile repeated reasoning into verified instinct.
#[component]
pub fn UltraInstinct() -> impl IntoView {
    view! {
        <section id="instinct" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1440px] mx-auto px-6 md:px-12">
                <div class="max-w-2xl mb-10 md:mb-14">
                    <div class="text-[11px] font-bold tracking-wide uppercase text-gc-gold mb-3">"Coming soon"</div>
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Ultra Instinct"
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[50ch]">
                        "Think it through once. Do it a hundred times without thinking. The model handles the new case. Repeats become instinct."
                    </p>
                </div>

                <ol class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
                    {LOOP.iter().enumerate().map(|(i, (name, what))| view! {
                        <li class="rounded-2xl border border-gc-border bg-gc-bg-card p-5">
                            <div class="font-mono text-[11px] text-gc-text-faint mb-2">{format!("{}", i + 1)}</div>
                            <div class="text-lg font-bold text-gc-text mb-1">{*name}</div>
                            <p class="text-sm text-gc-text-muted">{*what}</p>
                        </li>
                    }).collect_view()}
                </ol>
                <p class="text-xl md:text-2xl font-black tracking-tight text-gc-text mb-2">
                    "Experience can change the choice. It can't break the rules."
                </p>
                <p class="text-sm text-gc-text-muted max-w-[70ch] mb-14">
                    "It still picks only from what's on screen, still stops when unsure, still passes the gate and the check after."
                </p>

                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                    {FLAVORS.iter().map(|(name, what, repo, when)| view! {
                        <div class="rounded-2xl border border-gc-border bg-gc-bg-card p-6 flex flex-col gap-2">
                            <div class="flex items-baseline justify-between gap-3">
                                <div class="text-lg font-bold text-gc-text">{*name}</div>
                                <span class="text-[10px] font-bold tracking-wide uppercase text-gc-gold">{*when}</span>
                            </div>
                            <p class="text-sm text-gc-text-muted">{*what}</p>
                            <span class="font-mono text-[11px] text-gc-text-faint">{*repo}</span>
                        </div>
                    }).collect_view()}
                </div>
                <p class="text-sm text-gc-text-muted mt-6 max-w-[70ch]">
                    "With the CLI and skill, your coworker reaches for a recorded recipe first and gathers its metadata."
                </p>
            </div>
        </section>
    }
}
