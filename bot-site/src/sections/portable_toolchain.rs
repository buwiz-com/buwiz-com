use leptos::{ev, html, prelude::*};

use crate::components::agent_window::{AgentScreen, AgentWindow, GENIE, GENIE_RAIL, box_desktop};
use crate::components::demo_player::{DemoBar, DemoCursor, DemoPointer, PlayState, Timeline, autoplay_when_visible};
use crate::components::tool_icons::{IconSize, Tool, tool_icon};

/// Coming soon: the Portable Agent Toolchain, played in a chat with Genie. Every step is a
/// generative-UI card in the thread: the toolchain Genie picked, the build, the choices after
/// it, the registry, the move to the visitor's own computer. The window is the same NativeChat
/// window as Egress: rust-dev's desktop sits in the pane, its dock filling as tools install, and
/// grows to fill the window when the computer comes up and when it moves to the Mac. A cursor
/// drives it; a click on any live card takes over, and once the run ends the cards stay
/// clickable.
///
/// Every control is a drawn box, never a real input.
const STEPS: &[&str] = &["Ask", "Confirm", "Build", "Up", "Registry", "Move"];

const BOX: &str = "rust-dev";
const ASK: &str = "Make me a new computer for Rust development, with an office suite";
const PLAN_NOTE: &str =
    "Here's what rust-dev needs. I picked the Rust toolchain, a browser and an office suite. Confirm, or tap to change.";

const PLAN: [Tool; 11] = [
    Tool::Rustup,
    Tool::Rustc,
    Tool::Cargo,
    Tool::Analyzer,
    Tool::Terminal,
    Tool::Git,
    Tool::Chromium,
    Tool::Office,
    Tool::Pdf,
    Tool::Ssh,
    Tool::Python,
];
/// What Genie ticks for the request. The rest are offered, dashed.
const PICKED: [bool; 11] = [true, true, true, true, true, true, true, true, true, false, false];
/// The optional tile the cursor adds.
const SSH: usize = 9;

/// The PAT registry, verified first.
const REGISTRY: [(Tool, &str, bool); 4] = [
    (Tool::FreeCad, "3D CAD modeler", true),
    (Tool::Uv, "Python package manager", true),
    (Tool::LibreCad, "2D drafting", false),
    (Tool::Ftp, "FTP / SFTP client", false),
];

const LIFECYCLE: [&str; 6] = ["find", "get", "verify", "allow", "install", "run"];

const CHOICES: [&str; 3] = ["Add more from the registry", "Move it to my computer", "That's all"];
const ADD_MORE: usize = 0;
const MOVE: usize = 1;
const ALL_SET: usize = 2;

const YOUR_TURN: &str = "Your turn: the cards are clickable now. Or Replay.";

/// One capability through find → run.
const STEP_MS: u32 = 240;
/// From confirming or adding to the build starting.
const LEAD_MS: u32 = 300;
const MOVE_MS: u32 = 1400;
/// How long rust-dev's screen stays grown when it comes up.
const UP_HOLD: u32 = 1800;
/// From the build finishing to Genie asking what's next.
const UP_MS: u32 = UP_HOLD + 450;
/// After the move lands: the screen shrinks, then Genie asks what's next.
const MOVED_HOLD: u32 = 900;
const MOVED_MS: u32 = MOVED_HOLD + 450;
/// Seconds per capability, as the cards report it.
const SECS: f64 = 0.55;

const CARD: &str = "grid gap-3 rounded-2xl border border-gc-border bg-gc-bg-card p-3.5";
const HEAD: &str = "flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[13.5px] font-semibold text-gc-text";
const META: &str = "ml-auto font-mono text-[11.5px] font-medium text-gc-text-muted";

const TILE_ON: &str = "relative flex cursor-pointer flex-col items-center gap-1 rounded-xl border border-gc-accent/60 bg-gc-accent/[0.07] px-0.5 pt-2.5 pb-2 text-center outline-none transition-[opacity,border-color,background-color] duration-200 focus-visible:ring-2 focus-visible:ring-gc-brand";
const TILE_OFF: &str = "relative flex cursor-pointer flex-col items-center gap-1 rounded-xl border border-dashed border-gc-border-strong bg-gc-bg-elevated px-0.5 pt-2.5 pb-2 text-center opacity-45 outline-none transition-[opacity,border-color,background-color] duration-200 hover:opacity-70 focus-visible:ring-2 focus-visible:ring-gc-brand";

const PRIMARY: &str = "cursor-pointer rounded-full bg-gc-text px-3.5 py-1.5 text-[13px] font-semibold text-gc-bg outline-none focus-visible:ring-2 focus-visible:ring-gc-brand focus-visible:ring-offset-2 focus-visible:ring-offset-gc-bg-card";
const PRIMARY_OFF: &str = "cursor-default rounded-full bg-gc-text px-3.5 py-1.5 text-[13px] font-semibold text-gc-bg opacity-40";

const LIFE_ON: &str = "rounded-[5px] bg-gc-text px-[7px] py-0.5 text-gc-bg";
const LIFE_OFF: &str = "rounded-[5px] bg-gc-bg-muted px-[7px] py-0.5 text-gc-text-faint";

const OPT: &str = "cursor-pointer rounded-full border border-gc-border bg-gc-bg-card px-3.5 py-2 text-[13px] font-medium text-gc-text outline-none hover:border-gc-brand/40 focus-visible:ring-2 focus-visible:ring-gc-brand";
const OPT_PICKED: &str = "rounded-full border border-gc-text bg-gc-text px-3.5 py-2 text-[13px] font-medium text-gc-bg";
const OPT_OFF: &str = "rounded-full border border-gc-border bg-gc-bg-card px-3.5 py-2 text-[13px] font-medium text-gc-text opacity-45";

const BADGE: &str = "rounded-[5px] border border-gc-border px-1.5 font-mono text-[10px] font-semibold text-gc-text-muted";
const BADGE_OK: &str = "rounded-[5px] border border-gc-accent/50 px-1.5 font-mono text-[10px] font-semibold text-gc-accent";
const ADD: &str = "shrink-0 cursor-pointer rounded-full bg-gc-bg-muted px-3 py-1 text-[12px] font-semibold text-gc-text outline-none hover:bg-gc-bg-hover focus-visible:ring-2 focus-visible:ring-gc-brand";

const DEVICE: &str = "grid justify-items-center gap-1.5 rounded-xl border border-gc-border bg-gc-bg-elevated p-2.5";

#[derive(Clone)]
struct Msg {
    id: u32,
    body: Body,
}

#[derive(Clone)]
enum Body {
    Me(&'static str),
    /// Genie's words: a bold lead, then the rest.
    Genie(&'static str, String),
    Plan,
    Build(BuildCard),
    /// Picked choice, and which of `CHOICES` are offered.
    Choices(ArcRwSignal<Option<usize>>, &'static [usize]),
    Registry(ArcRwSignal<Reg>),
    /// Done yet, and what moves.
    Moved(ArcRwSignal<bool>, Vec<Tool>),
}

#[derive(Clone)]
struct BuildCard {
    label: String,
    tools: Vec<Tool>,
    /// How many are through.
    n: ArcRwSignal<usize>,
    /// The Box's dock once it's up.
    dock: ArcRwSignal<Option<Vec<Tool>>>,
}

/// The big line on rust-dev's screen.
#[derive(Clone, Copy, PartialEq)]
enum Note {
    None,
    Ready,
    Moving,
    Moved,
}

#[derive(Clone, Default)]
struct Reg {
    added: Option<Tool>,
    collapsed: bool,
}

/// The chat and everything the flows touch. The autoplay script and a visitor's clicks run the
/// same flows.
#[derive(Clone, Copy)]
struct Demo {
    tl: Timeline,
    cur: DemoCursor,
    msgs: RwSignal<Vec<Msg>>,
    next: StoredValue<u32>,
    step: RwSignal<Option<usize>>,
    caption: RwSignal<&'static str>,
    sel: RwSignal<[bool; 11]>,
    confirmed: RwSignal<Option<Vec<Tool>>>,
    query: RwSignal<&'static str>,
    installed: RwSignal<Vec<Tool>>,
    moved: RwSignal<bool>,
    screen: AgentScreen,
    note: RwSignal<Note>,
    scroller: NodeRef<html::Div>,
    // Where the cursor goes.
    ssh_tile: NodeRef<html::Div>,
    confirm_btn: NodeRef<html::Div>,
    opts: [NodeRef<html::Div>; 3],
    search: NodeRef<html::Div>,
    add_freecad: NodeRef<html::Div>,
}

impl Demo {
    fn reset(self) {
        self.msgs.set(Vec::new());
        self.sel.set(PICKED);
        self.confirmed.set(None);
        self.query.set("");
        self.installed.set(Vec::new());
        self.moved.set(false);
        self.note.set(Note::None);
        self.screen.reset();
        self.cur.reset();
    }

    fn push(self, body: Body) {
        let id = self
            .next
            .try_update_value(|n| {
                *n += 1;
                *n
            })
            .unwrap_or_default();
        self.msgs.update(|v| v.push(Msg { id, body }));
        self.stick();
    }

    fn say(self, lead: &'static str, text: impl Into<String>) {
        self.push(Body::Genie(lead, text.into()));
    }

    /// Keeps the chat scrolled to its newest card, once the DOM has it.
    fn stick(self) {
        let scroller = self.scroller;
        request_animation_frame(move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    fn step(self, n: usize, caption: &'static str) {
        self.step.set(Some(n));
        self.caption.set(caption);
    }

    fn chosen(self) -> Vec<Tool> {
        let sel = self.sel.get_untracked();
        PLAN.iter().zip(sel).filter(|(_, on)| *on).map(|(t, _)| *t).collect()
    }

    fn plan_open(self) -> bool {
        self.confirmed.with_untracked(Option::is_none)
    }

    /// A visitor's click. Mid-run it ends the autoplay and hands the chat over.
    fn by_hand(self) -> bool {
        match self.tl.state.get_untracked() {
            PlayState::Idle => false,
            PlayState::Done => true,
            PlayState::Playing | PlayState::Paused => {
                self.tl.stop();
                self.cur.reset();
                self.screen.reset();
                self.caption.set(YOUR_TURN);
                true
            }
        }
    }

    fn latest<T>(self, f: impl Fn(&Body) -> Option<T>) -> Option<T> {
        self.msgs.with_untracked(|v| v.iter().rev().find_map(|m| f(&m.body)))
    }

    fn ask(self) {
        self.push(Body::Me(ASK));
    }

    fn propose(self) {
        self.step(1, "Genie already picked the toolchain. Ticked are in, dashed are optional. Just confirm.");
        self.say("", PLAN_NOTE);
        self.push(Body::Plan);
    }

    fn toggle(self, i: usize) {
        if self.plan_open() {
            self.sel.update(|s| s[i] = !s[i]);
        }
    }

    fn confirm(self) {
        let tools = self.chosen();
        if !self.plan_open() || tools.is_empty() {
            return;
        }
        self.confirmed.set(Some(tools.clone()));
        self.step(2, "Builds in seconds: each capability goes find → verify → install. No image rebuild.");
        let n = tools.len();
        self.tl.after(LEAD_MS, move || {
            self.build(format!("Building {BOX}"), tools, move || {
                // The screen grows out of the pane: up, dock full. Then back.
                self.note.set(Note::Ready);
                self.screen.expand();
                self.tl.after(UP_HOLD, move || self.screen.shrink());
                self.tl.after(UP_MS, move || {
                    self.step(3, "Up. Genie asks what's next, right in the chat.");
                    self.say(BOX, format!(" is up with {n} capabilities. Anything else?"));
                    self.offer();
                });
            });
        });
    }

    /// A build card that steps through `tools`, then `done` once the Box is up.
    fn build(self, label: String, tools: Vec<Tool>, done: impl FnOnce() + 'static) {
        let n = ArcRwSignal::new(0);
        let dock = ArcRwSignal::new(None);
        self.push(Body::Build(BuildCard { label, tools: tools.clone(), n: n.clone(), dock: dock.clone() }));
        let len = tools.len();
        for i in 1..len {
            let n = n.clone();
            let t = tools[i - 1];
            // Each tool lands in rust-dev's dock as it's through.
            self.tl.after(i as u32 * STEP_MS, move || {
                n.set(i);
                self.install(t);
            });
        }
        self.tl.after(len as u32 * STEP_MS, move || {
            for t in tools {
                self.install(t);
            }
            n.set(len);
            dock.set(Some(self.installed.get_untracked()));
            done();
            self.stick();
        });
    }

    fn install(self, t: Tool) {
        if !self.installed.with_untracked(|v| v.contains(&t)) {
            self.installed.update(|v| v.push(t));
        }
    }

    fn offer(self) {
        let opts: &'static [usize] =
            if self.moved.get_untracked() { &[ADD_MORE, ALL_SET] } else { &[ADD_MORE, MOVE, ALL_SET] };
        self.push(Body::Choices(ArcRwSignal::new(None), opts));
    }

    fn pick(self, choice: ArcRwSignal<Option<usize>>, i: usize) {
        if choice.get_untracked().is_some() {
            return;
        }
        choice.set(Some(i));
        self.push(Body::Me(CHOICES[i]));
        match i {
            ADD_MORE => {
                self.step(4, "Search the PAT registry and add without leaving the chat.");
                self.query.set("");
                self.push(Body::Registry(ArcRwSignal::new(Reg::default())));
            }
            MOVE => {
                self.step(5, "Same capabilities move to your Mac. Nothing rebuilt.");
                self.moved.set(true);
                let done = ArcRwSignal::new(false);
                self.push(Body::Moved(done.clone(), self.installed.get_untracked()));
                // The screen grows to show the tools landing on the Mac, then shrinks back.
                self.note.set(Note::Moving);
                self.screen.expand();
                self.tl.after(MOVE_MS, move || {
                    done.set(true);
                    self.note.set(Note::Moved);
                });
                self.tl.after(MOVE_MS + MOVED_HOLD, move || self.screen.shrink());
                self.tl.after(MOVE_MS + MOVED_MS, move || {
                    self.say("", format!("Done. {BOX}'s tools are on Mac Studio too. Anything else?"));
                    self.offer();
                });
            }
            _ => self.say("", format!("Enjoy. I'll keep {BOX} warm.")),
        }
    }

    fn pick_latest(self, i: usize) {
        if let Some(c) = self.latest(|b| match b {
            Body::Choices(c, _) => Some(c.clone()),
            _ => None,
        }) {
            self.pick(c, i);
        }
    }

    fn add(self, reg: ArcRwSignal<Reg>, tool: Tool) {
        if reg.with_untracked(|r| r.added.is_some()) {
            return;
        }
        reg.update(|r| r.added = Some(tool));
        self.tl.after(LEAD_MS, move || {
            reg.update(|r| r.collapsed = true);
            self.build(format!("Installing {} on {BOX}", tool.name()), vec![tool], move || {
                self.say(tool.name(), " is ready. Anything else?");
                self.offer();
            });
        });
    }

    fn add_latest(self, tool: Tool) {
        if let Some(r) = self.latest(|b| match b {
            Body::Registry(r) => Some(r.clone()),
            _ => None,
        }) {
            self.add(r, tool);
        }
    }
}

/// Enter or Space on a drawn button.
fn activates(e: &ev::KeyboardEvent) -> bool {
    let hit = matches!(e.key().as_str(), "Enter" | " ");
    if hit {
        e.prevent_default();
    }
    hit
}

/// What a card leaves behind once it's done: a check, a line, and its tools.
fn done_chip(text: String, tools: Vec<Tool>) -> impl IntoView {
    view! {
        <div class="inline-flex max-w-full flex-wrap items-center gap-x-2 gap-y-1 self-start rounded-xl border border-gc-border px-3 py-1.5 text-[12.5px] text-gc-text-muted">
            <span class="font-semibold text-gc-accent">"✓"</span>
            <span>{text}</span>
            <span class="flex pl-1">
                {tools.into_iter().take(8).map(|t| view! {
                    <span class="-ml-1 rounded-[6px] ring-[1.5px] ring-gc-bg">{tool_icon(t, IconSize::Mini)}</span>
                }).collect_view()}
            </span>
        </div>
    }
}

fn tile(d: Demo, i: usize, t: Tool) -> impl IntoView {
    let on = move || d.sel.with(|s| s[i]);
    let flip = move || {
        if d.plan_open() && d.by_hand() {
            d.toggle(i);
        }
    };
    view! {
        <div
            node_ref={if i == SSH { d.ssh_tile } else { NodeRef::new() }}
            role="button"
            tabindex="0"
            aria-pressed=move || if on() { "true" } else { "false" }
            class=move || if on() { TILE_ON } else { TILE_OFF }
            on:click=move |_| flip()
            on:keydown=move |e| { if activates(&e) { flip() } }
        >
            {tool_icon(t, IconSize::Tile)}
            <span class="w-full truncate px-0.5 text-[12px] font-semibold text-gc-text">{t.name()}</span>
            <span class="w-full truncate px-0.5 font-mono text-[10.5px] text-gc-text-faint">{t.sub()}</span>
            <span class=move || if on() {
                "absolute right-[7px] top-[5px] text-[11px] font-bold leading-none text-gc-accent"
            } else {
                "absolute right-[7px] top-[4px] text-[14px] font-bold leading-none text-gc-brand"
            }>{move || if on() { "✓" } else { "+" }}</span>
        </div>
    }
}

/// The toolchain Genie picked. Collapses to a line once confirmed.
fn plan_card(d: Demo) -> impl IntoView {
    let count = move || d.sel.with(|s| s.iter().filter(|on| **on).count());
    let go = move || {
        if d.plan_open() && count() > 0 && d.by_hand() {
            d.confirm();
        }
    };
    view! {
        <div class="gc-card-in flex flex-col">
            {move || match d.confirmed.get() {
                Some(tools) => done_chip(format!("Confirmed · {} capabilities", tools.len()), tools).into_any(),
                None => view! {
                    <div class=CARD>
                        <div class=HEAD>
                            <span>"New computer · "<span class="font-mono font-medium">{BOX}</span></span>
                            <span class=META>{move || {
                                let n = count();
                                format!("{n} capabilities · ~{:.1}s", n as f64 * SECS)
                            }}</span>
                        </div>
                        <div class="grid grid-cols-3 gap-2 sm:grid-cols-[repeat(auto-fill,minmax(96px,1fr))]">
                            {PLAN.iter().enumerate().map(|(i, &t)| tile(d, i, t)).collect_view()}
                        </div>
                        <div class="flex flex-wrap items-center gap-2">
                            <span class="mr-auto font-mono text-[12px] text-gc-text-muted">"Ticked are in. Tap to drop or add."</span>
                            <div
                                node_ref=d.confirm_btn
                                role="button"
                                tabindex="0"
                                class={move || if count() > 0 { PRIMARY } else { PRIMARY_OFF }}
                                on:click=move |_| go()
                                on:keydown=move |e| { if activates(&e) { go() } }
                            >
                                "Confirm & build"
                            </div>
                        </div>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

/// The Box's desktop once it's up, with every installed tool in the dock. Its colors are the
/// Box's own.
fn desktop(dock: Vec<Tool>) -> impl IntoView {
    view! {
        <div
            class="relative flex min-h-24 flex-col overflow-hidden rounded-[10px]"
            style="background:radial-gradient(120% 100% at 20% 0%,#33476b 0%,#1a2140 55%,#16243f 100%)"
        >
            <div class="flex items-center justify-between gap-2 bg-black/40 px-2.5 py-1 text-[10.5px] text-[#e6e9f2]">
                <span>"Activities"</span>
                <span class="truncate">"Buwiz Bot Box · "{BOX}</span>
                <span>"●"</span>
            </div>
            <div class="flex-1"></div>
            <div class="mx-auto mb-2 mt-2 flex max-w-[calc(100%-16px)] flex-wrap justify-center gap-1 rounded-[10px] bg-[#0a0e1c]/70 p-1">
                {dock.into_iter().map(|t| tool_icon(t, IconSize::Dock)).collect_view()}
            </div>
        </div>
    }
}

/// rust-dev's screen in the pane: its dock fills as tools install, and a line across the
/// middle once it's up or moving. Laid out at its real size when grown.
fn rust_dev(d: Demo) -> impl IntoView {
    let note = move || match d.note.get() {
        Note::None => ().into_any(),
        Note::Ready => view! {
            <span class="text-[64px] font-black leading-none tracking-tight">"Ready."</span>
            <span class="text-[18px] text-white/75">{move || format!("{BOX} · {} capabilities", d.installed.with(Vec::len))}</span>
        }
        .into_any(),
        Note::Moving => view! {
            <span class="text-[44px] font-black leading-tight tracking-tight">"Moving to Mac Studio"</span>
            <span class="flex items-center gap-2 text-[18px] text-white/75">
                <span class="h-4 w-4 animate-spin rounded-full border-2 border-white/25 border-t-white"></span>
                "nothing rebuilt"
            </span>
        }
        .into_any(),
        Note::Moved => view! {
            <span class="text-[44px] font-black leading-tight tracking-tight">"✓ On Mac Studio too"</span>
            <span class="text-[18px] text-white/75">"nothing rebuilt"</span>
        }
        .into_any(),
    };
    box_desktop(
        "Buwiz Bot Box · rust-dev",
        (),
        view! { <div class="absolute inset-x-0 top-8 bottom-16 flex flex-col items-center justify-center gap-2 px-6 text-center text-white">{note}</div> },
        view! {
            <span class="h-[34px] w-0"></span>
            <For each=move || d.installed.get() key=|t| t.name() children=|t| view! { <span class="gc-card-in">{tool_icon(t, IconSize::Tile)}</span> } />
        },
    )
}

fn build_card(b: BuildCard) -> impl IntoView {
    let BuildCard { label, tools, n, dock } = b;
    let len = tools.len().max(1);
    let n = RwSignal::from(n);
    let dock = RwSignal::from(dock);
    let up = move || dock.with(Option::is_some);
    let secs = |k: usize| format!("{:.1}s", k as f64 * SECS);
    let life = move || if up() { 5 } else { (n.get() * 6 / len).min(5) };
    view! {
        <div class=format!("gc-card-in {CARD}")>
            <div class=HEAD>
                <span>{move || if up() { "✓ " } else { "" }}{label}</span>
                <span class=META>{move || if up() { format!("up in {}", secs(len)) } else { secs(n.get()) }}</span>
            </div>
            <div class="h-1.5 overflow-hidden rounded-full bg-gc-bg-muted">
                <div
                    class="h-full rounded-full bg-gc-brand transition-[width] duration-200"
                    style=move || format!("width:{}%", if up() { 100 } else { n.get() * 100 / len })
                ></div>
            </div>
            <div class="flex flex-wrap gap-1 font-mono text-[11px]">
                {LIFECYCLE.iter().enumerate().map(|(i, s)| view! {
                    <span class=move || if life() == i { LIFE_ON } else { LIFE_OFF }>{*s}</span>
                }).collect_view()}
            </div>
            <div class="flex flex-wrap gap-1.5">
                {tools.iter().enumerate().map(|(i, &t)| {
                    let ok = move || up() || i < n.get();
                    view! {
                        <span class=move || if ok() {
                            "relative rounded-[9px] transition-opacity duration-200"
                        } else if i == n.get() {
                            "relative rounded-[9px] ring-2 ring-gc-brand transition-opacity duration-200"
                        } else {
                            "relative rounded-[9px] opacity-30 transition-opacity duration-200"
                        }>
                            {tool_icon(t, IconSize::Tile)}
                            <span class=move || if ok() {
                                "absolute -bottom-1 -right-1 grid h-3.5 w-3.5 place-items-center rounded-full bg-gc-accent text-[9px] font-bold leading-none text-white"
                            } else {
                                "hidden"
                            }>"✓"</span>
                        </span>
                    }
                }).collect_view()}
            </div>
            {move || dock.get().map(desktop)}
        </div>
    }
}

/// What Genie offers next, as chips in the thread. One pick locks the rest.
fn choices(d: Demo, choice: ArcRwSignal<Option<usize>>, opts: &'static [usize]) -> impl IntoView {
    let picked = RwSignal::from(choice.clone());
    view! {
        <div class="gc-card-in flex flex-wrap gap-2 self-start">
            {opts.iter().map(|&i| {
                let pick = {
                    let choice = choice.clone();
                    move || {
                        if picked.get_untracked().is_none() && d.by_hand() {
                            d.pick(choice.clone(), i);
                        }
                    }
                };
                let pick2 = pick.clone();
                view! {
                    <div
                        node_ref=d.opts[i]
                        role="button"
                        tabindex=move || if picked.get().is_none() { "0" } else { "-1" }
                        class=move || match picked.get() {
                            None => OPT,
                            Some(k) if k == i => OPT_PICKED,
                            _ => OPT_OFF,
                        }
                        on:click=move |_| pick()
                        on:keydown=move |e| { if activates(&e) { pick2() } }
                    >
                        {CHOICES[i]}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// The PAT registry: a drawn search box over verified and community entries.
fn registry(d: Demo, reg: ArcRwSignal<Reg>) -> impl IntoView {
    let state = RwSignal::from(reg.clone());
    let rows = move || {
        let q = d.query.get();
        REGISTRY
            .iter()
            .filter(|(t, desc, _)| q.is_empty() || format!("{}{desc}", t.name()).to_lowercase().contains(q))
            .map(|&(t, desc, verified)| {
                let have = state.with(|r| r.added == Some(t)) || d.installed.with(|v| v.contains(&t));
                let add = {
                    let reg = reg.clone();
                    move || {
                        if state.with_untracked(|r| r.added.is_none()) && d.by_hand() {
                            d.add(reg.clone(), t);
                        }
                    }
                };
                let add2 = add.clone();
                view! {
                    <div class="flex items-center gap-2.5 rounded-xl border border-gc-border bg-gc-bg-elevated p-2">
                        {tool_icon(t, IconSize::Tile)}
                        <div class="min-w-0 flex-1">
                            <div class="flex flex-wrap items-center gap-1.5">
                                <span class="text-[13px] font-semibold text-gc-text">{t.name()}</span>
                                <span class=if verified { BADGE_OK } else { BADGE }>{if verified { "verified" } else { "community" }}</span>
                            </div>
                            <div class="truncate text-[11.5px] text-gc-text-muted">{desc}</div>
                        </div>
                        {if have {
                            view! { <span class="shrink-0 px-2 text-[12px] font-semibold text-gc-accent">"✓ Added"</span> }.into_any()
                        } else {
                            view! {
                                <div
                                    node_ref={if t == Tool::FreeCad { d.add_freecad } else { NodeRef::new() }}
                                    role="button"
                                    tabindex="0"
                                    class=ADD
                                    on:click=move |_| add()
                                    on:keydown=move |e| { if activates(&e) { add2() } }
                                >
                                    "Add"
                                </div>
                            }.into_any()
                        }}
                    </div>
                }
            })
            .collect_view()
    };
    view! {
        <div class="gc-card-in flex flex-col">
            {move || match state.with(|r| if r.collapsed { r.added } else { None }) {
                Some(t) => done_chip(format!("Added {} from the registry", t.name()), vec![t]).into_any(),
                None => view! {
                    <div class=CARD>
                        <div class=HEAD>
                            <span class="material-symbols-outlined text-[17px] text-gc-text-muted">"search"</span>
                            <span>"PAT registry"</span>
                            <span class=META>"verified first"</span>
                        </div>
                        <div node_ref=d.search class="flex min-h-[34px] items-center rounded-[10px] border border-gc-border bg-gc-bg-elevated px-3 py-1.5 text-[13px]">
                            {move || match d.query.get() {
                                "" => view! { <span class="text-gc-text-faint">"Search the registry"</span> }.into_any(),
                                q => view! { <span class="text-gc-text">{q}<span class="gc-caret"></span></span> }.into_any(),
                            }}
                        </div>
                        {rows.clone()}
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

/// Every capability moves from the Box to the visitor's Mac. Nothing is rebuilt.
fn move_card(done: ArcRwSignal<bool>, tools: Vec<Tool>) -> impl IntoView {
    let done = RwSignal::from(done);
    let arrived = tools.clone();
    view! {
        <div class=format!("gc-card-in {CARD}")>
            <div class=HEAD>
                <span>{move || if done.get() { "✓ Moved to Mac Studio" } else { "Moving to Mac Studio" }}</span>
                <span class=META>{move || if done.get() { "nothing rebuilt" } else { "…" }}</span>
            </div>
            <div class="grid grid-cols-1 items-center gap-2.5 text-center sm:grid-cols-[1fr_auto_1fr]">
                <div class=DEVICE>
                    <span class="text-[12.5px] font-semibold text-gc-text">{BOX}" · Box"</span>
                    <div class="flex min-h-[22px] flex-wrap justify-center gap-[3px]">
                        {tools.into_iter().map(|t| tool_icon(t, IconSize::Mini)).collect_view()}
                    </div>
                </div>
                <span class="material-symbols-outlined justify-self-center rotate-90 text-[20px] text-gc-brand sm:rotate-0">"arrow_forward"</span>
                <div class=DEVICE>
                    <span class="text-[12.5px] font-semibold text-gc-text">"Mac Studio"</span>
                    <div class="flex min-h-[22px] flex-wrap justify-center gap-[3px]">
                        {move || done.get().then(|| arrived.iter().map(|&t| tool_icon(t, IconSize::Mini)).collect_view())}
                    </div>
                </div>
            </div>
        </div>
    }
}

fn message(d: Demo, m: Msg) -> AnyView {
    match m.body {
        Body::Me(text) => view! {
            <div class="gc-card-in max-w-[88%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg">{text}</div>
        }
        .into_any(),
        Body::Genie(lead, text) => view! {
            <div class="gc-card-in max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text">
                <span class="font-semibold">{lead}</span>
                {text}
            </div>
        }
        .into_any(),
        Body::Plan => plan_card(d).into_any(),
        Body::Build(b) => build_card(b).into_any(),
        Body::Choices(c, opts) => choices(d, c, opts).into_any(),
        Body::Registry(r) => registry(d, r).into_any(),
        Body::Moved(done, tools) => move_card(done, tools).into_any(),
    }
}

fn point(title: &'static str, text: &'static str) -> impl IntoView {
    view! {
        <div class="grid content-start gap-1 rounded-2xl border border-gc-border bg-gc-bg p-4 text-[13.5px]">
            <span class="text-[15px] font-bold text-gc-text">{title}</span>
            <span class="text-gc-text-muted">{text}</span>
        </div>
    }
}

/// Coming soon: tools travel with the coworker between computers.
#[component]
pub fn PortableToolchain() -> impl IntoView {
    let win = NodeRef::<html::Div>::new();
    let composer = NodeRef::<html::Div>::new();
    let draft = RwSignal::new("");

    let tl = Timeline::new();
    let cur = DemoCursor::new(tl, win, (0.72, 0.8));
    let d = Demo {
        tl,
        cur,
        msgs: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
        step: RwSignal::new(None),
        caption: RwSignal::new(""),
        sel: RwSignal::new(PICKED),
        confirmed: RwSignal::new(None),
        query: RwSignal::new(""),
        installed: RwSignal::new(Vec::new()),
        moved: RwSignal::new(false),
        screen: AgentScreen::new(),
        note: RwSignal::new(Note::None),
        scroller: NodeRef::new(),
        ssh_tile: NodeRef::new(),
        confirm_btn: NodeRef::new(),
        opts: std::array::from_fn(|_| NodeRef::new()),
        search: NodeRef::new(),
        add_freecad: NodeRef::new(),
    };

    tl.script(move |s| {
        d.reset();
        draft.set("");
        d.step(0, "Ask for a computer in plain words.");
        // Genie's picks plus the ssh tile the cursor adds.
        let building = PICKED.iter().filter(|p| **p).count() as u32 + 1;

        s.wait(400)
            .move_to(cur, composer)
            .typing(ASK, 26, move |t| draft.set(t))
            .wait(300)
            .then(move || {
                draft.set("");
                d.ask();
            })
            .wait(500)
            .then(move || d.propose())
            .wait(1500)
            .click(cur, d.ssh_tile, move || d.toggle(SSH))
            .wait(400)
            .click(cur, d.confirm_btn, move || d.confirm())
            // The build runs, then the choices land; give them a beat.
            .wait(LEAD_MS + building * STEP_MS + UP_MS + 120)
            .click(cur, d.opts[ADD_MORE], move || d.pick_latest(ADD_MORE))
            .wait(400)
            .move_to(cur, d.search)
            .typing("cad", 140, move |q| d.query.set(q))
            .wait(400)
            .click(cur, d.add_freecad, move || d.add_latest(Tool::FreeCad))
            .wait(LEAD_MS + STEP_MS + 120)
            .click(cur, d.opts[MOVE], move || d.pick_latest(MOVE))
            .wait(MOVE_MS + MOVED_MS + 300)
            .then(move || d.caption.set(YOUR_TURN))
            .move_home(cur);
    });

    autoplay_when_visible(tl, win);

    view! {
        <section id="portable" class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <div class="text-[11px] font-bold tracking-wide uppercase text-gc-gold mb-3">"Coming soon"</div>
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "The Portable Agent Toolchain"
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[60ch]">
                        "All in the chat. Ask for a computer, Genie already knows what it needs, you confirm, it's up in seconds. Then add more from the PAT registry or move it to your own computer."
                    </p>
                </div>

                <DemoBar steps=STEPS current=d.step timeline=tl caption=d.caption />

                <AgentWindow
                    win
                    screen=d.screen
                    who=GENIE
                    rail=GENIE_RAIL
                    scroller=d.scroller
                    composer
                    draft
                    desktop=move || rust_dev(d)
                    routine=("Nightly cargo update", "Daily")
                    overlay=move || view! { <DemoPointer cursor=cur /> }
                >
                    <For each=move || d.msgs.get() key=|m| m.id children=move |m| message(d, m) />
                </AgentWindow>

                <div class="mt-6 grid grid-cols-1 gap-3 md:grid-cols-3">
                    {point("Generative UI, in the thread", "Every step is an AG-UI card in the chat: the plan, the build, the choices, the registry, the move. The computer itself shows in the pane on the right.")}
                    {point("Picked for you, you confirm", "Genie works out the toolchain from the request. Ticked tiles are in; dashed ones are optional. One click to confirm.")}
                    {point("Then grow or move it", "Choice chips after the build: add from the registry, or move every capability to your Mac. Nothing rebuilt either way.")}
                </div>
            </div>
        </section>
    }
}
