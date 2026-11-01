use leptos::{html, prelude::*};

use crate::components::agent_window::{
    AgentScreen, AgentWindow, Coworker, GENIE, GENIE_RAIL, PaneIcon, PaneIcons, box_desktop, dock_apps,
};
use crate::components::demo_player::{Aim, DemoBar, DemoCursor, DemoPointer, Script, Timeline, autoplay_when_visible};
use crate::components::tool_icons::{IconSize, Tool, tool_icon};

/// Share your agents as templates: a NativeChat window that plays itself. The owner shares
/// Buwiz Clerk, reviews the card and what it carries, publishes and copies the link. Then a
/// teammate opens the link, makes their own copy, signs in to their own accounts, and files a form.
///
/// Everything clickable is a drawn box, never a real control.
const STEPS: &[&str] = &["Share", "Review", "What it carries", "Publish", "Open link", "Their copy", "Ready"];

// Public claims. None of these may change without sign-off.
const TEMPLATE_URL: &str = "buwiz.com/t/buwiz-clerk";
const LAST_UPDATED: &str = "Oct 8";
/// The computer and its software in templates aren't shipped yet.
const COMPUTER_TAG: &str = "Coming soon";
const CLAIM_LOGINS: &str = "Your logins never travel.";
/// Illustrative 2550Q lines. 12% of sales is output VAT; payable is output less input.
const FORM_ROWS: [(&str, &str); 4] = [
    ("Vatable sales", "1,284,500.00"),
    ("Output VAT", "154,140.00"),
    ("Input VAT", "92,480.00"),
    ("VAT payable", "61,660.00"),
];

const C_SHARE: &str = "Share Buwiz Clerk as a template. Create a copy makes the link.";
const C_REVIEW: &str = "Nothing is public yet. Review the card before you publish.";
const C_CARRIES: &str = "Context and integrations carry over. Computer and software: coming soon.";
const C_PUBLISH: &str = "Publish, then copy the link.";
const C_OPEN: &str = "Anyone with the link sees what's inside. Use this coworker makes their copy.";
const C_COPY: &str = "Coming soon: their copy boots its own computer and installs the software.";
const C_SIGNIN: &str = "They sign in to their own accounts. Your logins never travel.";
const C_READY: &str = "Same coworker, their logins. It files the form the way yours did.";

const DESCRIPTION: &str = "Tax clerk: files BIR forms in Buwiz Forms, books entries in Buwiz Books.";
const INSTRUCTIONS: &str =
    "Tax clerk. Files BIR forms in Buwiz Forms and books entries in Buwiz Books. Confirms totals before filing.";
const MEMORIES: [&str; 4] = [
    "The owner wants receipts booked the same day.",
    "2550Q totals come from Buwiz Books, not from memory.",
    "Asks before filing anything that is not a draft.",
    "Sends a summary after every filing.",
];
const ASK: &str = "File our Q3 2550Q";

const BUWIZ_CLERK: Coworker = Coworker { initials: "BC", name: "Buwiz Clerk", color: "bg-[#16a34a]" };
const OC: Coworker = Coworker { initials: "OC", name: "OC", color: "bg-[#7dcea0]" };
const CO: Coworker = Coworker { initials: "Co", name: "Co", color: "bg-[#7c6cf0]" };
const VI: Coworker = Coworker { initials: "Vi", name: "Vi", color: "bg-[#8b5cf6]" };
const TR: Coworker = Coworker { initials: "Tr", name: "Tr", color: "bg-[#3b82f6]" };
const MA: Coworker = Coworker { initials: "Ma", name: "Ma", color: "bg-[#f472b6]" };

/// The owner's rail, with Buwiz Clerk on it.
const OWNER_RAIL: &[Coworker] = &[OC, BUWIZ_CLERK, CO, VI, TR];
/// The teammate's rail once their copy of Buwiz Clerk joins it.
const TEAM_RAIL: &[Coworker] = &[BUWIZ_CLERK, OC, GENIE, MA, CO, TR];

const OWNER_ROUTINE: (&str, &str) = ("Quarter-end filing", "Quarterly");

/// Buwiz Clerk's dock, in install order.
const DOCK: [Tool; 5] = [Tool::Chromium, Tool::BuwizBooks, Tool::BuwizForms, Tool::Office, Tool::Terminal];
/// The Computer & software page: four tools, then the recipes (None).
const CARRIED: [Option<Tool>; 5] =
    [Some(Tool::BuwizForms), Some(Tool::BuwizBooks), Some(Tool::Chromium), Some(Tool::Office), None];

const ICO: &str = "grid h-7 w-7 shrink-0 place-items-center rounded-lg text-gc-text-muted";
const SEC: &str = "text-[11.5px] font-semibold text-gc-text";
const BOX: &str = "rounded-xl border border-gc-border px-3";
const ROW: &str =
    "flex min-w-0 items-center gap-2.5 border-b border-gc-border py-2 text-[13px] text-gc-text last:border-b-0";
const TILE: &str = "grid shrink-0 place-items-center rounded-lg border border-gc-border bg-gc-bg-muted text-gc-text";
const PUBLISH: &str = "shrink-0 rounded-full bg-gc-text px-3.5 py-1.5 text-[12.5px] font-semibold text-gc-bg";
const OUTLINE: &str =
    "shrink-0 rounded-full border border-gc-border px-3 py-1.5 text-[12.5px] font-semibold text-gc-text";
const TAG: &str = "shrink-0 rounded-full border border-gc-accent/50 px-2 py-0.5 text-[11px] font-semibold text-gc-text";
const CHIP_OFF: &str = "inline-flex items-center rounded-lg border border-dashed border-gc-border-strong bg-transparent px-2.5 py-1 font-mono text-[12px] text-gc-text-muted line-through";

#[derive(Clone, Copy, PartialEq)]
enum Page {
    List,
    Context,
    Integrations,
    Computer,
}

/// Whose desktop the window shows.
#[derive(Clone, Copy, PartialEq)]
enum Desk {
    Owner,
    Genie,
    Copy,
}

#[derive(Clone, Copy, PartialEq)]
enum Boot {
    Starting,
    Installing,
    Ready,
}

#[derive(Clone, Copy, PartialEq)]
enum Flip {
    None,
    Out,
    In,
}

/// The menu item or list row the cursor is resting on.
#[derive(Clone, Copy, PartialEq)]
enum Lit {
    None,
    Menu,
    Row(usize),
}

#[derive(Clone, Copy)]
enum Body {
    Me(&'static str),
    Bot(&'static str),
    /// The template card: Unpublished, then Published with its link.
    Review,
    /// Sign in to Gmail and Buwiz Books.
    Action,
}

#[derive(Clone)]
struct Msg {
    id: u32,
    body: Body,
}

#[derive(Clone, Copy)]
struct T {
    tl: Timeline,
    cur: DemoCursor,
    step: RwSignal<Option<usize>>,
    caption: RwSignal<&'static str>,
    msgs: RwSignal<Vec<Msg>>,
    next: StoredValue<u32>,
    draft: RwSignal<&'static str>,
    who: RwSignal<Coworker>,
    rail: RwSignal<&'static [Coworker]>,
    routine: RwSignal<(&'static str, &'static str)>,
    menu: RwSignal<bool>,
    share_lit: RwSignal<bool>,
    lit: RwSignal<Lit>,
    open: RwSignal<bool>,
    page: RwSignal<Page>,
    back_dir: RwSignal<bool>,
    ticks: RwSignal<usize>,
    published: RwSignal<bool>,
    flip: RwSignal<Flip>,
    copied: RwSignal<bool>,
    toast: RwSignal<bool>,
    cut: RwSignal<bool>,
    web: RwSignal<bool>,
    signed: RwSignal<[bool; 2]>,
    desk: RwSignal<Desk>,
    boot: RwSignal<Boot>,
    dock_n: RwSignal<usize>,
    forms: RwSignal<bool>,
    screen: AgentScreen,
    share: PaneIcon,
    icons: PaneIcons,
    // Where the cursor goes. Every target is a <div>.
    win: NodeRef<html::Div>,
    scroller: NodeRef<html::Div>,
    composer: NodeRef<html::Div>,
    menu_copy: NodeRef<html::Div>,
    details: NodeRef<html::Div>,
    rows: [NodeRef<html::Div>; 3],
    back: NodeRef<html::Div>,
    publish: NodeRef<html::Div>,
    copy: NodeRef<html::Div>,
    use_btn: NodeRef<html::Div>,
    sign: [NodeRef<html::Div>; 2],
}

impl T {
    /// The owner's window as it is before the run. Replay drops queued steps, so every flag a
    /// step might have left on is set back here.
    fn reset(self) {
        self.who.set(BUWIZ_CLERK);
        self.rail.set(OWNER_RAIL);
        self.routine.set(OWNER_ROUTINE);
        self.desk.set(Desk::Owner);
        self.boot.set(Boot::Ready);
        self.dock_n.set(5);
        self.forms.set(false);
        let seed = [
            Body::Me("File the Q3 2550Q and book this month's receipts."),
            Body::Bot("2550Q drafted from your Buwiz Books totals. 14 receipts booked."),
        ];
        self.msgs.set(seed.into_iter().map(|body| Msg { id: self.id(), body }).collect());
        self.draft.set("");
        self.menu.set(false);
        self.share_lit.set(false);
        self.lit.set(Lit::None);
        self.open.set(false);
        self.page.set(Page::List);
        self.back_dir.set(false);
        self.ticks.set(0);
        self.published.set(false);
        self.flip.set(Flip::None);
        self.copied.set(false);
        self.toast.set(false);
        self.cut.set(false);
        self.web.set(false);
        self.signed.set([false, false]);
        self.screen.reset();
        self.cur.reset();
    }

    fn step(self, n: usize, caption: &'static str) {
        self.step.set(Some(n));
        self.caption.set(caption);
    }

    fn id(self) -> u32 {
        self.next
            .try_update_value(|n| {
                *n += 1;
                *n
            })
            .unwrap_or_default()
    }

    fn push(self, body: Body) {
        let id = self.id();
        self.msgs.update(|v| v.push(Msg { id, body }));
        let scroller = self.scroller;
        // Twice: cards grow a moment after they render.
        let down = move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        };
        request_animation_frame(down);
        gloo_timers::callback::Timeout::new(320, down).forget();
    }
}

/// Glide to `aim`, light it, press, then `f`. No settle afterwards.
fn press_on(
    s: &mut Script,
    cur: DemoCursor,
    aim: impl Into<Aim>,
    on: impl FnOnce() + 'static,
    f: impl FnOnce() + 'static,
) {
    s.move_to(cur, aim).then(on).wait(250).press(cur).wait(170).then(f);
}

/// The cursor rests on a toolbar icon, which lights up, then clicks it.
fn tap(s: &mut Script, cur: DemoCursor, aim: impl Into<Aim>, lit: RwSignal<bool>, f: impl FnOnce() + 'static) {
    press_on(
        s,
        cur,
        aim,
        move || lit.set(true),
        move || {
            lit.set(false);
            f();
        },
    );
    s.wait(380);
}

/// The same for a menu item or a list row.
fn tap_lit(s: &mut Script, t: T, aim: impl Into<Aim>, which: Lit, f: impl FnOnce() + 'static) {
    press_on(
        s,
        t.cur,
        aim,
        move || t.lit.set(which),
        move || {
            t.lit.set(Lit::None);
            f();
        },
    );
    s.wait(380);
}

/// Opens a page of the details modal, sliding forward or back.
fn go_page(t: T, page: Page, back: bool) {
    t.back_dir.set(back);
    t.page.set(page);
}

fn script(s: &mut Script, t: T) {
    // Timing assumes the fixed 2x speed: script waits run on the demo clock, CSS transitions in
    // real time. Demo time can lag real time but never lead it (frames are clamped), so a CSS
    // transition has always finished before the step that follows it. Nothing aims at an element
    // until 400 demo ms after it mounts.
    t.reset();
    t.step(0, C_SHARE);

    // Act 1: the owner shares Buwiz Clerk.
    s.wait(300);
    tap(s, t.cur, t.share.aim(), t.share_lit, move || t.menu.set(true));
    s.wait(100);
    tap_lit(s, t, t.menu_copy, Lit::Menu, move || {
        t.menu.set(false);
        t.step(1, C_REVIEW);
    });
    s.then(move || t.push(Body::Bot("Staging Buwiz Clerk as a template. Review it before it goes public.")))
        .wait(600)
        .then(move || t.push(Body::Review))
        .wait(400)
        .click(t.cur, t.details, move || {
            go_page(t, Page::List, false);
            t.open.set(true);
            t.step(2, C_CARRIES);
        })
        .wait(600);

    tap_lit(s, t, t.rows[0], Lit::Row(0), move || go_page(t, Page::Context, false));
    s.wait(600).click(t.cur, t.back, move || go_page(t, Page::List, true)).wait(100);
    tap_lit(s, t, t.rows[1], Lit::Row(1), move || go_page(t, Page::Integrations, false));
    s.wait(500).click(t.cur, t.back, move || go_page(t, Page::List, true)).wait(100);
    press_on(
        s,
        t.cur,
        t.rows[2],
        move || t.lit.set(Lit::Row(2)),
        move || {
            t.lit.set(Lit::None);
            t.ticks.set(0);
            go_page(t, Page::Computer, false);
        },
    );
    for i in 1..=5usize {
        s.wait(180).then(move || t.ticks.set(i));
    }
    s.wait(350).click(t.cur, t.back, move || go_page(t, Page::List, true)).wait(100);

    s.then(move || t.step(3, C_PUBLISH))
        .click(t.cur, t.publish, move || t.open.set(false))
        // The scrim is gone (300 real ms) before the card flips.
        .wait(240)
        .then(move || t.flip.set(Flip::Out))
        .wait(360)
        .then(move || {
            t.published.set(true);
            t.flip.set(Flip::In);
        })
        .wait(360)
        .then(move || t.push(Body::Bot("Published. Anyone with the link can use Buwiz Clerk.")))
        .wait(40)
        .click(t.cur, t.copy, move || {
            t.copied.set(true);
            t.toast.set(true);
        })
        .wait(1620)
        .then(move || {
            t.toast.set(false);
            t.copied.set(false);
        });

    // Cut to a teammate's app. Everything changes under the opaque cut.
    s.then(move || {
        t.step(4, C_OPEN);
        t.cut.set(true);
    })
    .wait(600)
    .then(move || {
        t.msgs.set(Vec::new());
        t.who.set(GENIE);
        t.rail.set(GENIE_RAIL);
        t.routine.set(("Weekly close", "Sundays"));
        t.desk.set(Desk::Genie);
        t.published.set(false);
        t.flip.set(Flip::None);
        t.cur.reset();
    })
    .wait(100)
    .then(move || t.cut.set(false))
    .wait(600)
    .then(move || t.web.set(true))
    .wait(400)
    .click(t.cur, t.use_btn, move || {
        t.web.set(false);
        t.who.set(BUWIZ_CLERK);
        t.rail.set(TEAM_RAIL);
        t.routine.set(OWNER_ROUTINE);
        t.desk.set(Desk::Copy);
        t.boot.set(Boot::Starting);
        t.dock_n.set(0);
        t.step(5, C_COPY);
    })
    // Buwiz Clerk pops into their rail before the screen grows over it.
    .wait(600)
    .then(move || t.screen.expand())
    .wait(300)
    .then(move || {
        t.push(Body::Bot("Setting up Buwiz Clerk: starting its computer, installing Buwiz Forms and Buwiz Books."))
    })
    .wait(460)
    .then(move || t.boot.set(Boot::Installing));
    for i in 1..=5usize {
        s.wait(380).then(move || t.dock_n.set(i));
    }
    s.wait(200)
        .then(move || t.boot.set(Boot::Ready))
        .wait(200)
        .then(move || {
            t.caption.set(C_SIGNIN);
            t.screen.shrink();
        })
        .wait(680)
        .then(move || t.push(Body::Action))
        .wait(400)
        .click(t.cur, t.sign[0], move || t.signed.update(|s| s[0] = true))
        .click(t.cur, t.sign[1], move || t.signed.update(|s| s[1] = true))
        .then(move || t.step(6, C_READY))
        .wait(160)
        .typing(ASK, 60, move |d| t.draft.set(d))
        .wait(200)
        .then(move || {
            t.draft.set("");
            t.push(Body::Me(ASK));
        })
        .wait(500)
        .then(move || {
            t.push(Body::Bot(
                "Opened Buwiz Forms and filled the 2550Q from the Buwiz Books totals. Ready for your review.",
            ));
            t.forms.set(true);
        })
        .wait(100)
        .then(move || t.screen.expand())
        .wait(1360)
        .then(move || t.screen.shrink())
        .wait(720)
        .then(|| ());
}

fn glyph(name: &'static str, size: &'static str) -> impl IntoView {
    view! { <span class=format!("material-symbols-outlined leading-none {size}") aria-hidden="true">{name}</span> }
}

fn check() -> impl IntoView {
    view! { <span class="mac-pop grid shrink-0 place-items-center text-gc-ok-text">{glyph("check", "text-[18px]")}</span> }
}

fn chevron() -> impl IntoView {
    view! { <span class="grid shrink-0 place-items-center text-gc-text-muted">{glyph("chevron_right", "text-[16px]")}</span> }
}

fn avatar(size: &'static str) -> impl IntoView {
    view! { <span class=format!("grid shrink-0 place-items-center rounded-full bg-[#16a34a] font-bold text-[#1a1a1a] {size}")>"BC"</span> }
}

fn status_pill(published: bool) -> impl IntoView {
    let (border, dot, label) = if published {
        ("border-gc-accent/50", "bg-gc-accent", "Published")
    } else {
        ("border-gc-border", "bg-gc-text-faint", "Unpublished")
    };
    view! {
        <span class=format!("inline-flex shrink-0 items-center gap-1.5 rounded-full border {border} px-2 py-0.5 text-[11.5px] font-medium text-gc-text")>
            <span class=format!("h-1.5 w-1.5 rounded-full {dot}")></span>
            {label}
        </span>
    }
}

/// The template card in the owner's chat. The outer box slides in once; the inner one flips.
fn review_card(t: T) -> impl IntoView {
    let flip = move || match t.flip.get() {
        Flip::None => "",
        Flip::Out => " gc-flip-out",
        Flip::In => " gc-flip-in",
    };
    view! {
        <div class="gc-card-in w-full max-w-[min(100%,360px)] self-start">
            <div class=move || format!("grid gap-2.5 rounded-2xl border border-gc-border bg-gc-bg-elevated p-3.5{}", flip())>
                <div class="flex min-w-0 items-center justify-between gap-2">
                    <span class="min-w-0 truncate text-[14px] font-semibold text-gc-text">"Buwiz Clerk"</span>
                    {move || status_pill(t.published.get())}
                </div>
                <div class="lx-desk relative aspect-[16/10] w-full overflow-hidden rounded-lg border border-gc-border">
                    <div class="absolute inset-x-0 top-0 h-3 bg-black/35"></div>
                    <div class="absolute bottom-2 left-1/2 flex -translate-x-1/2 gap-1.5 rounded-lg border border-white/15 bg-black/40 p-1">
                        {[Tool::BuwizForms, Tool::BuwizBooks, Tool::Chromium].into_iter().map(|k| tool_icon(k, IconSize::Mini)).collect_view()}
                    </div>
                </div>
                <p class="line-clamp-2 text-[12.5px] text-gc-text-muted">{DESCRIPTION}</p>
                <Show when=move || t.published.get()>
                    <div class="flex min-w-0 items-center gap-2">
                        <span class="min-w-0 flex-1 truncate rounded-lg border border-gc-border px-2.5 py-1.5 font-mono text-[12px] text-gc-text">{TEMPLATE_URL}</span>
                        <div node_ref=t.copy class=format!("{OUTLINE} min-w-[64px] text-center")>
                            {move || if t.copied.get() { "Copied" } else { "Copy" }}
                        </div>
                    </div>
                </Show>
                <div class="flex items-center gap-2">
                    <Show when=move || !t.published.get()>
                        <span class=PUBLISH>"Publish"</span>
                    </Show>
                    <div node_ref=t.details class=OUTLINE>"View Details"</div>
                </div>
            </div>
        </div>
    }
}

/// Their copy asks them to sign in to their own accounts.
fn action_card(t: T) -> impl IntoView {
    let rows = [(0usize, "Gmail", "Sign in with your account"), (1, "Buwiz Books", "Your Buwiz Books company file")];
    view! {
        <div class="gc-card-in grid w-full max-w-[min(100%,360px)] gap-2 self-start rounded-2xl border border-gc-border bg-gc-bg-elevated p-3.5">
            <div class="flex items-center gap-2 text-[13px] font-semibold text-gc-text">
                <span class="h-1.5 w-1.5 rounded-full bg-gc-gold"></span>
                "Action needed"
            </div>
            {rows.into_iter().map(|(i, name, sub)| view! {
                <div class="flex min-w-0 items-center gap-2.5 rounded-xl border border-gc-border p-2">
                    {if i == 0 {
                        view! { <span class=format!("{TILE} h-[34px] w-[34px] text-[13px] font-semibold")>"G"</span> }.into_any()
                    } else {
                        tool_icon(Tool::BuwizBooks, IconSize::Tile).into_any()
                    }}
                    <div class="min-w-0 flex-1">
                        <div class="text-[13px] font-medium text-gc-text">{name}</div>
                        <div class="truncate text-[11.5px] text-gc-text-muted">{sub}</div>
                    </div>
                    {move || if t.signed.get()[i] {
                        view! {
                            <span class="mac-pop inline-flex shrink-0 items-center gap-1 text-[12px] font-semibold text-gc-text">
                                <span class="grid place-items-center text-gc-ok-text">{glyph("check", "text-[16px]")}</span>
                                "Signed in"
                            </span>
                        }.into_any()
                    } else {
                        view! { <div node_ref=t.sign[i] class=OUTLINE>"Sign in"</div> }.into_any()
                    }}
                </div>
            }).collect_view()}
        </div>
    }
}

fn message(t: T, m: Msg) -> impl IntoView {
    match m.body {
        Body::Me(text) => view! {
            <div class="gc-card-in max-w-[88%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-[14px] text-gc-bg">{text}</div>
        }
        .into_any(),
        Body::Bot(text) => view! {
            <div class="gc-card-in max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-[14px] text-gc-text">{text}</div>
        }
        .into_any(),
        Body::Review => review_card(t).into_any(),
        Body::Action => action_card(t).into_any(),
    }
}

fn dock(t: T) -> impl IntoView {
    (0..5usize)
        .map(|i| {
            view! {
                <Show when=move || { t.dock_n.get() > i }>
                    <span class="lx-dock-item mac-pop">{tool_icon(DOCK[i], IconSize::Dock)}</span>
                </Show>
            }
        })
        .collect_view()
}

/// Their copy's desktop: a boot splash, then Buwiz Forms with the 2550Q once it's asked.
fn copy_body(t: T) -> impl IntoView {
    view! {
        <div class=move || if t.boot.get() == Boot::Starting {
            "absolute inset-x-0 bottom-0 top-8 z-[5] grid place-items-center bg-[#0b1020] opacity-100 transition-opacity duration-300"
        } else {
            "absolute inset-x-0 bottom-0 top-8 z-[5] grid place-items-center bg-[#0b1020] opacity-0 transition-opacity duration-300"
        }>
            <div class="grid justify-items-center gap-3">
                <span class="h-6 w-6 animate-spin rounded-full border-2 border-white/25 border-t-white motion-reduce:animate-none"></span>
                <span class="text-[12px] font-medium text-white/80">"Starting"</span>
            </div>
        </div>
        <Show when=move || t.forms.get()>
            <div class="absolute left-1/2 top-12 w-[min(78%,520px)] -translate-x-1/2">
                <div class="mac-pop overflow-hidden rounded-lg bg-white text-[#1f2328] shadow-xl">
                    <div class="flex items-center gap-2 bg-[#f1f3f4] px-3 py-1.5 text-[12px] font-medium">
                        {tool_icon(Tool::BuwizForms, IconSize::Mini)}
                        "2550Q · Buwiz Forms"
                    </div>
                    <div class="px-3 py-1">
                        {FORM_ROWS.into_iter().map(|(label, value)| view! {
                            <div class="flex items-center justify-between gap-3 border-b border-[#e5e7eb] py-1.5 font-mono text-[12px] tabular-nums last:border-b-0">
                                <span class="min-w-0 truncate">{label}</span>
                                <span class="shrink-0">{value}</span>
                            </div>
                        }).collect_view()}
                    </div>
                </div>
            </div>
        </Show>
    }
}

fn desktop(t: T) -> impl IntoView {
    move || match t.desk.get() {
        Desk::Genie => box_desktop("Buwiz Bot Box · Genie", (), (), dock_apps()).into_any(),
        Desk::Owner => box_desktop("Buwiz Bot Box · Buwiz Clerk", (), (), dock(t)).into_any(),
        Desk::Copy => box_desktop(
            "Buwiz Bot Box · Buwiz Clerk",
            view! {
                <span class="font-medium">
                    {move || match t.boot.get() {
                        Boot::Starting => "Starting",
                        Boot::Installing => "Installing",
                        Boot::Ready => "Ready",
                    }}
                </span>
            },
            copy_body(t),
            dock(t),
        )
        .into_any(),
    }
}

/// A row in the details list: icon tile, name over a one-line description, and what sits right.
fn entry(
    aim: NodeRef<html::Div>,
    icon: &'static str,
    name: impl IntoView + 'static,
    desc: String,
    lit: Signal<bool>,
    right: impl IntoView + 'static,
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
            <span class=format!("{TILE} h-[30px] w-[30px] text-gc-text-muted")>{glyph(icon, "text-[17px]")}</span>
            <span class="min-w-0 flex-1">
                <span class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-[13px] font-medium text-gc-text">{name}</span>
                <span class="block truncate text-[11.5px] text-gc-text-muted">{desc}</span>
            </span>
            {right}
        </div>
    }
}

fn list_page(t: T) -> impl IntoView {
    let lit = move |i: usize| Signal::derive(move || t.lit.get() == Lit::Row(i));
    let software = format!(
        "Linux box · {} · {} · {} · {}",
        Tool::BuwizForms.name(),
        Tool::BuwizBooks.name(),
        Tool::Chromium.name(),
        Tool::Office.name()
    );
    view! {
        <div class="grid shrink-0 justify-items-center gap-1 text-center">
            {avatar("h-14 w-14 text-[18px]")}
            <div class="mt-1 text-[17px] font-semibold leading-tight text-gc-text">"Buwiz Clerk"</div>
            <div class="text-[12px] text-gc-text-muted">{format!("Last updated {LAST_UPDATED}")}</div>
            <p class="max-w-[44ch] text-[12.5px] text-gc-text-muted">{DESCRIPTION}</p>
        </div>
        <div class="grid gap-0.5">
            {entry(t.rows[0], "psychology", "Context", "Instructions, 4 memories, 6 skills".to_string(), lit(0), chevron())}
            {entry(t.rows[1], "hub", "Integrations", "Gmail, Google Drive".to_string(), lit(1), chevron())}
            {entry(
                t.rows[2],
                "desktop_windows",
                view! { <span class="whitespace-nowrap">"Computer & software"</span><span class=TAG>{COMPUTER_TAG}</span> },
                software,
                lit(2),
                chevron(),
            )}
            {entry(
                NodeRef::new(),
                "shield_lock",
                "What stays private",
                "Your logins, vault, chats and files".to_string(),
                Signal::stored(false),
                view! { <span class="grid shrink-0 place-items-center text-gc-text-muted">{glyph("lock", "text-[16px]")}</span> },
            )}
        </div>
    }
}

fn context_page() -> impl IntoView {
    view! {
        <div class="grid shrink-0 justify-items-center">{avatar("h-10 w-10 text-[13px]")}</div>
        <div class=SEC>"Instructions"</div>
        <div class="rounded-xl bg-gc-bg-muted px-3 py-2.5 text-[13px] text-gc-text">{INSTRUCTIONS}</div>
        <div class=SEC>"Memories (4)"</div>
        <div class=BOX>
            {MEMORIES.into_iter().map(|m| view! { <div class=ROW>{m}</div> }).collect_view()}
        </div>
    }
}

fn integrations_page() -> impl IntoView {
    let rows = [("G", "Gmail", "Search, read and draft email."), ("D", "Google Drive", "Find and open shared files.")];
    view! {
        <div class=BOX>
            {rows.into_iter().map(|(tile, name, sub)| view! {
                <div class=ROW>
                    <span class=format!("{TILE} h-[30px] w-[30px] text-[12px] font-semibold")>{tile}</span>
                    <div class="min-w-0 flex-1">
                        <div class="font-medium">{name}</div>
                        <div class="truncate text-[11.5px] text-gc-text-muted">{sub}</div>
                    </div>
                </div>
            }).collect_view()}
        </div>
    }
}

fn computer_page(t: T) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center gap-2 text-[12.5px] text-gc-text-muted">
            "Installed with the copy. Logins are not included."
            <span class=TAG>{COMPUTER_TAG}</span>
        </div>
        <div class=BOX>
            {CARRIED.into_iter().enumerate().map(|(i, tool)| {
                let (tile, name, sub) = match tool {
                    Some(k) => (tool_icon(k, IconSize::Tile).into_any(), k.name(), k.sub()),
                    None => (
                        view! { <span class=format!("{TILE} h-[34px] w-[34px] rounded-[9px] text-gc-text-muted")>{glyph("menu_book", "text-[18px]")}</span> }.into_any(),
                        "3 recipes",
                        "File 2550Q · Book receipts · Month-end close",
                    ),
                };
                view! {
                    <div class=ROW>
                        {tile}
                        <div class="min-w-0 flex-1">
                            <div class="font-medium">{name}</div>
                            <div class="truncate text-[11.5px] text-gc-text-muted">{sub}</div>
                        </div>
                        <Show when=move || { t.ticks.get() > i }>{check()}</Show>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// View Details: what the template carries, with Context, Integrations and Computer & software
/// pages. Always mounted, so the cursor can measure its rows; the scrim fades it in and out.
fn details_modal(t: T) -> impl IntoView {
    let title = move || match t.page.get() {
        Page::List => "",
        Page::Context => "Context",
        Page::Integrations => "Integrations",
        Page::Computer => "Computer & software",
    };
    view! {
        <div class=move || if t.open.get() {
            "pointer-events-auto absolute inset-0 z-10 grid place-items-center bg-black/35 opacity-100 transition-opacity duration-300"
        } else {
            "pointer-events-none absolute inset-0 z-10 grid place-items-center bg-black/35 opacity-0 transition-opacity duration-300"
        }>
            <div class="flex h-[calc(100%-16px)] w-[calc(100%-16px)] flex-col overflow-hidden rounded-2xl border border-gc-border bg-gc-bg-elevated shadow-2xl sm:h-[min(600px,92%)] sm:w-[min(640px,92%)]">
                <div class="grid shrink-0 grid-cols-[28px_minmax(0,1fr)_28px] items-center gap-2 px-4 pb-2 pt-3.5 sm:px-5">
                    <div node_ref=t.back class=move || if t.page.get() == Page::List { "hidden" } else { "col-start-1 grid h-7 w-7 shrink-0 place-items-center rounded-lg text-gc-text-muted" }>
                        {glyph("chevron_left", "text-[20px]")}
                    </div>
                    <div class="col-start-2 truncate text-center text-[15px] font-semibold text-gc-text">{title}</div>
                    <div class=format!("{ICO} col-start-3")>{glyph("close", "text-[18px]")}</div>
                </div>
                <div class="flex min-h-0 flex-1 flex-col overflow-hidden px-4 pb-4 sm:px-5">
                    {move || {
                        let page = t.page.get();
                        let slide = if t.back_dir.get_untracked() { "gc-slide-back" } else { "gc-slide-in" };
                        view! {
                            <div class=format!("flex min-h-0 flex-1 flex-col gap-2.5 {slide}")>
                                {match page {
                                    Page::List => list_page(t).into_any(),
                                    Page::Context => context_page().into_any(),
                                    Page::Integrations => integrations_page().into_any(),
                                    Page::Computer => computer_page(t).into_any(),
                                }}
                            </div>
                        }
                    }}
                </div>
                <div class="flex shrink-0 justify-end border-t border-gc-border px-4 py-3 sm:px-5">
                    <div node_ref=t.publish class=PUBLISH>"Publish"</div>
                </div>
            </div>
        </div>
    }
}

/// The template's public page, as the teammate sees it in their browser. Its colors are the page's.
fn template_page(t: T) -> impl IntoView {
    view! {
        <div class="gc-card-in absolute inset-x-3 inset-y-[12%] z-20 flex flex-col overflow-hidden rounded-xl bg-white text-[#1f2328] shadow-[0_30px_80px_rgba(0,0,0,.45)] sm:inset-x-[14%] sm:inset-y-[8%]">
            <div class="flex items-center gap-1.5 bg-[#f1f3f4] px-2.5 py-1.5 font-mono text-[11.5px] text-[#57606a]">
                <span class="grid shrink-0 place-items-center">{glyph("lock", "text-[14px]")}</span>
                <span class="min-w-0 flex-1 truncate rounded-full bg-white px-2.5 py-0.5">{TEMPLATE_URL}</span>
            </div>
            <div class="grid min-h-0 flex-1 content-start gap-3 overflow-hidden p-4 sm:p-6">
                <div class="flex min-w-0 items-center gap-3">
                    {avatar("h-10 w-10 text-[13px]")}
                    <div class="min-w-0">
                        <div class="truncate text-[18px] font-bold leading-tight">"Buwiz Clerk"</div>
                        <div class="text-[12px] text-[#57606a]">"by Goldcoders"</div>
                    </div>
                </div>
                <p class="text-[13px]">"Files BIR forms in Buwiz Forms and books entries in Buwiz Books."</p>
                <div class="flex flex-wrap gap-1.5">
                    {["6 skills", "Gmail", "Google Drive"].into_iter().map(|p| view! {
                        <span class="rounded-full border border-[#e5e7eb] px-2.5 py-0.5 text-[12px]">{p}</span>
                    }).collect_view()}
                </div>
                <div class="grid gap-2">
                    <div class="flex items-center gap-2 text-[12px] font-semibold">
                        "Computer & software"
                        <span class="rounded-full border border-[#d0d7de] px-2 py-0.5 text-[11px] font-semibold">{COMPUTER_TAG}</span>
                    </div>
                    <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
                        {[Tool::BuwizForms, Tool::BuwizBooks, Tool::Chromium, Tool::Office].into_iter().map(|k| view! {
                            <div class="flex min-w-0 items-center gap-1.5 text-[12px]">
                                {tool_icon(k, IconSize::Mini)}
                                <span class="truncate">{k.name()}</span>
                            </div>
                        }).collect_view()}
                    </div>
                </div>
                <p class="text-[12px] text-[#57606a]">{CLAIM_LOGINS}</p>
                <div node_ref=t.use_btn class="rounded-lg bg-[#1f2328] py-2 text-center text-[13px] font-semibold text-white">"Use this coworker"</div>
            </div>
        </div>
    }
}

fn overlay(t: T) -> impl IntoView {
    view! {
        // The share menu, under the share icon: the strip below `md`, the pane above.
        <Show when=move || t.menu.get()>
            <div class="gc-card-in absolute right-3 top-[110px] z-[12] w-[240px] max-w-[calc(100%-16px)] rounded-xl border border-gc-border bg-gc-bg-elevated p-1.5 shadow-2xl md:top-12">
                <div
                    node_ref=t.menu_copy
                    class=move || if t.lit.get() == Lit::Menu {
                        "flex items-center gap-2.5 rounded-lg bg-gc-bg-hover px-2 py-1.5"
                    } else {
                        "flex items-center gap-2.5 rounded-lg px-2 py-1.5"
                    }
                >
                    <span class="grid h-7 w-7 shrink-0 place-items-center text-gc-text-muted">{glyph("content_copy", "text-[18px]")}</span>
                    <span class="min-w-0">
                        <span class="block text-[13px] font-medium text-gc-text">"Create a copy"</span>
                        <span class="block text-[11.5px] text-gc-text-muted">"Share a template link"</span>
                    </span>
                </div>
            </div>
        </Show>
        {details_modal(t)}
        <Show when=move || t.web.get()>{template_page(t)}</Show>
        <div
            role="status"
            class=move || if t.toast.get() {
                "pointer-events-none absolute bottom-20 left-1/2 z-[25] -translate-x-1/2 whitespace-nowrap rounded-lg bg-gc-text px-3 py-2 text-[13px] text-gc-bg opacity-100 transition-opacity duration-200"
            } else {
                "pointer-events-none absolute bottom-20 left-1/2 z-[25] -translate-x-1/2 whitespace-nowrap rounded-lg bg-gc-text px-3 py-2 text-[13px] text-gc-bg opacity-0 transition-opacity duration-200"
            }
        >
            "Link copied"
        </div>
        // The cut between the owner's app and the teammate's.
        <div class=move || if t.cut.get() {
            "pointer-events-none absolute inset-0 z-[31] bg-gc-bg opacity-100 transition-opacity duration-300"
        } else {
            "pointer-events-none absolute inset-0 z-[31] bg-gc-bg opacity-0 transition-opacity duration-300"
        }></div>
        <DemoPointer cursor=t.cur />
    }
}

#[component]
pub fn AgentTemplates() -> impl IntoView {
    let win = NodeRef::<html::Div>::new();
    let tl = Timeline::new();
    tl.speed.set(2.);
    let share_lit = RwSignal::new(false);
    let share = PaneIcon::new().lit(share_lit);
    let t = T {
        tl,
        cur: DemoCursor::new(tl, win, (0.5, 0.86)),
        step: RwSignal::new(None),
        caption: RwSignal::new(""),
        msgs: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
        draft: RwSignal::new(""),
        who: RwSignal::new(BUWIZ_CLERK),
        rail: RwSignal::new(OWNER_RAIL),
        routine: RwSignal::new(OWNER_ROUTINE),
        menu: RwSignal::new(false),
        share_lit,
        lit: RwSignal::new(Lit::None),
        open: RwSignal::new(false),
        page: RwSignal::new(Page::List),
        back_dir: RwSignal::new(false),
        ticks: RwSignal::new(0),
        published: RwSignal::new(false),
        flip: RwSignal::new(Flip::None),
        copied: RwSignal::new(false),
        toast: RwSignal::new(false),
        cut: RwSignal::new(false),
        web: RwSignal::new(false),
        signed: RwSignal::new([false, false]),
        desk: RwSignal::new(Desk::Owner),
        boot: RwSignal::new(Boot::Ready),
        dock_n: RwSignal::new(5),
        forms: RwSignal::new(false),
        screen: AgentScreen::new(),
        share,
        icons: PaneIcons { share: Some(share), ..PaneIcons::new() },
        win,
        scroller: NodeRef::new(),
        composer: NodeRef::new(),
        menu_copy: NodeRef::new(),
        details: NodeRef::new(),
        rows: std::array::from_fn(|_| NodeRef::new()),
        back: NodeRef::new(),
        publish: NodeRef::new(),
        copy: NodeRef::new(),
        use_btn: NodeRef::new(),
        sign: std::array::from_fn(|_| NodeRef::new()),
    };
    t.reset();
    t.step(0, C_SHARE);

    tl.script(move |s| script(s, t));
    autoplay_when_visible(tl, win);

    view! {
        <section id="templates" aria-labelledby="templates-title" class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card/40">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <h2 id="templates-title" class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Share your agents as templates."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[56ch]">
                        "A few clicks turn a coworker into a link. Whoever opens it gets their own copy: same instructions, memories, skills and integrations. "
                        {CLAIM_LOGINS}
                    </p>
                    <p class="mt-3 text-[13px] text-gc-text-muted">"Computer and software in templates: coming soon."</p>
                </div>
                <p class="sr-only">
                    "Share a coworker as a template link. A teammate opens the link, uses it, and gets their own copy with the same software, then signs in to their own accounts and files a form."
                </p>

                <DemoBar steps=STEPS current=t.step timeline=tl caption=t.caption caption_min="min-h-[2.75rem] sm:min-h-5" />

                <div aria-hidden="true">
                    <AgentWindow
                        win
                        screen=t.screen
                        who=t.who
                        rail=t.rail
                        routine=t.routine
                        icons=t.icons
                        scroller=t.scroller
                        composer=t.composer
                        draft=t.draft
                        desktop=move || desktop(t)
                        overlay=move || overlay(t)
                    >
                        <For each=move || t.msgs.get() key=|m| m.id children=move |m| message(t, m) />
                    </AgentWindow>
                </div>

            </div>
        </section>
    }
}
