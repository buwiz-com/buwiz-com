use leptos::{html, prelude::*};

use crate::components::agent_window::{
    AgentScreen, AgentWindow, FINANCE, FINANCE_RAIL, PaneIcon, PaneIcons, box_desktop, chrome, dock_apps, svg_globe,
};
use crate::components::demo_player::{
    DemoBar, DemoCursor, DemoPointer, Script, Timeline, autoplay_when_visible, in_view,
};

/// Egress, inside the NativeChat window: rail, chat, and Finance's computer pane. Finance's
/// computer runs in a datacenter, so a Facebook login there trips new-location checks. Routing
/// its traffic through the visitor's desktop makes the same login go straight through.
///
/// Two variants: the network rule is Ask (Finance asks in the chat) or Never (Finance can't ask,
/// so the visitor clicks the globe). The screen grows out of the pane to fill the window and
/// shrinks back. The app chrome uses the site's theme tokens; the Box and Facebook keep their
/// own colors. Every control is a drawn box, never a real input.
const STEPS_A: &[&str] = &["Why it matters", "Stuck", "Ask in chat", "Allow once", "Posted"];
const STEPS_B: &[&str] = &["Blocked", "Can't ask", "Click the globe", "Try again", "Posted"];

const ASK: &str = "Post the launch update on our Facebook page";
const EMAIL: &str = "finance@goldcoders.dev";
const DOTS: &str = "••••••••••";
const CODE: &str = "482913";

const TIP_OFF: &str =
    "Traffic goes out on its own. Click to route web traffic from all your computers through this desktop instead.";
const TIP_ON: &str =
    "Routing traffic through this desktop for all your computers. New connections go out through it. Click to stop.";
const TIP_NEVER: &str = "Use your network from this computer: Never. Click to change. Applies to this computer only.";
const TIP_ASK: &str = "Use your network from this computer: Ask. Click to change. Applies to this computer only.";

const FB_FONT: &str = "font-family:Helvetica,Arial,sans-serif;";

#[derive(Clone, Copy, PartialEq)]
enum Path {
    /// Network rule Ask: Finance asks with a permission card.
    A,
    /// Network rule Never: Finance can't ask; the visitor clicks the globe.
    B,
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Login,
    NewPlace,
    Code,
    Approve,
    Posted,
}

#[derive(Clone, Copy, PartialEq)]
enum Tip {
    Globe,
    Shield,
}

#[derive(Clone)]
struct Msg {
    id: u32,
    body: Body,
}

#[derive(Clone)]
enum Body {
    Me(&'static str),
    Fi(&'static str),
    /// Finance can't get past the phone approval.
    Stuck,
    /// Blocked, and the network rule is Never.
    Never,
    /// The permission card, and which chip was picked.
    Perm(ArcRwSignal<Option<usize>>),
    /// Try again / Not now, and which was picked.
    Retry(ArcRwSignal<Option<usize>>),
}

#[derive(Clone, Copy)]
struct E {
    tl: Timeline,
    cur: DemoCursor,
    path: RwSignal<Path>,
    step: RwSignal<Option<usize>>,
    caption: RwSignal<&'static str>,
    msgs: RwSignal<Vec<Msg>>,
    next: StoredValue<u32>,
    draft: RwSignal<&'static str>,
    route: RwSignal<bool>,
    pulse: RwSignal<bool>,
    tip: RwSignal<Option<Tip>>,
    // Facebook, on the Box.
    page: RwSignal<Page>,
    email: RwSignal<&'static str>,
    pass: RwSignal<&'static str>,
    code: RwSignal<&'static str>,
    pressed: RwSignal<bool>,
    // The screen growing out of the pane, and the pane's toolbar.
    screen: AgentScreen,
    icons: PaneIcons,
    win: NodeRef<html::Div>,
    scroller: NodeRef<html::Div>,
    composer: NodeRef<html::Div>,
    allow_chip: NodeRef<html::Div>,
    retry_chip: NodeRef<html::Div>,
}

impl E {
    fn reset(self) {
        self.msgs.set(Vec::new());
        self.draft.set("");
        self.route.set(false);
        self.pulse.set(false);
        self.tip.set(None);
        self.fb(Page::Login);
        self.screen.reset();
        self.cur.reset();
    }

    fn step(self, n: usize, caption: &'static str) {
        self.step.set(Some(n));
        self.caption.set(caption);
    }

    /// Facebook shows `page` with an empty form.
    fn fb(self, page: Page) {
        self.page.set(page);
        self.email.set("");
        self.pass.set("");
        self.code.set("");
        self.pressed.set(false);
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
        let scroller = self.scroller;
        request_animation_frame(move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    fn route_on(self) {
        self.route.set(true);
        self.pulse.set(true);
        self.tl.after(1900, move || self.pulse.set(false));
    }

    /// The thumbnail grows to fill the window.
    fn expand(self) {
        self.screen.expand();
    }

    /// And back into the pane.
    fn shrink(self) {
        self.screen.shrink();
    }
}

fn tip_text(e: E) -> Option<&'static str> {
    e.tip.get().map(|t| match t {
        Tip::Globe if e.route.get() => TIP_ON,
        Tip::Globe => TIP_OFF,
        Tip::Shield if e.path.get() == Path::B => TIP_NEVER,
        Tip::Shield => TIP_ASK,
    })
}

/// Chrome on the Box, showing Facebook.
fn fb_chrome(e: E) -> impl IntoView {
    let url = move || match e.page.get() {
        Page::Login => "/login",
        Page::NewPlace => "/checkpoint/?next",
        Page::Code => "/checkpoint/email",
        Page::Approve => "/checkpoint/approve",
        Page::Posted => "/goldcoders",
    };
    let tab = move || match e.page.get() {
        Page::Login => "Log in to Facebook",
        Page::Posted => "Goldcoders | Facebook",
        _ => "Security check | Facebook",
    };
    chrome(
        view! { <span class="grid h-3.5 w-3.5 shrink-0 place-items-center rounded-full bg-[#1877F2] text-[9px] font-bold leading-none text-white" style=FB_FONT>"f"</span> },
        tab,
        "facebook.com",
        url,
        "bg-[#f0f2f5] text-[#1c1e21]",
        FB_FONT,
        move || match e.page.get() {
            Page::Login => fb_login(e).into_any(),
            Page::NewPlace => {
                fb_check(e, "We noticed a login from a new location", 2, fb_new_place().into_any()).into_any()
            }
            Page::Code => fb_check(e, "Check your email", 3, fb_code(e).into_any()).into_any(),
            Page::Approve => fb_check(e, "Approve this login on your phone", 4, fb_approve().into_any()).into_any(),
            Page::Posted => fb_posted().into_any(),
        },
    )
}

const FB_FIELD: &str = "flex h-10 items-center rounded-md border border-[#dddfe2] bg-white px-3 text-[14px]";

fn fb_login(e: E) -> impl IntoView {
    view! {
        <div class="flex h-full flex-col items-center justify-center gap-3 px-4">
            <div class="text-[40px] font-bold leading-none tracking-[-1.5px] text-[#1877F2]">"facebook"</div>
            <div class="flex w-[min(320px,100%)] flex-col gap-2.5 rounded-lg bg-white p-4 shadow-[0_2px_4px_rgba(0,0,0,.1),0_8px_16px_rgba(0,0,0,.1)]">
                <div class=FB_FIELD>
                    {move || match e.email.get() {
                        "" => view! { <span class="text-[#8a8d91]">"Email address or phone number"</span> }.into_any(),
                        t => view! { <span>{t}</span> }.into_any(),
                    }}
                </div>
                <div class=FB_FIELD>
                    {move || match e.pass.get() {
                        "" => view! { <span class="text-[#8a8d91]">"Password"</span> }.into_any(),
                        t => view! { <span class="tracking-[2px]">{t}</span> }.into_any(),
                    }}
                </div>
                <div class=move || if e.pressed.get() {
                    "flex h-10 items-center justify-center rounded-md bg-[#1464cf] text-[17px] font-bold text-white"
                } else {
                    "flex h-10 items-center justify-center rounded-md bg-[#1877F2] text-[17px] font-bold text-white"
                }>"Log in"</div>
                <div class="text-center text-[12.5px] text-[#1877F2]">"Forgotten password?"</div>
                <div class="h-px bg-[#dadde1]"></div>
                <div class="mx-auto rounded-md bg-[#42b72a] px-4 py-2 text-[14px] font-bold text-white">"Create new account"</div>
            </div>
        </div>
    }
}

/// A checkpoint step: Facebook's bar, a card, and step n of 5.
fn fb_check(e: E, title: &'static str, n: u8, body: AnyView) -> impl IntoView {
    let waiting = n == 4;
    view! {
        <div class="flex h-11 items-center bg-white px-4 shadow-[0_1px_2px_rgba(0,0,0,.1)]">
            <span class="text-[24px] font-bold tracking-[-1px] text-[#1877F2]">"facebook"</span>
        </div>
        <div class="flex justify-center p-4">
            <div class="w-[min(470px,100%)] overflow-hidden rounded-lg bg-white shadow-[0_1px_2px_rgba(0,0,0,.15)]">
                <div class="border-b border-[#dadde1] px-4 py-3 text-[17px] font-bold">{title}</div>
                <div class="flex flex-col gap-3 p-4 text-[13.5px] leading-snug text-[#606770]">{body}</div>
                <div class="flex items-center justify-between gap-2 border-t border-[#dadde1] px-4 py-3">
                    <span class="text-[12px] text-[#8a8d91]">{format!("Step {n} of 5")}{if waiting { " · waiting…" } else { "" }}</span>
                    {if waiting {
                        view! { <span class="text-[13px] font-semibold text-[#1877F2]">"Try another way"</span> }.into_any()
                    } else {
                        view! {
                            <span class=move || if e.pressed.get() {
                                "rounded-md bg-[#1464cf] px-4 py-1.5 text-[13.5px] font-semibold text-white"
                            } else {
                                "rounded-md bg-[#1877F2] px-4 py-1.5 text-[13.5px] font-semibold text-white"
                            }>"Continue"</span>
                        }
                        .into_any()
                    }}
                </div>
            </div>
        </div>
    }
}

fn fb_new_place() -> impl IntoView {
    view! {
        <span>"Your password was just used from a place you don't usually log in from. To keep your account secure, confirm it's you."</span>
        <div class="flex items-center gap-3 rounded-md bg-[#f0f2f5] p-3">
            <span class="material-symbols-outlined text-[28px] text-[#1877F2]">"location_on"</span>
            <div>
                <div class="text-[14px] font-bold text-[#1c1e21]">"Singapore"</div>
                <div class="text-[12.5px]">"hosting network · Chrome on Linux · just now"</div>
            </div>
        </div>
    }
}

fn fb_code(e: E) -> impl IntoView {
    view! {
        <span>"We sent a 6-digit code to f•••••e@goldcoders.dev. Enter it to continue."</span>
        <div class="flex gap-1.5">
            {(0..6).map(|i| view! {
                <span class="grid h-10 w-9 place-items-center rounded-md border border-[#dddfe2] bg-white text-[18px] font-bold text-[#1c1e21]">
                    {move || e.code.get().chars().nth(i).map(|c| c.to_string()).unwrap_or_default()}
                </span>
            }).collect_view()}
        </div>
    }
}

fn fb_approve() -> impl IntoView {
    view! {
        <span>"We sent a notification to your iPhone. Open Facebook there and tap “Yes, it's me” to finish logging in."</span>
        <div class="flex items-center gap-3 rounded-md bg-[#f0f2f5] p-3">
            <span class="material-symbols-outlined text-[30px] text-[#1877F2]">"phone_iphone"</span>
            <span class="h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-[#1877F2]/25 border-t-[#1877F2]"></span>
            <span class="text-[13px] font-semibold text-[#1c1e21]">"Waiting for approval…"</span>
        </div>
    }
}

fn fb_posted() -> impl IntoView {
    view! {
        <div class="flex h-11 items-center gap-3 bg-white px-3 shadow-[0_1px_2px_rgba(0,0,0,.1)]">
            <span class="grid h-8 w-8 shrink-0 place-items-center rounded-full bg-[#1877F2] text-[22px] font-bold leading-none text-white">"f"</span>
            <span class="flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-full bg-[#f0f2f5] px-3 text-[13px] text-[#65676b] sm:max-w-[220px]">
                <span class="material-symbols-outlined text-[16px]">"search"</span>
                <span class="truncate">"Search Facebook"</span>
            </span>
            <span class="ml-auto hidden items-center gap-6 text-[#65676b] sm:flex">
                <span class="material-symbols-outlined text-[22px] text-[#1877F2]">"home"</span>
                <span class="material-symbols-outlined text-[22px]">"ondemand_video"</span>
                <span class="material-symbols-outlined text-[22px]">"storefront"</span>
                <span class="material-symbols-outlined text-[22px]">"groups"</span>
            </span>
            <span class="ml-auto grid h-8 w-8 shrink-0 place-items-center rounded-full bg-[#f0a14a] text-[11px] font-bold text-[#1a1a1a] sm:ml-6">"Fi"</span>
        </div>
        <div class="mx-auto flex w-[min(520px,100%)] flex-col gap-3 p-3">
            <div class="overflow-hidden rounded-lg bg-white shadow-[0_1px_2px_rgba(0,0,0,.15)]">
                <div class="h-14" style="background:linear-gradient(120deg,#1e3a8a,#7c3aed 55%,#f59e0b)"></div>
                <div class="flex items-end gap-3 px-4 pb-3">
                    <span class="-mt-6 grid h-14 w-14 shrink-0 place-items-center rounded-full border-4 border-white bg-[#0a0a0f] text-[15px] font-bold text-[#f59e0b]">"GC"</span>
                    <div class="min-w-0">
                        <div class="text-[17px] font-bold">"Goldcoders"</div>
                        <div class="text-[12px] text-[#65676b]">"Page · Software company"</div>
                    </div>
                </div>
            </div>
            <div class="rounded-lg bg-white p-3 shadow-[0_1px_2px_rgba(0,0,0,.15)]">
                <div class="flex items-center gap-2">
                    <span class="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-[#0a0a0f] text-[11px] font-bold text-[#f59e0b]">"GC"</span>
                    <div>
                        <div class="text-[14px] font-bold">"Goldcoders"</div>
                        <div class="flex items-center gap-1 text-[12px] text-[#65676b]">
                            "Just now · "
                            <span class="material-symbols-outlined text-[12px]">"public"</span>
                        </div>
                    </div>
                </div>
                <p class="mt-2 text-[14px] leading-snug">"Launch update: our new site is live. Coworkers that bring their own computer, and go online through yours. Thanks for following along."</p>
                <div class="mt-3 flex justify-around border-t border-[#dadde1] pt-2 text-[13px] font-semibold text-[#65676b]">
                    <span>"Like"</span>
                    <span>"Comment"</span>
                    <span>"Share"</span>
                </div>
            </div>
        </div>
        <div class="gc-card-in absolute bottom-3 left-3 flex items-center gap-2 rounded-lg bg-[#242526] px-3 py-2 text-[13px] text-white shadow-lg">
            <span class="text-[#45bd62]">"✓"</span>
            "Your post is now published."
        </div>
    }
}
/// The Box's desktop with Chrome open. Laid out at its real size in the expanded view, and
/// scaled down in the thumbnail.
fn box_screen(e: E) -> impl IntoView {
    box_desktop(
        "Buwiz Bot Box · Finance",
        view! {
            <span class=move || if e.route.get() {
                "whitespace-nowrap rounded-full bg-[#10b981]/25 px-2 py-0.5 text-[10px] font-semibold text-[#6ee7b7]"
            } else {
                "whitespace-nowrap rounded-full bg-white/10 px-2 py-0.5 text-[10px] text-white/70"
            }>
                {move || if e.route.get() { "⇄ via your desktop" } else { "datacenter IP" }}
            </span>
        },
        fb_chrome(e),
        dock_apps(),
    )
}

const OPT: &str = "cursor-default rounded-full border border-gc-border bg-gc-bg-card px-3.5 py-1.5 text-[13px] font-medium text-gc-text";
const OPT_PICKED: &str =
    "rounded-full border border-gc-text bg-gc-text px-3.5 py-1.5 text-[13px] font-medium text-gc-bg";
const OPT_OFF: &str =
    "rounded-full border border-gc-border bg-gc-bg-card px-3.5 py-1.5 text-[13px] font-medium text-gc-text opacity-45";

fn chips(
    picked: ArcRwSignal<Option<usize>>,
    labels: &'static [&'static str],
    aim: NodeRef<html::Div>,
) -> impl IntoView {
    let picked = RwSignal::from(picked);
    view! {
        <div class="flex flex-wrap gap-2">
            {labels.iter().enumerate().map(|(i, l)| view! {
                <div
                    node_ref={if i == 0 { aim } else { NodeRef::new() }}
                    class=move || match picked.get() {
                        None => OPT,
                        Some(k) if k == i => OPT_PICKED,
                        _ => OPT_OFF,
                    }
                >
                    {*l}
                </div>
            }).collect_view()}
        </div>
    }
}

const BUBBLE_FI: &str = "gc-card-in max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text";
const BUBBLE_WARN: &str = "gc-card-in max-w-[88%] self-start rounded-2xl border border-gc-error/40 bg-gc-error/[0.07] px-3.5 py-2.5 text-gc-text";

fn message(e: E, m: Msg) -> AnyView {
    match m.body {
        Body::Me(text) => view! {
            <div class="gc-card-in max-w-[88%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg">{text}</div>
        }
        .into_any(),
        Body::Fi(text) => view! { <div class=BUBBLE_FI>{text}</div> }.into_any(),
        Body::Stuck => view! {
            <div class=BUBBLE_WARN>"Stuck at ‘Approve on your phone’. I can’t finish this login."</div>
        }
        .into_any(),
        Body::Never => view! {
            <div class=BUBBLE_WARN>
                "Blocked: datacenter IP. Your network rule is "
                <b>"Never"</b>
                ", so I can’t ask. Click "
                {svg_globe("inline-block h-[15px] w-[15px] align-[-3px] text-gc-text-muted")}
                " in my computer pane to route through your desktop."
            </div>
        }
        .into_any(),
        Body::Perm(picked) => view! {
            <div class="gc-card-in grid w-full max-w-[440px] gap-2 self-start rounded-2xl border border-gc-border bg-gc-bg-card p-3.5">
                <div class="flex items-center gap-2 text-[13px] font-semibold text-gc-text">
                    <span class="h-2 w-2 rounded-full bg-gc-gold"></span>
                    "Permission · Finance"
                </div>
                <div class="text-[13.5px] text-gc-text-muted">"Use your internet connection for Finance’s computer? Sites will see your IP, not a datacenter’s."</div>
                {chips(picked, &["Allow once", "Always", "Deny"], e.allow_chip)}
            </div>
        }
        .into_any(),
        Body::Retry(picked) => view! {
            <div class="gc-card-in self-start">{chips(picked, &["Try again", "Not now"], e.retry_chip)}</div>
        }
        .into_any(),
    }
}

/// Ask Finance, then watch the screen grow and Finance log in to Facebook.
fn ask_and_login(s: &mut Script, e: E) {
    s.wait(400)
        .move_to(e.cur, e.composer)
        .typing(ASK, 22, move |t| e.draft.set(t))
        .wait(250)
        .then(move || {
            e.draft.set("");
            e.push(Body::Me(ASK));
        })
        .wait(550)
        .then(move || e.push(Body::Fi("On it. Signing in to Facebook first.")))
        .wait(700)
        .then(move || {
            e.fb(Page::Login);
            e.expand();
        })
        .wait(550);
    log_in(s, e);
}

/// Finance fills Facebook's login and presses Log in.
fn log_in(s: &mut Script, e: E) {
    s.typing(EMAIL, 16, move |t| e.email.set(t))
        .wait(120)
        .typing(DOTS, 30, move |t| e.pass.set(t))
        .wait(250)
        .then(move || e.pressed.set(true))
        .wait(220);
}

/// Finance presses Continue on a checkpoint, and the next one loads.
fn next_page(s: &mut Script, e: E, page: Page) {
    s.then(move || e.pressed.set(true)).wait(220).then(move || e.fb(page));
}

/// With routing on: the same login goes straight through, the post goes up, the screen shrinks.
fn straight_in(s: &mut Script, e: E) {
    s.then(move || {
        e.fb(Page::Login);
        e.expand();
    })
    .wait(550);
    log_in(s, e);
    s.then(move || {
        e.fb(Page::Posted);
        e.caption.set("Same login from your IP looks like you at your own desk. Straight in, no checkpoint.");
    })
    .wait(2400)
    .then(move || e.shrink())
    .wait(500)
    .then(move || {
        e.step(4, "Posted. No extra checks.");
        e.push(Body::Fi("Posted. No extra checks."));
    })
    .move_home(e.cur)
    .wait(4200)
    .then(move || {
        if in_view(e.win) {
            e.tl.play();
        }
    });
}

fn path_a(s: &mut Script, e: E) {
    e.step(0, "Why it matters: Finance's computer runs in a datacenter. Watch an ordinary Facebook login.");
    ask_and_login(s, e);
    s.then(move || {
        e.fb(Page::NewPlace);
        e.caption
            .set("Right password, wrong place. Facebook sees a hosting network in Singapore and flags a new location.");
    })
    .wait(1700);
    next_page(s, e, Page::Code);
    s.wait(300).typing(CODE, 90, move |t| e.code.set(t)).wait(250);
    next_page(s, e, Page::Approve);
    s.then(move || e.caption.set("Then an email code, then approval on a phone the agent doesn't have. If your phone isn't with you, it stops here."))
        .wait(2000)
        .then(move || e.shrink())
        .wait(500)
        .then(move || {
            e.step(1, "Stuck. The site wants a person on a home connection, not a datacenter.");
            e.push(Body::Stuck);
        })
        .wait(1400);

    let perm = ArcRwSignal::new(None::<usize>);
    let card = perm.clone();
    s.then(move || {
        e.step(2, "Your network rule is Ask, so Finance asks for your connection right in the chat.");
        e.push(Body::Perm(card));
    })
    .wait(1500)
    .click(e.cur, e.allow_chip, move || {
        perm.set(Some(0));
        e.route_on();
        e.step(3, "Allowed once. The globe in the pane turns into green route arrows by itself: traffic now goes out through your desktop.");
    })
    .wait(1100)
    .then(move || e.push(Body::Fi("Routing through your desktop. Trying again.")))
    .wait(800);
    straight_in(s, e);
}

fn path_b(s: &mut Script, e: E) {
    e.step(0, "Finance's computer runs in a datacenter. Watch the Facebook login.");
    ask_and_login(s, e);
    s.then(move || {
        e.fb(Page::NewPlace);
        e.caption.set("Facebook flags a new location: a hosting network in Singapore.");
    })
    .wait(1500);
    next_page(s, e, Page::Approve);
    s.then(move || e.caption.set("Then it wants approval on a phone the agent doesn't have."))
        .wait(1500)
        .then(move || e.shrink())
        .wait(500)
        .then(move || {
            e.step(1, "Your network rule is Never, so Finance can't ask for your connection. It points you at the globe instead.");
            e.push(Body::Never);
        })
        .wait(1700)
        .move_to(e.cur, e.icons.shield.aim())
        .then(move || e.tip.set(Some(Tip::Shield)))
        .wait(2000)
        .then(move || {
            e.tip.set(None);
            e.step(2, "The globe means traffic goes out on its own. Click it to route Finance's computer through your desktop.");
        })
        .move_to(e.cur, e.icons.globe.aim())
        .then(move || e.tip.set(Some(Tip::Globe)))
        .wait(1700)
        .press(e.cur)
        .wait(170)
        .then(move || e.route_on())
        .wait(1900);

    let retry = ArcRwSignal::new(None::<usize>);
    let card = retry.clone();
    s.then(move || {
        e.tip.set(None);
        e.push(Body::Retry(card));
    })
    .wait(900)
    .click(e.cur, e.retry_chip, move || {
        retry.set(Some(0));
        e.push(Body::Me("Try again"));
        e.step(3, "Try again. Same login, now from your IP.");
    })
    .wait(500)
    .then(move || e.push(Body::Fi("Routing through your desktop. Trying again.")))
    .wait(800);
    straight_in(s, e);
}

/// Your coworker's computer goes online through yours.
#[component]
pub fn Egress() -> impl IntoView {
    let win = NodeRef::<html::Div>::new();
    let tl = Timeline::new();
    let path = RwSignal::new(Path::A);
    let route = RwSignal::new(false);
    let pulse = RwSignal::new(false);
    let tip = RwSignal::new(None);
    let icons = PaneIcons {
        globe: PaneIcon::new()
            .tint(Signal::derive(move || if route.get() { "text-gc-accent" } else { "text-gc-text-muted" }))
            .lit(Signal::derive(move || tip.get() == Some(Tip::Globe)))
            .fx(Signal::derive(move || if pulse.get() { "gc-route-pulse" } else { "" })),
        shield: PaneIcon::new()
            .tint(Signal::derive(move || if path.get() == Path::B { "text-gc-error" } else { "text-gc-text-muted" }))
            .lit(Signal::derive(move || tip.get() == Some(Tip::Shield))),
        routed: route.into(),
        ..PaneIcons::new()
    };
    let e = E {
        tl,
        cur: DemoCursor::new(tl, win, (0.62, 0.93)),
        path,
        step: RwSignal::new(None),
        caption: RwSignal::new(""),
        msgs: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
        draft: RwSignal::new(""),
        route,
        pulse,
        tip,
        page: RwSignal::new(Page::Login),
        email: RwSignal::new(""),
        pass: RwSignal::new(""),
        code: RwSignal::new(""),
        pressed: RwSignal::new(false),
        screen: AgentScreen::new(),
        icons,
        win,
        scroller: NodeRef::new(),
        composer: NodeRef::new(),
        allow_chip: NodeRef::new(),
        retry_chip: NodeRef::new(),
    };

    tl.script(move |s| {
        e.reset();
        match e.path.get_untracked() {
            Path::A => path_a(s, e),
            Path::B => path_b(s, e),
        }
    });

    autoplay_when_visible(tl, win);

    let pick = move |p: Path| {
        e.path.set(p);
        tl.play();
    };
    let seg = move |p: Path| {
        move || {
            if e.path.get() == p {
                "rounded-full bg-gc-text px-3.5 py-1.5 text-[13px] font-semibold text-gc-bg"
            } else {
                "rounded-full px-3.5 py-1.5 text-[13px] font-semibold text-gc-text-muted hover:text-gc-text"
            }
        }
    };

    view! {
        <section id="egress" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <div class="text-[11px] font-bold tracking-wide uppercase text-gc-text-faint mb-3">"Egress"</div>
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Your IP, not a datacenter's."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[56ch]">
                        "Sites trust a home connection and question a datacenter. Route your coworker's computer through your own desktop, and its logins look like you at your desk: no new-location checks, no phone approvals."
                    </p>
                </div>

                <div class="mb-4 flex flex-wrap items-center gap-x-3 gap-y-2">
                    <span class="text-[13px] text-gc-text-muted">"Network rule on Finance's computer"</span>
                    <div class="inline-flex rounded-full border border-gc-border p-0.5" role="group" aria-label="Network rule">
                        <button type="button" class=seg(Path::A) aria-pressed=move || (e.path.get() == Path::A).to_string() on:click=move |_| pick(Path::A)>
                            "Ask · Path A"
                        </button>
                        <button type="button" class=seg(Path::B) aria-pressed=move || (e.path.get() == Path::B).to_string() on:click=move |_| pick(Path::B)>
                            "Never · Path B"
                        </button>
                    </div>
                </div>

                {move || {
                    let steps = if e.path.get() == Path::A { STEPS_A } else { STEPS_B };
                    view! { <DemoBar steps=steps current=e.step timeline=tl caption=e.caption /> }
                }}

                <AgentWindow
                    win
                    screen=e.screen
                    who=FINANCE
                    rail=FINANCE_RAIL
                    icons=e.icons
                    tip=Signal::derive(move || tip_text(e))
                    scroller=e.scroller
                    composer=e.composer
                    draft=e.draft
                    desktop=move || box_screen(e)
                    overlay=move || view! { <DemoPointer cursor=e.cur /> }
                >
                    <For each=move || e.msgs.get() key=|m| m.id children=move |m| message(e, m) />
                </AgentWindow>
            </div>
        </section>
    }
}
