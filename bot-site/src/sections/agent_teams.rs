use leptos::prelude::*;

struct SpecAgent {
    name: &'static str,
    role: &'static str,
    color: &'static str,
    mark: &'static str,
    model: &'static str,
    skills: &'static [&'static str],
    connectors: &'static [&'static str],
    knowledge: &'static str,
}

struct Team {
    name: &'static str,
    computer: Option<&'static str>,
    agents: &'static [SpecAgent],
}

const TEAMS: [Team; 3] = [
    Team {
        name: "Ship software safely",
        computer: Some("box-ship"),
        agents: &[
            SpecAgent {
                name: "Ada",
                role: "Tech lead",
                color: "bg-[#8b5cf6]",
                mark: "Ad",
                model: "Claude",
                skills: &["review", "patch", "tests"],
                connectors: &["GitHub", "CI"],
                knowledge: "Repo map, coding standards, release checklist",
            },
            SpecAgent {
                name: "Pixel",
                role: "Frontend",
                color: "bg-[#3b82f6]",
                mark: "Px",
                model: "Gemini 3.8",
                skills: &["screenshot", "computer use"],
                connectors: &["Figma"],
                knowledge: "Design tokens, component inventory",
            },
            SpecAgent {
                name: "Rigel",
                role: "QA",
                color: "bg-[#f59e0b]",
                mark: "Ri",
                model: "Claude",
                skills: &["recipe", "assert"],
                connectors: &["box-exec"],
                knowledge: "Known flakes, browser matrix",
            },
        ],
    },
    Team {
        name: "Close the books",
        computer: Some("box-finance"),
        agents: &[
            SpecAgent {
                name: "Finance",
                role: "Controller",
                color: "bg-[#f0a04b]",
                mark: "Fi",
                model: "Grok 4.7",
                skills: &["ledger", "recipes"],
                connectors: &["eBIRForms"],
                knowledge: "Chart of accounts, filing calendar",
            },
            SpecAgent {
                name: "Ops Chief",
                role: "Right hand",
                color: "bg-[#7dcea0]",
                mark: "OC",
                model: "Claude",
                skills: &["schedule", "triage"],
                connectors: &["Calendar"],
                knowledge: "Who owns which close step",
            },
        ],
    },
    Team {
        name: "Find qualified leads",
        computer: None,
        agents: &[
            SpecAgent {
                name: "Scout",
                role: "Researcher",
                color: "bg-[#3b82f6]",
                mark: "Sc",
                model: "GPT-6 Astra",
                skills: &["search", "extract"],
                connectors: &["websearch"],
                knowledge: "ICP notes, disqualify list",
            },
            SpecAgent {
                name: "Relay",
                role: "Outreach",
                color: "bg-[#34d399]",
                mark: "Re",
                model: "Claude",
                skills: &["draft", "voice"],
                connectors: &["Inbox"],
                knowledge: "Tone guide, past threads",
            },
        ],
    },
];

fn agent_at(i: usize) -> &'static SpecAgent {
    let mut n = 0;
    for team in &TEAMS {
        for a in team.agents {
            if n == i {
                return a;
            }
            n += 1;
        }
    }
    &TEAMS[0].agents[0]
}

fn team_for(i: usize) -> &'static Team {
    let mut n = 0;
    for team in &TEAMS {
        for _ in team.agents {
            if n == i {
                return team;
            }
            n += 1;
        }
    }
    &TEAMS[0]
}

/// Grouped specialized agents. Hover or tap a coworker to see model, skills, connectors, knowledge.
#[component]
pub fn AgentTeams() -> impl IntoView {
    let selected = RwSignal::new(0usize);

    view! {
        <section id="teams" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1440px] mx-auto px-6 md:px-12">
                <div class="max-w-2xl mb-10 md:mb-14">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Specialize, then group."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[52ch]">
                        "An agent is more than a model and a prompt. Give it toolcall skills, plugins, a knowledge base. Put coworkers on a team. Share a box with that team."
                    </p>
                </div>

                <div class="grid grid-cols-1 lg:grid-cols-12 gap-6 lg:gap-8 items-start">
                    <div class="lg:col-span-8 rounded-2xl border border-gc-border bg-gc-bg p-4 md:p-6 gc-team-canvas">
                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                            {
                                let mut idx = 0usize;
                                TEAMS.iter().map(|team| {
                                    let start = idx;
                                    idx += team.agents.len();
                                    view! {
                                        <div class="rounded-2xl border border-gc-border bg-gc-bg-card p-4 md:p-5">
                                            <div class="flex items-start justify-between gap-3 mb-4">
                                                <div class="text-sm font-bold text-gc-text">{team.name}</div>
                                                {team.computer.map(|c| view! {
                                                    <div class="flex items-center gap-1 text-[11px] text-gc-text-muted shrink-0">
                                                        <span class="material-symbols-outlined text-[14px]">"computer"</span>
                                                        {c}
                                                    </div>
                                                })}
                                            </div>
                                            <div class="flex flex-col gap-2">
                                                {team.agents.iter().enumerate().map(|(j, a)| {
                                                    let i = start + j;
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class=move || {
                                                                if selected.get() == i {
                                                                    "flex items-center gap-3 w-full text-left rounded-xl border border-gc-brand/40 bg-gc-bg px-3 py-2.5"
                                                                } else {
                                                                    "flex items-center gap-3 w-full text-left rounded-xl border border-gc-border bg-gc-bg-muted/50 px-3 py-2.5 hover:border-gc-border-hover"
                                                                }
                                                            }
                                                            on:mouseenter=move |_| selected.set(i)
                                                            on:click=move |_| selected.set(i)
                                                        >
                                                            <span class=format!("w-8 h-8 rounded-full {} flex items-center justify-center text-[10px] font-bold text-[#1a1a1a] shrink-0", a.color)>{a.mark}</span>
                                                            <span class="min-w-0">
                                                                <span class="block text-sm font-semibold text-gc-text truncate">{a.name}</span>
                                                                <span class="block text-[11px] text-gc-text-muted truncate">{a.role}</span>
                                                            </span>
                                                            <span class="ml-auto text-[10px] font-semibold text-gc-text-faint shrink-0">{a.model}</span>
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()
                            }
                        </div>
                    </div>

                    <div class="lg:col-span-4 lg:sticky lg:top-28">
                        {move || {
                            let i = selected.get();
                            let a = agent_at(i);
                            let team = team_for(i);
                            view! {
                                <div class="rounded-2xl border border-gc-border bg-gc-bg-card p-5 md:p-6">
                                    <div class="flex items-center gap-3 mb-5">
                                        <span class=format!("w-12 h-12 rounded-full {} flex items-center justify-center text-sm font-bold text-[#1a1a1a]", a.color)>{a.mark}</span>
                                        <div>
                                            <div class="text-lg font-bold text-gc-text">{a.name}</div>
                                            <div class="text-sm text-gc-text-muted">{a.role}</div>
                                        </div>
                                    </div>
                                    <div class="space-y-4 text-sm">
                                        <div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint mb-1">"Model"</div>
                                            <div class="font-semibold text-gc-text">{a.model}</div>
                                        </div>
                                        <div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint mb-1">"Toolcall skills"</div>
                                            <div class="flex flex-wrap gap-1.5">
                                                {a.skills.iter().map(|s| view! {
                                                    <span class="px-2 py-0.5 rounded-full bg-gc-bg-muted border border-gc-border text-xs text-gc-text">{*s}</span>
                                                }).collect_view()}
                                            </div>
                                        </div>
                                        <div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint mb-1">"Plugins / connectors"</div>
                                            <div class="flex flex-wrap gap-1.5">
                                                {a.connectors.iter().map(|s| view! {
                                                    <span class="px-2 py-0.5 rounded-full bg-gc-bg-muted border border-gc-border text-xs text-gc-text">{*s}</span>
                                                }).collect_view()}
                                            </div>
                                        </div>
                                        <div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint mb-1">"Knowledge"</div>
                                            <div class="text-gc-text-muted">{a.knowledge}</div>
                                        </div>
                                        <div>
                                            <div class="text-[11px] font-semibold tracking-wide text-gc-text-faint mb-1">"Shared computer"</div>
                                            <div class="text-gc-text-muted">
                                                {team.computer.unwrap_or("None yet. Drop a box on this team.")}
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            }
                        }}
                    </div>
                </div>
            </div>
        </section>
    }
}
