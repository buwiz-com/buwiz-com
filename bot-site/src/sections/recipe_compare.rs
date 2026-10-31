use gloo_timers::callback::Interval;
use leptos::prelude::*;
use send_wrapper::SendWrapper;

struct Recipe {
    name: &'static str,
    what: &'static str,
    steps: &'static [(&'static str, &'static str)],
}

const RECIPES: [Recipe; 3] = [
    Recipe {
        name: "Open a site",
        what: "Focus the bar, type a URL, enter, wait for load.",
        steps: &[
            ("click", "address bar"),
            ("key", "ctrl+L"),
            ("type", "https://example.com"),
            ("key", "Return"),
            ("wait", "800ms"),
        ],
    },
    Recipe {
        name: "Export a form",
        what: "Open the form, fill it, export PDF.",
        steps: &[
            ("click", "new form"),
            ("type", "vendor name"),
            ("click", "export"),
            ("wait", "400ms"),
            ("screenshot", "done"),
        ],
    },
    Recipe {
        name: "Sign in once",
        what: "You type the password. The vault keeps it. The agent never sees it.",
        steps: &[
            ("click", "email field"),
            ("type", "you@company"),
            ("key", "Tab"),
            ("click", "sign in"),
            ("wait", "session"),
        ],
    },
];

fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten())
        .map(|mq| mq.matches())
        .unwrap_or(false)
}

fn hop_class(i: usize, at: i32) -> &'static str {
    if at < 0 {
        "flex items-center gap-3 rounded-xl border border-gc-border bg-gc-bg-muted/40 px-3 py-2 opacity-40"
    } else if (at as usize) == i {
        "flex items-center gap-3 rounded-xl border border-gc-brand/50 bg-gc-bg px-3 py-2"
    } else if (at as usize) > i {
        "flex items-center gap-3 rounded-xl border border-gc-border bg-gc-bg-muted px-3 py-2"
    } else {
        "flex items-center gap-3 rounded-xl border border-gc-border bg-gc-bg-muted/40 px-3 py-2 opacity-40"
    }
}

/// Usual N round trips vs grok-box one recipe tool call.
#[component]
pub fn RecipeCompare() -> impl IntoView {
    let selected = RwSignal::new(0usize);
    let usual_at = RwSignal::new(-1i32);
    let recipe_at = RwSignal::new(-1i32);
    let recipe_sent = RwSignal::new(false);

    Effect::new(move |_| {
        let idx = selected.get();
        let n = RECIPES[idx].steps.len() as i32;
        if prefers_reduced_motion() {
            usual_at.set(n);
            recipe_sent.set(true);
            recipe_at.set(n);
            return;
        }
        usual_at.set(-1);
        recipe_at.set(-1);
        recipe_sent.set(false);

        let usual = SendWrapper::new(Interval::new(480, move || {
            usual_at.update(|v| {
                if *v < n {
                    *v += 1;
                }
            });
        }));
        let recipe = SendWrapper::new(Interval::new(160, move || {
            if !recipe_sent.get_untracked() {
                recipe_sent.set(true);
                return;
            }
            recipe_at.update(|v| {
                if *v < n {
                    *v += 1;
                }
            });
        }));
        on_cleanup(move || {
            drop(usual);
            drop(recipe);
        });
    });

    view! {
        <section id="recipe" class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card">
            <div class="max-w-[1440px] mx-auto px-6 md:px-12">
                <div class="max-w-2xl mb-10 md:mb-14">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "One recipe. One tool call."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[50ch]">
                        "Other stacks screenshot after every click. grok-box cooks the whole plan in one POST. Pick a recipe."
                    </p>
                </div>

                <div class="grid grid-cols-1 lg:grid-cols-12 gap-8 items-start">
                    <div class="lg:col-span-4 flex flex-col gap-2" aria-label="Recipes">
                        {RECIPES.iter().enumerate().map(|(i, r)| {
                            view! {
                                <button
                                    type="button"
                                    aria-pressed=move || selected.get() == i
                                    class=move || {
                                        if selected.get() == i {
                                            "text-left rounded-2xl border border-gc-brand/40 bg-gc-bg p-4 md:p-5"
                                        } else {
                                            "text-left rounded-2xl border border-gc-border bg-gc-bg/40 p-4 md:p-5 hover:border-gc-border-hover"
                                        }
                                    }
                                    on:click=move |_| selected.set(i)
                                >
                                    <div class="text-base md:text-lg font-bold text-gc-text">{r.name}</div>
                                    <div class="text-sm text-gc-text-muted mt-1">{r.what}</div>
                                </button>
                            }
                        }).collect_view()}
                    </div>

                    <div class="lg:col-span-8">
                        {move || {
                            let r = &RECIPES[selected.get()];
                            let n = r.steps.len();
                            let usual = usual_at.get();
                            let rec = recipe_at.get();
                            let sent = recipe_sent.get();
                            view! {
                                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                                    <div class="rounded-2xl border border-gc-border bg-gc-bg p-5">
                                        <div class="flex items-baseline justify-between gap-3 mb-4">
                                            <div class="text-sm font-bold text-gc-text">"Usual"</div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint">
                                                {n.to_string()}" round trips"
                                            </div>
                                        </div>
                                        <p class="text-xs text-gc-text-muted mb-4">
                                            "Each click is a tool call. Then a screenshot. Then the model thinks. Then the next click."
                                        </p>
                                        <div class="flex flex-col gap-2">
                                            {r.steps.iter().enumerate().map(|(i, (op, detail))| {
                                                view! {
                                                    <div class=hop_class(i, usual)>
                                                        <span class="text-[10px] font-bold tracking-wide uppercase text-gc-brand w-16 shrink-0">{*op}</span>
                                                        <span class="text-sm text-gc-text truncate">{*detail}</span>
                                                        <span class="ml-auto text-[10px] text-gc-text-faint shrink-0">"call + shot"</span>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>

                                    <div class="rounded-2xl border border-gc-brand/30 bg-gc-bg p-5">
                                        <div class="flex items-baseline justify-between gap-3 mb-4">
                                            <div class="text-sm font-bold text-gc-text">"Recipe"</div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-brand">
                                                "1 tool call"
                                            </div>
                                        </div>
                                        <div class=if sent {
                                            "mb-4 rounded-xl border border-gc-brand/40 bg-gc-bg-muted px-3 py-2 font-mono text-[11px] text-gc-text"
                                        } else {
                                            "mb-4 rounded-xl border border-gc-border bg-gc-bg-muted/50 px-3 py-2 font-mono text-[11px] text-gc-text-faint"
                                        }>
                                            "POST /v1/cua/recipe"
                                        </div>
                                        <p class="text-xs text-gc-text-muted mb-4">
                                            "The box runs every step locally. The model waits for one result."
                                        </p>
                                        <div class="flex flex-col gap-2">
                                            {r.steps.iter().enumerate().map(|(i, (op, detail))| {
                                                view! {
                                                    <div class=hop_class(i, rec)>
                                                        <span class="text-[10px] font-bold tracking-wide uppercase text-gc-brand w-16 shrink-0">{*op}</span>
                                                        <span class="text-sm text-gc-text truncate">{*detail}</span>
                                                        <span class="ml-auto text-[10px] text-gc-text-faint shrink-0">"on box"</span>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                </div>
                            }
                        }}
                    </div>
                </div>

                <a href="#instinct" class="mt-10 inline-flex items-baseline gap-3 text-sm text-gc-text-muted hover:text-gc-text">
                    <span class="text-[10px] font-bold tracking-wide uppercase text-gc-gold">"Next"</span>
                    "You won't write recipes by hand. Ultra Instinct records them while you work →"
                </a>
            </div>
        </section>
    }
}
