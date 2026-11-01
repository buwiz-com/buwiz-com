use leptos::{ev, html, prelude::*};

use crate::components::demo_player::{
    DemoBar, DemoCursor, DemoPointer, Script, Timeline, autoplay_when_visible, in_view,
};

/// Allowly: macOS permission prompts answered from the phone. A USB board on the Mac does the
/// real clicking and typing, because macOS ignores synthetic clicks on its own prompts. Plays
/// itself with two pointers: the visitor's on the phone, the board's on the Mac.
///
/// The Mac and the iPhone keep their own real-world colors in both site themes. Every control is
/// a drawn box, never a real input.
const STEPS: &[&str] =
    &["Automation", "Gatekeeper", "Accessibility", "Trackpad", "Password", "Full Disk Access", "All clear"];

const MAC_FONT: &str =
    "font-family:-apple-system,BlinkMacSystemFont,'SF Pro Text','Helvetica Neue',Helvetica,Arial,sans-serif;";
const WALL: &str = "background:radial-gradient(120% 90% at 18% 8%,#8a7cf0 0%,#4a43a8 38%,#1d2257 72%,#121634 100%);";
/// Where the board's pointer rests, as a fraction of the Mac's screen.
const BOARD_HOME: (f64, f64) = (0.86, 0.84);
/// What the visitor types on the phone. The Mac only ever shows dots.
const PASSWORD: &str = "sunrise";
const MAC_NAME: &str = "MacBook Pro";

#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    Automation,
    Gatekeeper,
    Accessibility,
}

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Accessibility,
    FullDisk,
}

#[derive(Clone, Copy, PartialEq)]
enum Folder {
    Applications,
    Utilities,
}

/// What the phone shows: request cards, the live screen with a trackpad, or the keyboard.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Cards,
    Live,
    Keys,
}

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Waiting,
    Pressing,
    Done,
}

#[derive(Clone, Copy, PartialEq)]
struct Card {
    app: &'static str,
    title: &'static str,
    sub: &'static str,
    no: &'static str,
    yes: &'static str,
    /// What the card says while the board works.
    doing: &'static str,
}

const AUTOMATION: Card = Card {
    app: "Terminal",
    title: "“Terminal” wants to control “Finder”",
    sub: "Automation prompt",
    no: "Don’t Allow",
    yes: "Allow",
    doing: "Board is pressing Allow on your Mac…",
};
const GATEKEEPER: Card = Card {
    app: "Finder",
    title: "Open “Figma”? It was downloaded from the Internet",
    sub: "Gatekeeper, first launch",
    no: "Cancel",
    yes: "Open",
    doing: "Board is pressing Open on your Mac…",
};
const ACCESSIBILITY: Card = Card {
    app: "NativeChat",
    title: "Turn on Accessibility for NativeChat",
    sub: "Needs System Settings + your password",
    no: "Deny",
    yes: "Take control",
    doing: "Board is opening System Settings…",
};
const FULL_DISK: Card = Card {
    app: "Terminal",
    title: "Give Terminal Full Disk Access",
    sub: "Not in the list yet: the board adds it with +",
    no: "Deny",
    yes: "Add & turn on",
    doing: "Board is adding Terminal…",
};

const APPLICATIONS: [(&str, Ico); 7] = [
    ("App Store", Ico::Plain("#1c8cf5", "A")),
    ("Calendar", Ico::Plain("#ff453a", "17")),
    ("Figma", Ico::Figma),
    ("Mail", Ico::Plain("#2f8cf6", "@")),
    ("NativeChat", Ico::NativeChat),
    ("Safari", Ico::Safari),
    ("Utilities", Ico::Folder),
];
const UTILITIES: [(&str, Ico); 6] = [
    ("Activity Monitor", Ico::Plain("#2d2d2f", "~")),
    ("Console", Ico::Plain("#3a3a3c", "!")),
    ("Disk Utility", Ico::Plain("#8e8e93", "◎")),
    ("Script Editor", Ico::Plain("#9a9aa0", "S")),
    ("System Information", Ico::Plain("#6e6e73", "i")),
    ("Terminal", Ico::Terminal),
];

#[derive(Clone, Copy)]
struct D {
    tl: Timeline,
    me: DemoCursor,
    board: DemoCursor,
    step: RwSignal<Option<usize>>,
    caption: RwSignal<&'static str>,
    // The Mac.
    app: RwSignal<&'static str>,
    term: RwSignal<(&'static str, &'static str)>,
    dialog: RwSignal<Option<Dialog>>,
    /// The Mac button the board is pressing.
    hot: RwSignal<&'static str>,
    settings: RwSignal<Option<Pane>>,
    nc_on: RwSignal<bool>,
    term_added: RwSignal<bool>,
    term_on: RwSignal<bool>,
    finder: RwSignal<Option<Folder>>,
    picked: RwSignal<bool>,
    /// The password prompt, and which settings it guards.
    sheet: RwSignal<Option<&'static str>>,
    dots: RwSignal<usize>,
    toast: RwSignal<Option<(&'static str, &'static str)>>,
    // The phone.
    mode: RwSignal<Mode>,
    card: RwSignal<Option<Card>>,
    ask: RwSignal<Ask>,
    /// The phone button the visitor is pressing.
    tapped: RwSignal<&'static str>,
    key: RwSignal<Option<char>>,
    typed: RwSignal<usize>,
    drag: RwSignal<bool>,
    trail: RwSignal<Vec<(f64, f64)>>,
    log: RwSignal<Vec<(&'static str, &'static str)>>,
    clear: RwSignal<bool>,
    // The live view on the phone mirrors the Mac at this scale.
    mac_size: RwSignal<(f64, f64)>,
    k: RwSignal<f64>,
    mac: NodeRef<html::Div>,
    mirror: NodeRef<html::Div>,
    // Where the board's pointer goes.
    m_yes: NodeRef<html::Div>,
    m_plus: NodeRef<html::Div>,
    m_util: NodeRef<html::Div>,
    m_term_file: NodeRef<html::Div>,
    m_open: NodeRef<html::Div>,
    sw_nc: NodeRef<html::Div>,
    sw_term: NodeRef<html::Div>,
    // Where the visitor's pointer goes.
    p_yes: NodeRef<html::Div>,
    pad_a: NodeRef<html::Div>,
    pad_b: NodeRef<html::Div>,
    p_kb: NodeRef<html::Div>,
    p_ret: NodeRef<html::Div>,
    keys: [NodeRef<html::Div>; 26],
}

impl D {
    fn reset(self) {
        self.app.set("Finder");
        self.term.set(("rg invoices ~/Library/Mail", ""));
        self.dialog.set(None);
        self.hot.set("");
        self.settings.set(None);
        self.nc_on.set(false);
        self.term_added.set(false);
        self.term_on.set(false);
        self.finder.set(None);
        self.picked.set(false);
        self.sheet.set(None);
        self.dots.set(0);
        self.toast.set(None);
        self.mode.set(Mode::Cards);
        self.card.set(None);
        self.ask.set(Ask::Waiting);
        self.tapped.set("");
        self.key.set(None);
        self.typed.set(0);
        self.drag.set(false);
        self.trail.set(Vec::new());
        self.log.set(Vec::new());
        self.clear.set(false);
        self.me.reset();
        self.board.reset();
    }

    fn step(self, n: usize, caption: &'static str) {
        self.step.set(Some(n));
        self.caption.set(caption);
    }

    fn request(self, c: Card) {
        self.ask.set(Ask::Waiting);
        self.card.set(Some(c));
    }

    fn key_ref(self, c: char) -> NodeRef<html::Div> {
        self.keys[(c as u8 - b'a') as usize]
    }
}

/// The visitor taps a phone button: press, then `f`.
fn tap(s: &mut Script, d: D, aim: NodeRef<html::Div>, label: &'static str, f: impl FnOnce() + 'static) {
    s.move_to(d.me, aim)
        .wait(120)
        .press(d.me)
        .then(move || d.tapped.set(label))
        .wait(180)
        .then(move || {
            d.tapped.set("");
            f();
        })
        .wait(250);
}

/// The board clicks a Mac button: glide, press, the button darkens, then `f`.
fn board_click(s: &mut Script, d: D, aim: NodeRef<html::Div>, label: &'static str, f: impl FnOnce() + 'static) {
    s.move_to(d.board, aim)
        .wait(140)
        .press(d.board)
        .then(move || d.hot.set(label))
        .wait(220)
        .then(move || {
            d.hot.set("");
            f();
        })
        .wait(260);
}

/// The visitor types `PASSWORD` key by key; each key lands on the Mac as a dot.
fn type_password(s: &mut Script, d: D, per_key: u32) {
    for c in PASSWORD.chars() {
        s.move_in(d.me, d.key_ref(c), per_key)
            .wait(40)
            .press(d.me)
            .then(move || {
                d.key.set(Some(c));
                d.typed.update(|n| *n += 1);
            })
            .wait(70)
            .then(move || d.dots.update(|n| *n += 1))
            .wait(80)
            .then(move || d.key.set(None));
    }
}

/// A modal prompt: it appears on the Mac and the phone, the visitor taps the answer, the board
/// clicks it on the Mac.
fn modal(
    s: &mut Script,
    d: D,
    dialog: Dialog,
    card: Card,
    toast: (&'static str, &'static str),
    log: (&'static str, &'static str),
) {
    s.wait(450).then(move || d.dialog.set(Some(dialog))).wait(650).then(move || d.request(card)).wait(900);
    tap(s, d, d.p_yes, card.yes, move || d.ask.set(Ask::Pressing));
    s.wait(150);
    board_click(s, d, d.m_yes, card.yes, move || {
        d.dialog.set(None);
        d.toast.set(Some(toast));
        d.ask.set(Ask::Done);
    });
    s.wait(1000)
        .then(move || {
            d.card.set(None);
            d.log.update(|v| v.push(log));
        })
        .wait(900)
        .then(move || d.toast.set(None));
}

#[derive(Clone, Copy, PartialEq)]
enum Ico {
    Terminal,
    Figma,
    NativeChat,
    Access,
    Settings,
    Safari,
    Allowly,
    Folder,
    /// A plain tile: color and a glyph.
    Plain(&'static str, &'static str),
}

/// App icons drawn as SVG, so they scale from a list row to a dialog.
fn icon(i: Ico, class: &'static str) -> AnyView {
    match i {
        Ico::Terminal => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#1f1f1f" stroke="#5a5a5a" stroke-width="1"></rect>
                <text x="7.5" y="25" font-family="ui-monospace,Menlo,monospace" font-size="13" font-weight="700" fill="#d8d8d8">">_"</text>
            </svg>
        }
        .into_any(),
        Ico::Figma => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#1e1e1e"></rect>
                <g transform="translate(12.4 8.6) scale(0.4)">
                    <path d="M19 28.5a9.5 9.5 0 1 1 19 0 9.5 9.5 0 0 1-19 0z" fill="#1ABCFE"></path>
                    <path d="M0 47.5A9.5 9.5 0 0 1 9.5 38H19v9.5a9.5 9.5 0 1 1-19 0z" fill="#0ACF83"></path>
                    <path d="M19 0v19h9.5a9.5 9.5 0 1 0 0-19H19z" fill="#FF7262"></path>
                    <path d="M0 9.5A9.5 9.5 0 0 0 9.5 19H19V0H9.5A9.5 9.5 0 0 0 0 9.5z" fill="#F24E1E"></path>
                    <path d="M0 28.5A9.5 9.5 0 0 0 9.5 38H19V19H9.5A9.5 9.5 0 0 0 0 28.5z" fill="#A259FF"></path>
                </g>
            </svg>
        }
        .into_any(),
        Ico::NativeChat => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#6d5cf0"></rect>
                <path d="M11 11h18a3 3 0 0 1 3 3v9a3 3 0 0 1-3 3h-9l-5.5 4.5V26H11a3 3 0 0 1-3-3v-9a3 3 0 0 1 3-3z" fill="#fff"></path>
            </svg>
        }
        .into_any(),
        Ico::Access => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#1f7aec"></rect>
                <circle cx="20" cy="11" r="3" fill="#fff"></circle>
                <path d="M10.5 16.5l9.5 2 9.5-2M20 18.5v6.5l-4.5 8.5M20 25l4.5 8.5" fill="none" stroke="#fff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"></path>
            </svg>
        }
        .into_any(),
        Ico::Settings => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#8e8e93"></rect>
                <circle cx="20" cy="20" r="10" fill="none" stroke="#f2f2f7" stroke-width="4.5" stroke-dasharray="2.6 2.2"></circle>
                <circle cx="20" cy="20" r="7" fill="none" stroke="#f2f2f7" stroke-width="2.4"></circle>
                <circle cx="20" cy="20" r="2.6" fill="#f2f2f7"></circle>
            </svg>
        }
        .into_any(),
        Ico::Safari => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#fff" stroke="#dcdce0" stroke-width="1"></rect>
                <circle cx="20" cy="20" r="14" fill="#1d8bf1"></circle>
                <polygon points="20,8.5 23,20 17,20" fill="#ff3b30"></polygon>
                <polygon points="17,20 23,20 20,31.5" fill="#fff"></polygon>
            </svg>
        }
        .into_any(),
        Ico::Allowly => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill="#16a34a"></rect>
                <path d="M12 21l5.5 5.5L28.5 14.5" fill="none" stroke="#fff" stroke-width="3.6" stroke-linecap="round" stroke-linejoin="round"></path>
            </svg>
        }
        .into_any(),
        Ico::Folder => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <path d="M3 11a3 3 0 0 1 3-3h9l3 3h16a3 3 0 0 1 3 3v17a3 3 0 0 1-3 3H6a3 3 0 0 1-3-3z" fill="#1e8cf0"></path>
                <path d="M3 15h34v16a3 3 0 0 1-3 3H6a3 3 0 0 1-3-3z" fill="#5ab4f7"></path>
            </svg>
        }
        .into_any(),
        Ico::Plain(color, glyph) => view! {
            <svg viewBox="0 0 40 40" class=class aria-hidden="true">
                <rect x="1" y="1" width="38" height="38" rx="9" fill=color></rect>
                <text x="20" y="26.5" text-anchor="middle" font-family="-apple-system,Helvetica,Arial,sans-serif" font-size="17" font-weight="700" fill="#fff">{glyph}</text>
            </svg>
        }
        .into_any(),
    }
}

fn apple() -> impl IntoView {
    view! {
        <svg viewBox="0 0 24 24" class="h-3.5 w-3.5 shrink-0 fill-current" aria-hidden="true">
            <path d="M17.05 20.28c-.98.95-2.05.8-3.08.35-1.09-.46-2.09-.48-3.24 0-1.44.62-2.2.44-3.06-.35C2.79 15.25 3.51 7.59 9.05 7.31c1.35.07 2.29.74 3.08.8 1.18-.24 2.31-.93 3.57-.84 1.51.12 2.65.72 3.4 1.8-3.12 1.87-2.38 5.98.48 7.13-.57 1.5-1.31 2.99-2.54 4.09zM12.03 7.25c-.15-2.23 1.66-4.07 3.74-4.25.29 2.58-2.34 4.5-3.74 4.25z"></path>
        </svg>
    }
}

fn lights() -> impl IntoView {
    view! {
        <span class="flex shrink-0 gap-[5px]">
            <span class="h-[9px] w-[9px] rounded-full bg-[#ff5f57]"></span>
            <span class="h-[9px] w-[9px] rounded-full bg-[#febc2e]"></span>
            <span class="h-[9px] w-[9px] rounded-full bg-[#28c840]"></span>
        </span>
    }
}

/// A macOS push button. Darkens while the board presses it.
fn mac_btn(d: D, label: &'static str, primary: bool, aim: NodeRef<html::Div>) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || match (primary, d.hot.get() == label) {
                (true, false) => "rounded-[6px] bg-[#0a84ff] whitespace-nowrap px-1.5 py-[4px] text-center text-[10.5px] sm:px-2 sm:text-[11.5px] font-medium text-white shadow-sm",
                (true, true) => "rounded-[6px] bg-[#0060cc] whitespace-nowrap px-1.5 py-[4px] text-center text-[10.5px] sm:px-2 sm:text-[11.5px] font-medium text-white shadow-sm",
                (false, false) => "rounded-[6px] bg-white whitespace-nowrap px-1.5 py-[4px] text-center text-[10.5px] sm:px-2 sm:text-[11.5px] text-[#1d1d1f] shadow-[0_0.5px_1.5px_rgba(0,0,0,.25)]",
                (false, true) => "rounded-[6px] bg-[#d6d6db] whitespace-nowrap px-1.5 py-[4px] text-center text-[10.5px] sm:px-2 sm:text-[11.5px] text-[#1d1d1f] shadow-[0_0.5px_1.5px_rgba(0,0,0,.25)]",
            }
        >
            {label}
        </div>
    }
}

/// A macOS switch, green when on.
fn mac_switch(on: Signal<bool>, aim: NodeRef<html::Div>) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || if on.get() {
                "relative ml-auto h-[17px] w-[30px] shrink-0 rounded-full bg-[#34c759] transition-colors duration-200"
            } else {
                "relative ml-auto h-[17px] w-[30px] shrink-0 rounded-full bg-[#e2e2e7] shadow-[inset_0_0_0_1px_rgba(0,0,0,.06)] transition-colors duration-200"
            }
        >
            <span class=move || if on.get() {
                "absolute left-[14px] top-px h-[15px] w-[15px] rounded-full bg-white shadow-[0_1px_2px_rgba(0,0,0,.3)] transition-[left] duration-200"
            } else {
                "absolute left-px top-px h-[15px] w-[15px] rounded-full bg-white shadow-[0_1px_2px_rgba(0,0,0,.3)] transition-[left] duration-200"
            }></span>
        </div>
    }
}

fn alert(d: D, kind: Dialog, live: bool) -> impl IntoView {
    let (ico, title, body, no, yes, stacked) = match kind {
        Dialog::Automation => (
            Ico::Terminal,
            "“Terminal” wants access to control “Finder”.",
            "Allowing control will provide access to documents and data in “Finder”, and to perform actions within that app.",
            "Don’t Allow",
            "Allow",
            false,
        ),
        Dialog::Gatekeeper => (
            Ico::Figma,
            "“Figma” is an app downloaded from the Internet. Are you sure you want to open it?",
            "Safari downloaded this file today at 9:40 AM from figma.com.",
            "Cancel",
            "Open",
            false,
        ),
        Dialog::Accessibility => (
            Ico::Access,
            "“NativeChat” would like to control this computer using accessibility features.",
            "Grant access to this application in Privacy & Security settings, located in System Settings.",
            "Deny",
            "Open System Settings",
            true,
        ),
    };
    let yes_ref = if live { d.m_yes } else { NodeRef::new() };
    view! {
        <div class="mac-pop absolute left-1/2 top-1/2 z-10 w-[214px] -translate-x-1/2 -translate-y-1/2 rounded-[12px] bg-[#ececec]/95 p-3 text-center text-[#1d1d1f] shadow-[0_20px_60px_rgba(0,0,0,.5)] ring-1 ring-black/10 backdrop-blur-xl sm:w-[260px] sm:p-4">
            <div class="mx-auto mb-2 h-10 w-10 sm:h-12 sm:w-12">{icon(ico, "h-full w-full drop-shadow")}</div>
            <div class="text-[11.5px] font-bold leading-snug sm:text-[12.5px]">{title}</div>
            <div class="mt-1 hidden text-[10.5px] leading-snug text-black/60 sm:block">{body}</div>
            {if stacked {
                view! {
                    <div class="mt-3 flex flex-col gap-1.5">
                        {mac_btn(d, yes, true, yes_ref)}
                        {mac_btn(d, no, false, NodeRef::new())}
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div class="mt-3 grid grid-cols-2 gap-2">
                        {mac_btn(d, no, false, NodeRef::new())}
                        {mac_btn(d, yes, true, yes_ref)}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

fn sidebar_row(name: &'static str, color: &'static str, glyph: &'static str, on: bool) -> impl IntoView {
    view! {
        <div class=if on {
            "flex items-center gap-2 rounded-[6px] bg-[#0a84ff] px-1.5 py-[3px] text-white"
        } else {
            "flex items-center gap-2 rounded-[6px] px-1.5 py-[3px]"
        }>
            <span class="grid h-[17px] w-[17px] shrink-0 place-items-center rounded-[5px] text-white" style=format!("background:{color}")>
                <span class="material-symbols-outlined text-[12px]">{glyph}</span>
            </span>
            <span class="truncate">{name}</span>
        </div>
    }
}

fn app_row(ico: Ico, name: &'static str, on: Signal<bool>, aim: NodeRef<html::Div>) -> impl IntoView {
    view! {
        <div class="flex h-[30px] items-center gap-2.5 border-b border-black/[.06] px-2.5 text-[11.5px] sm:h-[34px]">
            {icon(ico, "h-5 w-5 shrink-0")}
            <span class="truncate">{name}</span>
            {mac_switch(on, aim)}
        </div>
    }
}

fn settings_window(d: D, pane: Pane, live: bool) -> impl IntoView {
    let r = move |n: NodeRef<html::Div>| if live { n } else { NodeRef::new() };
    let (title, desc) = match pane {
        Pane::Accessibility => ("Accessibility", "Allow the applications below to control your computer."),
        Pane::FullDisk => (
            "Full Disk Access",
            "Allow the applications below to access data like Mail, Messages, Safari, Home, Time Machine backups, and certain administrative settings for all users on this Mac.",
        ),
    };
    let rows = match pane {
        Pane::Accessibility => view! {
            {app_row(Ico::Terminal, "Terminal", Signal::stored(true), NodeRef::new())}
            {app_row(Ico::NativeChat, "NativeChat", d.nc_on.into(), r(d.sw_nc))}
        }
        .into_any(),
        Pane::FullDisk => view! {
            {app_row(Ico::Safari, "Safari", Signal::stored(true), NodeRef::new())}
            <Show when=move || d.term_added.get()>
                <div class="gc-card-in">{app_row(Ico::Terminal, "Terminal", d.term_on.into(), r(d.sw_term))}</div>
            </Show>
        }
        .into_any(),
    };
    view! {
        <div class="mac-pop absolute inset-x-[3%] bottom-[4%] top-[10%] z-[6] flex overflow-hidden rounded-[12px] bg-[#f5f5f7] text-[#1d1d1f] shadow-[0_24px_70px_rgba(0,0,0,.5)] ring-1 ring-black/15 sm:inset-x-[8%] sm:bottom-[6%] sm:top-[11%]">
            <div class="relative hidden w-[30%] max-w-[200px] shrink-0 flex-col gap-px border-r border-black/[.08] bg-[#e8e8ed] px-2 pb-2 pt-9 text-[11px] sm:flex">
                <div class="absolute left-3 top-3">{lights()}</div>
                <div class="mb-1.5 flex h-6 items-center gap-1 rounded-[6px] bg-black/[.06] px-2 text-[10.5px] text-black/40">
                    <span class="material-symbols-outlined text-[13px]">"search"</span>
                    "Search"
                </div>
                {sidebar_row("Wi‑Fi", "#0a84ff", "wifi", false)}
                {sidebar_row("Bluetooth", "#0a84ff", "bluetooth", false)}
                {sidebar_row("Network", "#0a84ff", "language", false)}
                <div class="my-1 h-px bg-black/[.06]"></div>
                {sidebar_row("General", "#8e8e93", "settings", false)}
                {sidebar_row("Accessibility", "#1f7aec", "accessibility_new", false)}
                {sidebar_row("Appearance", "#1d1d1f", "contrast", false)}
                {sidebar_row("Privacy & Security", "#3478f6", "back_hand", true)}
                {sidebar_row("Desktop & Dock", "#1d1d1f", "dock_to_bottom", false)}
                {sidebar_row("Displays", "#0a84ff", "light_mode", false)}
            </div>
            <div class="relative flex min-w-0 flex-1 flex-col">
                <div class="flex h-9 shrink-0 items-center gap-1 border-b border-black/[.06] px-3">
                    <span class="mr-1.5 sm:hidden">{lights()}</span>
                    <span class="material-symbols-outlined text-[17px] text-black/40">"chevron_left"</span>
                    <span class="material-symbols-outlined text-[17px] text-black/20">"chevron_right"</span>
                    <span class="ml-1 truncate text-[12.5px] font-bold">{title}</span>
                </div>
                <div class="min-h-0 flex-1 overflow-hidden p-3 sm:p-4">
                    <div class="mb-2.5 text-[10.5px] leading-snug text-black/55">{desc}</div>
                    <div class="overflow-hidden rounded-[10px] bg-white ring-1 ring-black/[.07]">
                        {rows}
                        <div class="flex items-center gap-px px-1.5 py-1 text-[13px] text-black/60">
                            <div
                                node_ref=r(d.m_plus)
                                class=move || if d.hot.get() == "+" {
                                    "grid h-5 w-6 place-items-center rounded-[4px] bg-black/15 leading-none"
                                } else {
                                    "grid h-5 w-6 place-items-center rounded-[4px] leading-none"
                                }
                            >
                                "+"
                            </div>
                            <div class="grid h-5 w-6 place-items-center leading-none text-black/25">"−"</div>
                        </div>
                    </div>
                </div>
            </div>
            {move || d.finder.get().map(|f| finder_sheet(d, f, live))}
        </div>
    }
}

/// The open panel the + button drops from the title bar.
fn finder_sheet(d: D, folder: Folder, live: bool) -> impl IntoView {
    let r = move |n: NodeRef<html::Div>| if live { n } else { NodeRef::new() };
    let (title, items): (&str, &[(&str, Ico)]) = match folder {
        Folder::Applications => ("Applications", &APPLICATIONS),
        Folder::Utilities => ("Utilities", &UTILITIES),
    };
    view! {
        <div class="mac-sheet absolute inset-x-[3%] top-9 z-[8] flex h-[86%] flex-col sm:h-[80%] overflow-hidden rounded-b-[10px] bg-white shadow-[0_18px_50px_rgba(0,0,0,.35)] ring-1 ring-black/10 sm:inset-x-[9%]">
            <div class="flex h-8 shrink-0 items-center gap-1.5 border-b border-black/[.07] bg-[#f6f6f6] px-2 text-[11.5px]">
                <span class="material-symbols-outlined text-[16px] text-black/40">"chevron_left"</span>
                <span class="material-symbols-outlined text-[16px] text-black/20">"chevron_right"</span>
                {icon(Ico::Folder, "h-4 w-4 shrink-0")}
                <span class="font-semibold">{title}</span>
                <span class="material-symbols-outlined ml-auto text-[15px] text-black/40">"search"</span>
            </div>
            <div class="flex min-h-0 flex-1">
                <div class="hidden w-[28%] shrink-0 flex-col gap-px bg-[#f3f3f5] p-1.5 text-[10.5px] sm:flex">
                    <div class="px-1.5 pb-0.5 pt-1 text-[9.5px] font-semibold text-black/35">"Favorites"</div>
                    {[("AirDrop", "wifi_tethering", false), ("Recents", "schedule", false), ("Applications", "apps", true), ("Desktop", "desktop_mac", false), ("Documents", "description", false), ("Downloads", "download", false)]
                        .into_iter()
                        .map(|(n, g, on)| view! {
                            <div class=if on { "flex items-center gap-1.5 rounded-[5px] bg-black/[.08] px-1.5 py-[3px]" } else { "flex items-center gap-1.5 rounded-[5px] px-1.5 py-[3px]" }>
                                <span class="material-symbols-outlined text-[13px] text-[#0a84ff]">{g}</span>
                                <span class="truncate">{n}</span>
                            </div>
                        })
                        .collect_view()}
                </div>
                <div class="min-w-0 flex-1 overflow-hidden p-1 text-[11px]">
                    {items.iter().map(|&(name, ico)| {
                        let aim = match name {
                            "Utilities" => r(d.m_util),
                            "Terminal" => r(d.m_term_file),
                            _ => NodeRef::new(),
                        };
                        view! {
                            <div
                                node_ref=aim
                                class=move || {
                                    let sel = (name == "Terminal" && d.picked.get()) || d.hot.get() == name;
                                    if sel {
                                        "flex h-[18px] items-center gap-2 rounded-[5px] bg-[#0a84ff] px-2 text-white sm:h-[22px]"
                                    } else {
                                        "flex h-[18px] items-center gap-2 rounded-[5px] px-2 sm:h-[22px]"
                                    }
                                }
                            >
                                {icon(ico, "h-4 w-4 shrink-0")}
                                <span class="truncate">{name}</span>
                                {(name == "Utilities").then(|| view! { <span class="material-symbols-outlined ml-auto text-[13px] opacity-50">"chevron_right"</span> })}
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
            <div class="flex shrink-0 justify-end gap-2 border-t border-black/[.07] px-2.5 py-1.5">
                <div class="w-[64px]">{mac_btn(d, "Cancel", false, NodeRef::new())}</div>
                <div class=move || if d.picked.get() { "w-[64px]" } else { "w-[64px] opacity-50" }>{mac_btn(d, "Open", true, r(d.m_open))}</div>
            </div>
        </div>
    }
}

fn password_dialog(d: D, what: &'static str) -> impl IntoView {
    view! {
        <div class="mac-pop absolute left-1/2 top-1/2 z-[11] w-[220px] -translate-x-1/2 -translate-y-1/2 rounded-[12px] bg-[#ececec]/[.97] p-3 text-center text-[#1d1d1f] shadow-[0_20px_60px_rgba(0,0,0,.5)] ring-1 ring-black/10 backdrop-blur-xl sm:w-[270px] sm:p-4">
            <div class="relative mx-auto mb-2 h-10 w-10 sm:h-12 sm:w-12">
                {icon(Ico::Settings, "h-full w-full drop-shadow")}
                <span class="absolute -bottom-1 -right-1 grid h-5 w-5 place-items-center rounded-full bg-white shadow">
                    <span class="material-symbols-outlined text-[13px] text-[#1d1d1f]">"lock"</span>
                </span>
            </div>
            <div class="text-[11.5px] font-bold leading-snug sm:text-[12.5px]">{format!("System Settings is trying to modify your {what} settings.")}</div>
            <div class="mt-1 text-[10.5px] text-black/60">"Enter your password to allow this."</div>
            <div class="mt-2.5 hidden h-[22px] items-center rounded-[5px] border border-black/15 bg-white px-2 text-left text-[11px] text-black/80 sm:flex">"Goldcoders"</div>
            <div class="mt-1.5 flex h-[22px] items-center rounded-[5px] border-2 border-[#0a84ff]/60 bg-white px-2 text-left text-[11px] tracking-[2px] text-black">
                {move || "•".repeat(d.dots.get())}
                <span class="ml-px h-3 w-px animate-pulse bg-black"></span>
            </div>
            <div class="mt-3 grid grid-cols-2 gap-2">
                {mac_btn(d, "Cancel", false, NodeRef::new())}
                {mac_btn(d, "Modify Settings", true, NodeRef::new())}
            </div>
        </div>
    }
}

/// The Mac's screen. `live` is the real one with the board's pointer; the phone shows a mirror.
fn mac_screen(d: D, live: bool) -> impl IntoView {
    let at = d.board.at();
    view! {
        // Menu bar
        <div class="absolute inset-x-0 top-0 z-[5] flex h-6 items-center gap-3 bg-black/25 px-3 text-[11.5px] text-white backdrop-blur-md sm:gap-4">
            {apple()}
            <span class="font-semibold">{move || d.app.get()}</span>
            <span class="hidden sm:inline">"File"</span>
            <span class="hidden sm:inline">"Edit"</span>
            <span class="hidden md:inline">"View"</span>
            <span class="hidden md:inline">"Window"</span>
            <span class="hidden md:inline">"Help"</span>
            <span class="ml-auto flex items-center gap-2.5">
                <span class="material-symbols-outlined text-[14px]">"wifi"</span>
                <span class="material-symbols-outlined hidden text-[14px] sm:inline">"toggle_on"</span>
                <span>"Tue 9:41"</span>
            </span>
        </div>

        // The coworker's terminal, behind everything
        <div class="absolute left-[5%] top-[13%] h-[42%] w-[60%] overflow-hidden rounded-[10px] bg-[#1e1e1e]/95 shadow-xl ring-1 ring-white/10 sm:w-[50%]">
            <div class="flex h-5 items-center gap-1.5 bg-[#2d2d2d] px-2">
                {lights()}
                <span class="mx-auto pr-8 text-[9.5px] text-white/50">"coworker — zsh"</span>
            </div>
            <div class="p-2.5 font-mono text-[9.5px] leading-relaxed text-[#9fe59f] sm:text-[10.5px]">
                <div class="truncate">"coworker@mac ~ % "{move || d.term.get().0}</div>
                <div class="text-white/50">{move || d.term.get().1}</div>
            </div>
        </div>

        {move || d.settings.get().map(|p| settings_window(d, p, live))}
        {move || d.dialog.get().map(|k| alert(d, k, live))}
        {move || d.sheet.get().map(|w| password_dialog(d, w))}

        {move || d.toast.get().map(|(t, b)| view! {
            <div class="mac-toast absolute right-2 top-8 z-[12] flex w-[min(290px,80%)] gap-2.5 rounded-[14px] bg-[#f6f6f8]/90 p-2.5 text-[#1d1d1f] shadow-[0_10px_30px_rgba(0,0,0,.35)] ring-1 ring-black/10 backdrop-blur-xl">
                {icon(Ico::Allowly, "h-8 w-8 shrink-0")}
                <div class="min-w-0 flex-1">
                    <div class="flex items-baseline justify-between gap-2">
                        <span class="truncate text-[11.5px] font-bold">{t}</span>
                        <span class="shrink-0 text-[10px] text-black/40">"now"</span>
                    </div>
                    <div class="text-[10.5px] leading-snug text-black/65">{b}</div>
                </div>
            </div>
        })}

        {if live {
            view! { <DemoPointer cursor=d.board tag="USB board" /> }.into_any()
        } else {
            // The board's pointer, drawn big enough to read at the mirror's scale.
            view! {
                <svg
                    viewBox="0 0 16 24"
                    class="pointer-events-none absolute left-0 top-0 z-30"
                    style=move || {
                        let (w, h) = d.mac_size.get();
                        let (x, y) = at.get().unwrap_or((BOARD_HOME.0 * w, BOARD_HOME.1 * h));
                        let s = 1. / d.k.get().max(0.2);
                        format!("width:{:.1}px;height:{:.1}px;transform:translate({x:.1}px,{y:.1}px)", 13. * s, 19.5 * s)
                    }
                    aria-hidden="true"
                >
                    <path d="M1 1 L1 19 L6 14 L9.5 22 L12.5 20.7 L9 13 L15 13 Z" fill="white" stroke="black" stroke-width="1.2"></path>
                </svg>
            }
            .into_any()
        }}
    }
}

const PHONE_SEC: &str = "rounded-xl bg-white/10 py-2.5 text-center text-[14px] font-semibold text-white";
const PHONE_PRI: &str =
    "rounded-xl bg-[#0a84ff] py-2.5 text-center text-[14px] font-semibold text-white transition-transform duration-100";
const PHONE_PRI_DOWN: &str = "scale-[.97] rounded-xl bg-[#0062cc] py-2.5 text-center text-[14px] font-semibold text-white transition-transform duration-100";

fn card_head(app: &'static str, tag: &'static str) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-2">
            <span class="truncate text-[11px] text-white/50">{app}" · "{MAC_NAME}</span>
            <span class="shrink-0 rounded-md bg-[#e3b341] px-1.5 py-0.5 text-[10px] font-bold tracking-wide text-black">{tag}</span>
        </div>
    }
}

fn request_card(d: D, c: Card) -> impl IntoView {
    view! {
        <div class="gc-card-in flex flex-col gap-2 rounded-2xl border border-[#e3b341]/70 bg-[#16171b] p-3.5">
            {card_head(c.app, "SYSTEM")}
            <div class="text-[15px] font-bold leading-snug">{c.title}</div>
            <div class="text-[12px] text-white/55">{c.sub}</div>
            {move || match d.ask.get() {
                Ask::Waiting => view! {
                    <div class="mt-1 grid grid-cols-2 gap-2">
                        <div class=PHONE_SEC>{c.no}</div>
                        <div node_ref=d.p_yes class=move || if d.tapped.get() == c.yes { PHONE_PRI_DOWN } else { PHONE_PRI }>{c.yes}</div>
                    </div>
                }
                .into_any(),
                Ask::Pressing => view! {
                    <div class="mt-1 flex items-center gap-2 rounded-xl bg-white/[.06] px-3 py-2.5 text-[12.5px] text-white/75">
                        <span class="h-3.5 w-3.5 shrink-0 animate-spin rounded-full border-2 border-white/20 border-t-white"></span>
                        {c.doing}
                    </div>
                }
                .into_any(),
                Ask::Done => view! {
                    <div class="mt-1 rounded-xl bg-[#10331f] px-3 py-2.5 text-[12.5px] font-semibold text-[#4ade80]">"✓ Done on your Mac"</div>
                }
                .into_any(),
            }}
        </div>
    }
}

fn cards_view(d: D) -> impl IntoView {
    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-2.5">
            {move || d.card.get().map(|c| request_card(d, c))}
            <Show when=move || d.clear.get()>
                <div class="gc-card-in flex flex-col items-center gap-1 rounded-2xl bg-[#16171b] p-4 text-center">
                    <span class="text-[28px] leading-none text-[#4ade80]">"✓"</span>
                    <span class="text-[15px] font-bold">"4 handled · 0 waiting"</span>
                    <span class="text-[12px] text-white/55">"2 dialogs · 1 toggle · 1 added app"</span>
                </div>
            </Show>
            <Show when=move || !d.log.with(Vec::is_empty)>
                <div class="px-1 pt-1 text-[10.5px] font-semibold uppercase tracking-wider text-white/35">"Handled"</div>
                <div class="flex flex-col gap-1.5">
                    {move || d.log.get().into_iter().map(|(what, how)| view! {
                        <div class="gc-card-in flex items-center gap-2 rounded-xl bg-white/[.05] px-3 py-2 text-[12px]">
                            <span class="text-[#4ade80]">"✓"</span>
                            <span class="min-w-0 flex-1 truncate">{what}</span>
                            <span class="shrink-0 text-white/45">{how}</span>
                        </div>
                    }).collect_view()}
                </div>
            </Show>
            <div class="mt-auto rounded-2xl bg-[#5b9cf6] py-3 text-center text-[15px] font-bold text-[#0c0d10]">"Hold to talk"</div>
        </div>
    }
}

fn trackpad_view(d: D) -> impl IntoView {
    view! {
        <div class="relative min-h-[110px] flex-1 rounded-2xl border border-dashed border-white/20 bg-white/[.03]">
            <span class="absolute inset-x-0 bottom-2.5 text-center text-[11px] text-white/40">"Trackpad · drag to move, tap to click"</span>
            <div node_ref=d.pad_a class="absolute left-[20%] top-[22%] h-2 w-2"></div>
            <div node_ref=d.pad_b class="absolute left-[72%] top-[54%] h-2 w-2"></div>
        </div>
        <div class="grid grid-cols-2 gap-2">
            <div class="rounded-xl bg-white/10 py-2.5 text-center text-[13px] font-semibold">"Right-click"</div>
            <div
                node_ref=d.p_kb
                class=move || if d.tapped.get() == "Keyboard" {
                    "scale-[.97] rounded-xl bg-white/25 py-2.5 text-center text-[13px] font-semibold transition-transform duration-100"
                } else {
                    "rounded-xl bg-white/10 py-2.5 text-center text-[13px] font-semibold transition-transform duration-100"
                }
            >
                <span class="material-symbols-outlined mr-1 align-[-3px] text-[16px]">"keyboard"</span>
                "Keyboard"
            </div>
        </div>
    }
}

fn key(d: D, c: char) -> impl IntoView {
    view! {
        <div
            node_ref=d.key_ref(c)
            class=move || if d.key.get() == Some(c) {
                "relative grid h-[34px] place-items-center rounded-[5px] bg-[#8e8e93] text-[15px] shadow-[0_1px_0_rgba(0,0,0,.45)]"
            } else {
                "relative grid h-[34px] place-items-center rounded-[5px] bg-[#5c5c61] text-[15px] shadow-[0_1px_0_rgba(0,0,0,.45)]"
            }
        >
            {c.to_string()}
            <Show when=move || d.key.get() == Some(c)>
                <span class="absolute -top-[40px] left-1/2 grid h-[42px] w-[34px] -translate-x-1/2 place-items-center rounded-[7px] bg-[#8e8e93] text-[22px] shadow-lg">
                    {c.to_string()}
                </span>
            </Show>
        </div>
    }
}

fn keys_view(d: D) -> impl IntoView {
    let row = |s: &'static str| s.chars().map(move |c| key(d, c)).collect_view();
    view! {
        <div class="gc-card-in flex flex-col gap-1.5 rounded-2xl border border-[#e3b341]/70 bg-[#16171b] p-3">
            {card_head("System Settings", "PASSWORD")}
            <div class="text-[14px] font-bold">"Typing into the Mac"</div>
            <div class="flex h-8 items-center rounded-lg border border-white/20 px-2.5 text-[14px] tracking-[3px]">
                {move || "•".repeat(d.typed.get())}
                <span class="ml-px h-4 w-px animate-pulse bg-white"></span>
            </div>
            <div class="text-[10.5px] text-white/45">"Phone → board → Mac. Never through the agent."</div>
        </div>
        <div class="-mx-3 -mb-3 mt-auto flex flex-col gap-[9px] bg-[#2b2b2f] px-[3px] pb-5 pt-2.5">
            <div class="grid grid-cols-10 gap-[5px]">{row("qwertyuiop")}</div>
            <div class="grid grid-cols-9 gap-[5px] px-[5%]">{row("asdfghjkl")}</div>
            <div class="grid grid-cols-[1.5fr_repeat(7,1fr)_1.5fr] gap-[5px]">
                <div class="grid h-[34px] place-items-center rounded-[5px] bg-[#3d3d42] text-[14px]">"⇧"</div>
                {row("zxcvbnm")}
                <div class="grid h-[34px] place-items-center rounded-[5px] bg-[#3d3d42] text-[14px]">"⌫"</div>
            </div>
            <div class="grid grid-cols-[1.2fr_3fr_1.6fr] gap-[5px]">
                <div class="grid h-[34px] place-items-center rounded-[5px] bg-[#3d3d42] text-[13px]">"123"</div>
                <div class="grid h-[34px] place-items-center rounded-[5px] bg-[#5c5c61] text-[13px]">"space"</div>
                <div
                    node_ref=d.p_ret
                    class=move || if d.tapped.get() == "return" {
                        "grid h-[34px] place-items-center rounded-[5px] bg-[#0060cc] text-[13px] font-semibold"
                    } else {
                        "grid h-[34px] place-items-center rounded-[5px] bg-[#0a84ff] text-[13px] font-semibold"
                    }
                >
                    "return"
                </div>
            </div>
        </div>
    }
}

fn kind(tag: &'static str, title: &'static str, text: &'static str) -> impl IntoView {
    view! {
        <div class="grid content-start gap-1.5 rounded-2xl border border-gc-border bg-gc-bg p-4 text-[13.5px]">
            <span class="justify-self-start rounded-md border border-gc-border px-2 py-0.5 font-mono text-[10.5px] font-semibold uppercase tracking-wide text-gc-text-muted">{tag}</span>
            <span class="text-[15px] font-bold text-gc-text">{title}</span>
            <span class="text-gc-text-muted">{text}</span>
        </div>
    }
}

/// Allowly: answer your Mac's permission prompts from your phone.
#[component]
pub fn Allowly() -> impl IntoView {
    let stage = NodeRef::<html::Div>::new();
    let phone = NodeRef::<html::Div>::new();
    let mac = NodeRef::<html::Div>::new();

    let tl = Timeline::new();
    let d = D {
        tl,
        me: DemoCursor::new(tl, phone, (0.78, 0.9)),
        board: DemoCursor::new(tl, mac, BOARD_HOME),
        step: RwSignal::new(None),
        caption: RwSignal::new(""),
        app: RwSignal::new("Finder"),
        term: RwSignal::new(("rg invoices ~/Library/Mail", "")),
        dialog: RwSignal::new(None),
        hot: RwSignal::new(""),
        settings: RwSignal::new(None),
        nc_on: RwSignal::new(false),
        term_added: RwSignal::new(false),
        term_on: RwSignal::new(false),
        finder: RwSignal::new(None),
        picked: RwSignal::new(false),
        sheet: RwSignal::new(None),
        dots: RwSignal::new(0),
        toast: RwSignal::new(None),
        mode: RwSignal::new(Mode::Cards),
        card: RwSignal::new(None),
        ask: RwSignal::new(Ask::Waiting),
        tapped: RwSignal::new(""),
        key: RwSignal::new(None),
        typed: RwSignal::new(0),
        drag: RwSignal::new(false),
        trail: RwSignal::new(Vec::new()),
        log: RwSignal::new(Vec::new()),
        clear: RwSignal::new(false),
        mac_size: RwSignal::new((800., 500.)),
        k: RwSignal::new(0.28),
        mac,
        mirror: NodeRef::new(),
        m_yes: NodeRef::new(),
        m_plus: NodeRef::new(),
        m_util: NodeRef::new(),
        m_term_file: NodeRef::new(),
        m_open: NodeRef::new(),
        sw_nc: NodeRef::new(),
        sw_term: NodeRef::new(),
        p_yes: NodeRef::new(),
        pad_a: NodeRef::new(),
        pad_b: NodeRef::new(),
        p_kb: NodeRef::new(),
        p_ret: NodeRef::new(),
        keys: std::array::from_fn(|_| NodeRef::new()),
    };

    // The live view copies the Mac at its real size, scaled down to the phone.
    let measure = move || {
        let Some(m) = mac.get_untracked() else { return };
        let r = m.get_bounding_client_rect();
        if r.width() <= 0. {
            return;
        }
        d.mac_size.set((r.width(), r.height()));
        if let Some(v) = d.mirror.get_untracked() {
            let w = v.get_bounding_client_rect().width();
            if w > 0. {
                d.k.set(w / r.width());
            }
        }
    };
    Effect::new(move |_| {
        let _ = (mac.get(), d.mirror.get());
        request_animation_frame(measure);
    });
    let resize = window_event_listener(ev::resize, move |_| measure());
    on_cleanup(move || resize.remove());

    // The finger's trail on the trackpad.
    let finger = d.me.at();
    Effect::new(move |_| {
        let Some(p) = finger.get() else { return };
        if d.drag.get_untracked() {
            d.trail.update(|v| {
                v.push(p);
                if v.len() > 14 {
                    v.remove(0);
                }
            });
        }
    });

    tl.script(move |s| {
        d.reset();

        // 1. A modal prompt: Automation.
        s.then(move || {
            d.step(0, "The coworker's script drives Finder, and macOS asks. You tap Allow on your phone; the board clicks Allow on the Mac.");
            d.app.set("Terminal");
            d.term.set(("osascript tidy-downloads.scpt", "waiting for Automation…"));
        });
        modal(s, d, Dialog::Automation, AUTOMATION, ("Terminal can control Finder", "Allowed from your phone. Clicked by the board."), ("Terminal → Finder", "Allowed"));

        // 2. A modal prompt: Gatekeeper.
        s.then(move || {
            d.step(1, "The coworker installed an app. Gatekeeper asks on first launch. Open, from your phone.");
            d.app.set("Finder");
            d.term.set(("open ~/Downloads/Figma.app", "waiting for Gatekeeper…"));
        });
        modal(s, d, Dialog::Gatekeeper, GATEKEEPER, ("“Figma” is open", "Opened from your phone. Clicked by the board."), ("Figma", "Opened"));

        // 3. Accessibility: the prompt can only send you to System Settings.
        s.then(move || {
            d.step(2, "This one can't be answered in the dialog. Take control, and the board opens System Settings.");
            d.app.set("NativeChat");
            d.term.set(("nativechat computer-use --start", "waiting for Accessibility…"));
        })
        .wait(450)
        .then(move || d.dialog.set(Some(Dialog::Accessibility)))
        .wait(650)
        .then(move || d.request(ACCESSIBILITY))
        .wait(1000);
        tap(s, d, d.p_yes, ACCESSIBILITY.yes, move || d.ask.set(Ask::Pressing));
        s.wait(150);
        board_click(s, d, d.m_yes, "Open System Settings", move || {
            d.dialog.set(None);
            d.app.set("System Settings");
            d.settings.set(Some(Pane::Accessibility));
        });
        s.wait(500).then(move || {
            d.card.set(None);
            d.mode.set(Mode::Live);
        });

        // 4. The phone becomes a trackpad.
        s.then(move || d.step(3, "macOS only lets a real mouse flip this switch. You see the Mac on your phone and drag on the trackpad; the board moves the Mac's pointer."))
            .wait(900)
            .move_to(d.me, d.pad_a)
            .wait(150)
            .then(move || d.drag.set(true))
            .start_move(d.board, d.sw_nc, 1300)
            .move_in(d.me, d.pad_b, 1300)
            .then(move || d.drag.set(false))
            .wait(350)
            .press(d.me)
            .wait(110)
            .press(d.board)
            .wait(260)
            .then(move || {
                d.trail.set(Vec::new());
                d.sheet.set(Some("Accessibility"));
            })
            .wait(800);

        // 5. The phone becomes a keyboard for the Mac password.
        s.then(move || d.step(4, "macOS wants your password. Tap Keyboard: each key goes phone → board, which types it as a real USB keyboard. It never passes through the agent."));
        tap(s, d, d.p_kb, "Keyboard", move || d.mode.set(Mode::Keys));
        s.wait(450);
        type_password(s, d, 280);
        s.wait(250);
        tap(s, d, d.p_ret, "return", move || d.hot.set("Modify Settings"));
        s.wait(200)
            .then(move || {
                d.hot.set("");
                d.sheet.set(None);
                d.dots.set(0);
                d.typed.set(0);
                d.nc_on.set(true);
            })
            .wait(700)
            .then(move || {
                d.toast.set(Some(("Accessibility on for NativeChat", "Switched by the board. Password typed from your phone.")));
                d.mode.set(Mode::Cards);
                d.log.update(|v| v.push(("NativeChat", "Accessibility on")));
            })
            .wait(1600)
            .then(move || d.toast.set(None));

        // 6. Full Disk Access: the app isn't listed, so the board adds it.
        s.then(move || {
            d.step(5, "Terminal isn't in the list yet, so there's no dialog to answer. The board clicks +, picks Terminal, opens it, then turns it on.");
            d.settings.set(Some(Pane::FullDisk));
            d.term.set(("rg invoices ~/Library/Mail", "waiting for Full Disk Access…"));
        })
        .wait(500)
        .then(move || d.request(FULL_DISK))
        .wait(1000);
        tap(s, d, d.p_yes, FULL_DISK.yes, move || d.ask.set(Ask::Pressing));
        board_click(s, d, d.m_plus, "+", move || d.finder.set(Some(Folder::Applications)));
        s.wait(350);
        board_click(s, d, d.m_util, "Utilities", move || d.finder.set(Some(Folder::Utilities)));
        s.wait(250);
        board_click(s, d, d.m_term_file, "Terminal", move || d.picked.set(true));
        board_click(s, d, d.m_open, "Open", move || {
            d.finder.set(None);
            d.picked.set(false);
            d.term_added.set(true);
        });
        s.wait(350);
        board_click(s, d, d.sw_term, "", move || {
            d.sheet.set(Some("Full Disk Access"));
            d.mode.set(Mode::Keys);
        });
        s.wait(400);
        type_password(s, d, 170);
        tap(s, d, d.p_ret, "return", move || d.hot.set("Modify Settings"));
        s.wait(200)
            .then(move || {
                d.hot.set("");
                d.sheet.set(None);
                d.dots.set(0);
                d.typed.set(0);
                d.term_on.set(true);
            })
            .wait(600)
            .then(move || {
                d.toast.set(Some(("Full Disk Access on for Terminal", "Your coworker keeps searching Mail.")));
                d.term.set(("rg invoices ~/Library/Mail", "142 matches in 38 messages"));
                d.mode.set(Mode::Cards);
                d.ask.set(Ask::Done);
            })
            .wait(900)
            .then(move || {
                d.card.set(None);
                d.log.update(|v| v.push(("Terminal", "Full Disk Access on")));
            })
            .wait(700);

        // 7. All clear.
        s.then(move || {
            d.step(6, "Four requests, handled from your pocket: two dialogs, one switch, one app added.");
            d.settings.set(None);
            d.app.set("Terminal");
            d.clear.set(true);
        })
        .move_home(d.me)
        .move_home(d.board)
        .wait(4200)
        .then(move || {
            if in_view(stage) {
                tl.play();
            }
        });
    });

    autoplay_when_visible(tl, stage);

    view! {
        <section id="allowly" class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card/40 overflow-hidden">
            <div class="max-w-[1240px] mx-auto px-6 md:px-12">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <div class="text-[11px] font-bold tracking-wide uppercase text-gc-text-faint mb-3">"Allowly · Mac"</div>
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "You tap Allow. It actually presses Allow."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[56ch]">
                        "A tool for working with agents. Your coworker hits a macOS permission prompt while you're out, and it lands on your phone. Tap the answer: a USB board on your Mac clicks it like a real mouse, and types like a real keyboard."
                    </p>
                </div>

                <DemoBar steps=STEPS current=d.step timeline=tl caption=d.caption />

                <div node_ref=stage class="grid select-none grid-cols-1 items-center gap-8 lg:grid-cols-[minmax(0,1fr)_270px] lg:gap-14">
                    // Mac
                    <div class="min-w-0">
                        <div class="rounded-[18px] bg-[#1c1c1f] p-2 shadow-2xl ring-1 ring-black/40 sm:p-[10px]">
                            <div node_ref=mac class="relative aspect-[5/4] overflow-hidden rounded-[8px] sm:aspect-[16/10]" style=format!("{WALL}{MAC_FONT}")>
                                {mac_screen(d, true)}
                            </div>
                        </div>
                        <div class="-ml-[2%] h-3 w-[104%] rounded-b-[14px] bg-gradient-to-b from-[#3a3a3e] to-[#1c1c1f]"></div>
                        <div class="mx-auto h-1 w-24 rounded-b-md bg-[#2a2a2e]"></div>
                    </div>

                    // iPhone
                    <div class="mx-auto w-[270px] shrink-0 rounded-[46px] bg-[#1c1c1f] p-[10px] shadow-2xl ring-1 ring-black/40">
                        <div node_ref=phone class="relative flex h-[580px] flex-col overflow-hidden rounded-[37px] bg-[#0c0d10] text-white" style=MAC_FONT>
                            <div class="relative flex h-11 shrink-0 items-center justify-between px-6 pt-1 text-[12.5px] font-semibold">
                                <span>"9:41"</span>
                                <span class="absolute left-1/2 top-2.5 h-[26px] w-[84px] -translate-x-1/2 rounded-full bg-black"></span>
                                <span class="flex items-center gap-1">
                                    <span class="material-symbols-outlined text-[14px]">"signal_cellular_alt"</span>
                                    <span class="material-symbols-outlined text-[14px]">"wifi"</span>
                                    <span class="material-symbols-outlined rotate-90 text-[15px]">"battery_full"</span>
                                </span>
                            </div>
                            <div class="flex shrink-0 items-center justify-between border-b border-white/10 px-4 pb-2.5 pt-1">
                                <span class="text-[18px] font-bold">"Allowly"</span>
                                <span class="inline-flex items-center gap-1.5 rounded-full bg-[#10331f] px-2.5 py-1 text-[11px] font-semibold text-[#4ade80]">
                                    <span class=move || if d.mode.get() == Mode::Cards { "h-1.5 w-1.5 rounded-full bg-[#4ade80]" } else { "h-1.5 w-1.5 animate-pulse rounded-full bg-[#4ade80]" }></span>
                                    {move || if d.mode.get() == Mode::Cards { "Connected" } else { "Live screen" }}
                                </span>
                            </div>
                            <div class="flex min-h-0 flex-1 flex-col gap-2.5 p-3">
                                <Show when=move || d.mode.get() != Mode::Cards>
                                    <div node_ref=d.mirror class="relative aspect-[5/4] w-full shrink-0 overflow-hidden rounded-[10px] ring-1 ring-white/15 sm:aspect-[16/10]">
                                        <div
                                            class="pointer-events-none absolute left-0 top-0 origin-top-left overflow-hidden"
                                            style=move || {
                                                let (w, h) = d.mac_size.get();
                                                format!("width:{w:.1}px;height:{h:.1}px;transform:scale({:.4});{WALL}{MAC_FONT}", d.k.get())
                                            }
                                        >
                                            {mac_screen(d, false)}
                                        </div>
                                    </div>
                                </Show>
                                {move || match d.mode.get() {
                                    Mode::Cards => cards_view(d).into_any(),
                                    Mode::Live => trackpad_view(d).into_any(),
                                    Mode::Keys => keys_view(d).into_any(),
                                }}
                            </div>

                            // The finger on the trackpad, and where it has been.
                            {move || {
                                let trail = d.trail.get();
                                let n = trail.len().max(1) as f64;
                                trail.into_iter().enumerate().map(|(i, (x, y))| view! {
                                    <span
                                        class="pointer-events-none absolute left-0 top-0 z-20 h-2 w-2 rounded-full bg-[#0a84ff]"
                                        style=format!("transform:translate({:.1}px,{:.1}px);opacity:{:.2}", x - 4., y - 4., 0.08 + 0.5 * (i as f64 + 1.) / n)
                                    ></span>
                                }).collect_view()
                            }}
                            <Show when=move || d.drag.get()>
                                <span
                                    class="pointer-events-none absolute left-0 top-0 z-20 h-7 w-7 rounded-full bg-[#0a84ff]/45 ring-4 ring-[#0a84ff]/20"
                                    style=move || {
                                        let (x, y) = finger.get().unwrap_or((0., 0.));
                                        format!("transform:translate({:.1}px,{:.1}px)", x - 14., y - 14.)
                                    }
                                ></span>
                            </Show>
                            <DemoPointer cursor=d.me />
                        </div>
                    </div>
                </div>

                <div class="mt-10 grid grid-cols-1 gap-3 md:grid-cols-3">
                    {kind("Modal prompt", "macOS asks in a dialog", "Automation, Files & Folders, Camera, a newly downloaded app. Tap the answer on the phone; the board clicks it.")}
                    {kind("Manual toggle", "Only System Settings can grant it", "Accessibility, Screen Recording. Your phone becomes a trackpad to click the switch, then a keyboard for the Mac password.")}
                    {kind("Manual add", "The app isn't in the list yet", "Full Disk Access. The board clicks +, picks the app in the Finder sheet, then turns it on.")}
                </div>
            </div>
        </section>
    }
}
