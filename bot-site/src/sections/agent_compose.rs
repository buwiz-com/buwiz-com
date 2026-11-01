use leptos::{html, prelude::*};

use crate::components::agent_window::{
    AgentScreen, AgentWindow, GENIE, GENIE_RAIL, PaneIcon, PaneIcons, box_desktop, dock_apps,
};
use crate::components::demo_player::{Aim, DemoBar, DemoCursor, DemoPointer, Script, Timeline, autoplay_when_visible};
use crate::components::demo_widgets::{DemoMenu, MenuItem, SelectPill, Toggle};

/// Build your coworker: a NativeChat window that plays itself, the same window as Egress. The
/// pane's plug and wrench open Plugins and Tools. A cursor adds the Cloudflare plugin, signs in on Cloudflare's own page, gives Genie the account, shares it with another
/// bot, picks skills, and sets what the Computer tools may do without asking.
///
/// Everything clickable is a drawn box, never a real input or select.
const STEPS: &[&str] =
    &["Add plugin", "Sign in on the web", "Pick account", "Share with bots", "Skills", "Tool permissions", "Ready"];

const ACCOUNT: &str = "codeitlikemiley";
const ALWAYS: &str = "Always allow";
const ASK: &str = "Ask every time";

const SKILLS: [&str; 4] = ["agents-sdk", "cloudflare-email-service", "durable-objects", "wrangler"];

const TOOLS: [(&str, &str, &str); 8] = [
    ("terminal", "shell", "Runs commands on the Bot's own computer"),
    ("description", "read_file", "Reads a file on the Bot's computer"),
    ("edit_document", "write_file", "Creates or changes a file"),
    ("language", "open_url", "Opens a web page in its browser"),
    ("mouse", "computer", "Looks at the screen, clicks and types"),
    ("assignment", "request_user_form", "Asks you to fill in a form in chat"),
    ("menu_book", "run_recipe", "Runs a task you taught it"),
    ("forum", "message_bot", "Sends a message to another Bot"),
];

/// Computer's tools and whether each asks first. Row 3, restart, is the one the demo changes.
const PERMS: [(&str, bool); 7] = [
    ("computer_status", false),
    ("start_computer", false),
    ("shutdown_computer", false),
    ("restart_computer", false),
    ("reset_computer", true),
    ("update_computer", true),
    ("set_network", true),
];
const RESTART: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Closed,
    Plugins,
    Cloudflare,
    Bots,
    Tools,
    Computer,
}

#[derive(Clone, Copy, PartialEq)]
enum Web {
    Closed,
    Ask,
    Authorized,
}

/// The menu row or list entry the cursor is resting on.
#[derive(Clone, Copy, PartialEq)]
enum Lit {
    None,
    Account,
    Computer,
    Ask,
}

const ICO: &str = "grid h-7 w-7 shrink-0 place-items-center rounded-lg text-gc-text-muted";
const SEC: &str = "text-[11.5px] font-semibold text-gc-text";
const CARD: &str = "rounded-xl bg-gc-bg-muted px-3";
const ROW: &str =
    "flex min-w-0 items-center gap-2.5 border-b border-gc-border py-1.5 text-[13px] text-gc-text last:border-b-0";
const GRID: &str = "grid grid-cols-1 sm:grid-cols-2 gap-x-3.5 gap-y-0.5";
const TILE: &str = "grid shrink-0 place-items-center rounded-lg border border-gc-border bg-gc-bg-muted text-gc-text";

/// The cursor rests on a toolbar icon, which lights up, then clicks it.
fn tap(s: &mut Script, cur: DemoCursor, aim: Aim, lit: RwSignal<bool>, f: impl FnOnce() + 'static) {
    s.move_to(cur, aim)
        .then(move || lit.set(true))
        .wait(250)
        .press(cur)
        .wait(170)
        .then(move || {
            lit.set(false);
            f();
        })
        .wait(380);
}

fn search(text: &'static str) -> impl IntoView {
    view! {
        <div class="flex shrink-0 items-center gap-1.5 rounded-lg border border-gc-border bg-gc-bg-card px-2.5 py-1.5 text-[12.5px] text-gc-text-faint">
            <span class="material-symbols-outlined text-[15px]">"search"</span>
            {text}
        </div>
    }
}

fn added() -> impl IntoView {
    view! {
        <span class="inline-flex shrink-0 items-center gap-0.5 text-[12px] text-gc-text-muted">
            <span class="material-symbols-outlined text-[14px] text-gc-accent">"check"</span>
            "Added"
        </span>
    }
}

fn add(aim: NodeRef<html::Div>) -> impl IntoView {
    view! {
        <div node_ref=aim class="shrink-0 rounded-full border border-gc-border bg-gc-bg-muted px-3 py-1 text-[12px] font-semibold text-gc-text">"Add"</div>
    }
}

fn chevron() -> impl IntoView {
    view! { <span class="material-symbols-outlined shrink-0 text-[16px] text-gc-text-muted">"chevron_right"</span> }
}

/// A catalog entry: tile, name over a one-line description, and whatever sits on its right.
#[component]
fn Entry(
    tile: &'static str,
    #[prop(optional)] icon: bool,
    name: &'static str,
    desc: &'static str,
    #[prop(optional)] mono: bool,
    #[prop(optional)] aim: NodeRef<html::Div>,
    #[prop(optional, into)] lit: Signal<bool>,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || if lit.get() {
                "flex min-w-0 items-center gap-2.5 rounded-lg bg-gc-brand/10 p-1.5"
            } else {
                "flex min-w-0 items-center gap-2.5 rounded-lg p-1.5"
            }
        >
            {if icon {
                view! { <span class=format!("{TILE} h-[30px] w-[30px] text-gc-text-muted")><span class="material-symbols-outlined text-[17px] leading-none">{tile}</span></span> }.into_any()
            } else {
                view! { <span class=format!("{TILE} h-[30px] w-[30px] text-[12px] font-semibold")>{tile}</span> }.into_any()
            }}
            <span class="min-w-0 flex-1">
                <span class=if mono { "block font-mono text-[12px] text-gc-text" } else { "block text-[13px] font-medium text-gc-text" }>{name}</span>
                <span class="block truncate text-[11.5px] text-gc-text-muted">{desc}</span>
            </span>
            {children()}
        </div>
    }
}

/// A page heading inside the modal: big tile, name, and a line under it.
fn page_head(tile: &'static str, icon: bool, name: &'static str, sub: &'static str) -> impl IntoView {
    view! {
        <div class="flex shrink-0 items-center gap-3">
            {if icon {
                view! { <span class=format!("{TILE} h-11 w-11 rounded-[10px]")><span class="material-symbols-outlined text-[22px] leading-none">{tile}</span></span> }.into_any()
            } else {
                view! { <span class=format!("{TILE} h-11 w-11 rounded-[10px] text-[18px] font-semibold")>{tile}</span> }.into_any()
            }}
            <div class="min-w-0">
                <div class="text-[17px] font-semibold leading-tight text-gc-text">{name}</div>
                <div class="text-[12px] text-gc-text-muted">{sub}</div>
            </div>
        </div>
    }
}

#[component]
pub fn AgentCompose() -> impl IntoView {
    // Where the cursor goes.
    let win = NodeRef::<html::Div>::new();
    let back = NodeRef::<html::Div>::new();
    let close = NodeRef::<html::Div>::new();
    let cf_add = NodeRef::<html::Div>::new();
    let cf_row = NodeRef::<html::Div>::new();
    let add_acct = NodeRef::<html::Div>::new();
    let authorize = NodeRef::<html::Div>::new();
    let def_sel = NodeRef::<html::Div>::new();
    let opt_acct = NodeRef::<html::Div>::new();
    let bots_btn = NodeRef::<html::Div>::new();
    let bot_master = NodeRef::<html::Div>::new();
    let sk: [NodeRef<html::Div>; 4] = std::array::from_fn(|_| NodeRef::new());
    let comp_row = NodeRef::<html::Div>::new();
    let perm_sel = NodeRef::<html::Div>::new();
    let opt_ask = NodeRef::<html::Div>::new();
    let composer = NodeRef::<html::Div>::new();

    // What the window shows.
    let pane = RwSignal::new(Pane::Closed);
    let web = RwSignal::new(Web::Closed);
    let lit = RwSignal::new(Lit::None);
    let cf_added = RwSignal::new(false);
    let has_account = RwSignal::new(false);
    let default_set = RwSignal::new(false);
    let def_menu = RwSignal::new(false);
    let master = RwSignal::new(false);
    let skills = RwSignal::new([true, true, false, true]);
    let restart_asks = RwSignal::new(false);
    let perm_menu = RwSignal::new(false);
    let ready = RwSignal::new(false);
    let step = RwSignal::new(None::<usize>);
    let caption = RwSignal::new("");

    let plug_lit = RwSignal::new(false);
    let tools_lit = RwSignal::new(false);
    let icons =
        PaneIcons { plug: PaneIcon::new().lit(plug_lit), wrench: PaneIcon::new().lit(tools_lit), ..PaneIcons::new() };
    let screen = AgentScreen::new();

    let tl = Timeline::new();
    tl.speed.set(2.);
    let cur = DemoCursor::new(tl, win, (0.5, 0.86));
    let plugins = icons.plug.aim();
    let tools = icons.wrench.aim();

    tl.script(move |s| {
        pane.set(Pane::Closed);
        web.set(Web::Closed);
        lit.set(Lit::None);
        cf_added.set(false);
        has_account.set(false);
        default_set.set(false);
        def_menu.set(false);
        master.set(false);
        skills.set([true, true, false, true]);
        restart_asks.set(false);
        perm_menu.set(false);
        ready.set(false);
        plug_lit.set(false);
        tools_lit.set(false);
        cur.reset();
        step.set(Some(0));
        caption.set("Genie needs Cloudflare. Open Plugins.");

        s.wait(500);
        tap(s, cur, plugins, plug_lit, move || pane.set(Pane::Plugins));
        s.then(move || caption.set("Add the cloudflare plugin from the catalog."))
            .click(cur, cf_add, move || cf_added.set(true))
            .wait(300)
            .click(cur, cf_row, move || pane.set(Pane::Cloudflare));

        s.then(move || {
            step.set(Some(1));
            caption
                .set("Add an account. Sign-in happens on Cloudflare, through the server. The app never sees a token.");
        })
        .click(cur, add_acct, move || web.set(Web::Ask))
        .wait(700)
        .click(cur, authorize, move || web.set(Web::Authorized))
        .wait(800)
        .then(move || {
            web.set(Web::Closed);
            has_account.set(true);
        })
        .wait(500);

        s.then(move || {
            step.set(Some(2));
            caption.set("Pick which account Genie uses by default, or leave it on Always ask.");
        })
        .click(cur, def_sel, move || def_menu.set(true))
        .move_to(cur, opt_acct)
        .then(move || lit.set(Lit::Account))
        .wait(200)
        .click(cur, opt_acct, move || {
            default_set.set(true);
            def_menu.set(false);
            lit.set(Lit::None);
        });

        s.then(move || {
            step.set(Some(3));
            caption.set("Share the account with another bot.");
        })
        .click(cur, bots_btn, move || pane.set(Pane::Bots))
        .click(cur, bot_master, move || master.set(true))
        .wait(400)
        .click(cur, back, move || pane.set(Pane::Cloudflare));

        s.then(move || {
            step.set(Some(4));
            caption.set("Turn skills on or off for this bot.");
        })
        .click(cur, sk[2], move || skills.update(|k| k[2] = true))
        .click(cur, sk[1], move || skills.update(|k| k[1] = false))
        .wait(300)
        .click(cur, close, move || pane.set(Pane::Closed));

        s.then(move || {
            step.set(Some(5));
            caption.set("Set what tools may do without asking.");
        });
        tap(s, cur, tools, tools_lit, move || pane.set(Pane::Tools));
        s.move_to(cur, comp_row)
            .then(move || lit.set(Lit::Computer))
            .wait(250)
            .click(cur, comp_row, move || {
                pane.set(Pane::Computer);
                lit.set(Lit::None);
            })
            .click(cur, perm_sel, move || perm_menu.set(true))
            .move_to(cur, opt_ask)
            .then(move || lit.set(Lit::Ask))
            .wait(200)
            .click(cur, opt_ask, move || {
                restart_asks.set(true);
                perm_menu.set(false);
                lit.set(Lit::None);
            })
            .wait(500)
            .click(cur, close, move || pane.set(Pane::Closed));

        s.then(move || {
            step.set(Some(6));
            caption.set(
                "Genie now has Cloudflare (codeitlikemiley), 3 of 4 skills, and asks before restarting its computer.",
            );
            ready.set(true);
        })
        .move_to(cur, composer);
    });

    autoplay_when_visible(tl, win);

    let bots = move || if master.get() { 2 } else { 1 };

    let plugins_view = move || {
        view! {
            {search("Search plugins")}
            <div class=SEC>"Apps"</div>
            <div class=GRID>
                <Entry tile="G" name="Gmail" desc="Sign in with your Gmail accounts">{added()}</Entry>
            </div>
            <div class=SEC>"Development"</div>
            <div class=GRID>
                <Entry tile="C" name="cloudflare" desc="Skills for the Cloudflare developer platform" aim=cf_row>
                    {move || if cf_added.get() { added().into_any() } else { add(cf_add).into_any() }}
                </Entry>
                <Entry tile="B" name="browser-use" desc="Give it a real browser">{add(NodeRef::new())}</Entry>
                <Entry tile="B" name="base44" desc="Build and deploy full-stack apps">{add(NodeRef::new())}</Entry>
                <Entry tile="S" name="supabase" desc="Official Supabase plugin">{added()}</Entry>
            </div>
            <div class=SEC>"Productivity"</div>
            <div class=GRID>
                <Entry tile="C" name="calcom" desc="Cal.com scheduling">{add(NodeRef::new())}</Entry>
                <Entry tile="Q" name="quo" desc="Send texts and calls">{add(NodeRef::new())}</Entry>
            </div>
        }
    };

    let cloudflare_view = move || {
        view! {
            {page_head("C", false, "cloudflare", "View Source · cloudflare/skills")}
            <div class=SEC>"Accounts"</div>
            <div class=CARD>
                <Show when=move || has_account.get()>
                    <div class=ROW>
                        <div class="min-w-0 flex-1">
                            <div class="flex items-center gap-1 truncate">
                                {ACCOUNT}
                                <span class="material-symbols-outlined text-[13px] text-gc-text-muted">"edit"</span>
                            </div>
                            <div class="text-[11.5px] text-gc-accent">"Connected"</div>
                        </div>
                        <div node_ref=bots_btn class="inline-flex shrink-0 items-center gap-1 rounded-full border border-gc-border bg-gc-bg-elevated px-2.5 py-0.5 text-[12px] text-gc-text">
                            <span class="material-symbols-outlined text-[14px]">"group"</span>
                            {move || format!("{} Bot{}", bots(), if bots() > 1 { "s" } else { "" })}
                        </div>
                        <span class="material-symbols-outlined shrink-0 text-[16px] text-gc-text-muted">"sync"</span>
                    </div>
                </Show>
                <div class=ROW>
                    <div node_ref=add_acct class="flex items-center gap-1.5 text-gc-text-muted">
                        <span class="material-symbols-outlined text-[16px]">"add"</span>
                        "Add Another Account"
                    </div>
                </div>
            </div>
            <div class=SEC>"Current Bot"</div>
            <div class=CARD>
                <div class=format!("{ROW} flex-wrap gap-y-1.5")>
                    <div class="min-w-[150px] flex-1">"Genie's default Cloudflare account"</div>
                    <SelectPill
                        label=Signal::derive(move || if default_set.get() { ACCOUNT } else { "Always ask" })
                        warn=Signal::derive(move || !default_set.get())
                        aim=def_sel
                    >
                        <Show when=move || def_menu.get()>
                            <DemoMenu class="right-0 mt-1 min-w-full">
                                <MenuItem label="Always ask" checked=true />
                                <MenuItem label=ACCOUNT lit=Signal::derive(move || lit.get() == Lit::Account) aim=opt_acct />
                            </DemoMenu>
                        </Show>
                    </SelectPill>
                </div>
            </div>
            <div class=SEC>"Tools"</div>
            <div class=CARD>
                <div class=ROW>
                    <div class="min-w-0 flex-1 truncate">"4 of 4 on for Genie"</div>
                    <span class="shrink-0 text-[12px] text-gc-text-muted">"Use in Genie"</span>
                    <Toggle on=true />
                </div>
            </div>
            <div class=SEC>"Skills 14"</div>
            <div class=CARD>
                {SKILLS.iter().enumerate().map(|(i, name)| view! {
                    <div class=ROW>
                        <div class="min-w-0 flex-1 truncate font-mono text-[12px]">{*name}</div>
                        <Toggle on=Signal::derive(move || skills.get()[i]) aim=sk[i] />
                        {chevron()}
                    </div>
                }).collect_view()}
            </div>
        }
    };

    let bots_view = move || {
        view! {
            <div class="text-[12.5px] text-gc-text-muted">{move || format!("{} of 2 Bots can use this account", bots())}</div>
            {search("Filter Bots")}
            <div class=CARD>
                <div class=ROW>
                    <span class=format!("{TILE} h-6 w-6 rounded-md text-[11px]")>"G"</span>
                    <div class="min-w-0 flex-1">"Genie"</div>
                    <Toggle on=true />
                </div>
                <div class=ROW>
                    <span class=format!("{TILE} h-6 w-6 rounded-md text-[11px]")>"M"</span>
                    <div class="min-w-0 flex-1">"Masterpowers"</div>
                    <Toggle on=master aim=bot_master />
                </div>
            </div>
        }
    };

    let tools_view = move || {
        view! {
            {search("Search tools")}
            <div class=SEC>"Built in"</div>
            <div class=GRID>
                {TOOLS.iter().map(|&(i, n, d)| view! {
                    <Entry tile=i icon=true name=n desc=d mono=true><Toggle on=true /></Entry>
                }).collect_view()}
                <Entry tile="extension" icon=true name="Plugins" desc="Finds, installs and removes plugins">
                    {chevron()}<Toggle on=true />
                </Entry>
                <Entry
                    tile="power_settings_new"
                    icon=true
                    name="Computer"
                    desc="Checks, starts, stops, restarts…"
                    aim=comp_row
                    lit=Signal::derive(move || lit.get() == Lit::Computer)
                >
                    {chevron()}<Toggle on=true />
                </Entry>
            </div>
        }
    };

    let computer_view = move || {
        view! {
            <div class="flex shrink-0 items-center gap-3">
                {page_head("power_settings_new", true, "Computer", "Built in · 7 of 7 allowed")}
                <div class="ml-auto"><Toggle on=true /></div>
            </div>
            <div class="text-[12.5px] text-gc-text-muted">"Resetting, updating and the network setting always ask you first."</div>
            <div class=SEC>"Tools 7"</div>
            <div class=CARD>
                {PERMS.iter().enumerate().map(|(i, &(name, asks))| {
                    let restart = i == RESTART;
                    let asks = Signal::derive(move || if restart { restart_asks.get() } else { asks });
                    view! {
                        <div class=ROW>
                            <div class="min-w-0 flex-1 truncate font-mono text-[12px]">{name}</div>
                            <SelectPill
                                label=Signal::derive(move || if asks.get() { ASK } else { ALWAYS })
                                warn=asks
                                aim=if restart { perm_sel } else { NodeRef::new() }
                            >
                                <Show when=move || restart && perm_menu.get()>
                                    <DemoMenu class="right-0 mt-1 min-w-full">
                                        <MenuItem label=ALWAYS checked=true />
                                        <MenuItem label=ASK lit=Signal::derive(move || lit.get() == Lit::Ask) aim=opt_ask />
                                        <MenuItem label="Never" />
                                    </DemoMenu>
                                </Show>
                            </SelectPill>
                        </div>
                    }
                }).collect_view()}
            </div>
        }
    };

    let title = move || match pane.get() {
        Pane::Plugins => "Plugins",
        Pane::Bots => "Bots using codeitlikemiley",
        Pane::Tools => "Tools",
        _ => "",
    };
    let has_back = move || matches!(pane.get(), Pane::Cloudflare | Pane::Bots | Pane::Computer);

    view! {
        <section id="compose" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Fine-grained control over every agent."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[56ch]">
                        "Plugins, accounts, which bot uses which login, skills, and what each tool may do without asking. Set per agent, click by click."
                    </p>
                </div>

                <DemoBar steps=STEPS current=step timeline=tl caption=caption />

                <AgentWindow
                    win
                    screen
                    who=GENIE
                    rail=GENIE_RAIL
                    icons
                    composer
                    desktop=|| box_desktop("Buwiz Bot Box · Genie", (), (), dock_apps())
                    overlay=move || view! {
                        // Modal
                        <div class=move || if pane.get() == Pane::Closed {
                            "absolute inset-0 z-10 grid place-items-center bg-black/35 opacity-0 pointer-events-none transition-opacity duration-300"
                        } else {
                            "absolute inset-0 z-10 grid place-items-center bg-black/35 opacity-100 transition-opacity duration-300"
                        }>
                            <div class="flex flex-col overflow-hidden rounded-2xl border border-gc-border bg-gc-bg-elevated shadow-2xl w-[calc(100%-16px)] h-[calc(100%-16px)] sm:w-[min(640px,92%)] sm:h-[min(600px,92%)]">
                                <div class="flex shrink-0 items-center gap-2 px-4 pt-3.5 pb-2 sm:px-5">
                                    <div node_ref=back class=move || if has_back() { ICO } else { "hidden" }>
                                        <span class="material-symbols-outlined text-[20px]">"chevron_left"</span>
                                    </div>
                                    <div class="min-w-0 truncate text-[15px] font-semibold text-gc-text">{title}</div>
                                    <div node_ref=close class=format!("{ICO} ml-auto")>
                                        <span class="material-symbols-outlined text-[18px]">"close"</span>
                                    </div>
                                </div>
                                <div class="flex min-h-0 flex-1 flex-col gap-2.5 overflow-hidden px-4 pb-4 sm:px-5">
                                    {move || match pane.get() {
                                        Pane::Closed => ().into_any(),
                                        Pane::Plugins => plugins_view().into_any(),
                                        Pane::Cloudflare => cloudflare_view().into_any(),
                                        Pane::Bots => bots_view().into_any(),
                                        Pane::Tools => tools_view().into_any(),
                                        Pane::Computer => computer_view().into_any(),
                                    }}
                                </div>
                            </div>
                        </div>

                        // Cloudflare's own sign-in page, in the browser. Its colors are Cloudflare's.
                        <Show when=move || web.get() != Web::Closed>
                            <div class="absolute z-20 inset-x-3 inset-y-[12%] sm:inset-x-[14%] sm:inset-y-[8%] flex flex-col overflow-hidden rounded-xl bg-white text-[#1f2328] shadow-[0_30px_80px_rgba(0,0,0,.45)]">
                                <div class="flex items-center gap-1.5 bg-[#f1f3f4] px-2.5 py-1.5 font-mono text-[11.5px] text-[#57606a]">
                                    <span class="material-symbols-outlined text-[14px]">"lock"</span>
                                    <span class="min-w-0 flex-1 truncate rounded-full bg-white px-2.5 py-0.5">"dash.cloudflare.com/oauth2/auth"</span>
                                </div>
                                <div class="grid flex-1 place-items-center p-4">
                                    <div class="grid w-full max-w-[300px] gap-2.5 text-center">
                                        <div class="text-[22px] font-extrabold tracking-tight text-[#f38020]">"cloudflare"</div>
                                        {move || if web.get() == Web::Authorized {
                                            view! {
                                                <div class="inline-flex items-center justify-center gap-1 text-[13px] font-semibold text-[#059669]">
                                                    <span class="material-symbols-outlined text-[16px]">"check_circle"</span>
                                                    "Authorized. You can close this window."
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="text-[13px] font-semibold">"Buwiz Bot wants to access your account"</div>
                                                <div class="grid gap-1 rounded-lg border border-[#e5e7eb] bg-[#f6f8fa] p-2.5 text-left text-[12px]">
                                                    {["Workers Scripts: edit", "Account settings: read", "D1 / KV: edit"].into_iter().map(|s| view! {
                                                        <span class="flex items-center gap-1.5">
                                                            <span class="material-symbols-outlined text-[14px] text-[#059669]">"check"</span>
                                                            {s}
                                                        </span>
                                                    }).collect_view()}
                                                </div>
                                                <div class="text-[12px] text-[#57606a]">"Signed in as codeitlikemiley@…"</div>
                                                <div node_ref=authorize class="rounded-lg bg-[#f38020] py-2 text-[13px] font-semibold text-white">"Authorize"</div>
                                            }.into_any()
                                        }}
                                    </div>
                                </div>
                            </div>
                        </Show>
                        <DemoPointer cursor=cur />
                    }
                >
                    <div class="max-w-[88%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg">"Deploy the invoice worker to Cloudflare"</div>
                    <div class="max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text">
                        {move || if ready.get() {
                            "Got Cloudflare as codeitlikemiley. Deploying the invoice worker now."
                        } else {
                            "I don't have Cloudflare yet. Add it and pick an account for me."
                        }}
                    </div>
                </AgentWindow>
            </div>
        </section>
    }
}
