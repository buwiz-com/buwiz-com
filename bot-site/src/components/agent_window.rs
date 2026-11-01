//! The NativeChat app window the home-page demos share: the coworker rail, the chat (header,
//! scrolling thread, composer) and the computer pane on the right (toolbar, the coworker's screen
//! as a thumbnail, Routines). The screen can grow out of the pane to fill the window and shrink
//! back, driven from a demo's script. A sheet (such as Settings) can cover the pane.
//!
//! One size everywhere: up to 1080px wide and 640px tall. Below `lg` the rail hides; below `md`
//! the pane folds into a strip under the chat header with a small thumbnail and the toolbar.
//!
//! The chrome uses the site's theme tokens. The screen keeps the Box's own colors.

use gloo_timers::callback::Timeout;
use leptos::{ev, html, prelude::*};

use crate::components::demo_player::Aim;

/// The screen is laid out at this size, then scaled into the pane's thumbnail.
const VW: f64 = 960.;
const VH: f64 = 600.;
/// How long the screen takes to grow out of the pane and to shrink back.
const GROW_MS: u32 = 380;
const SHRINK_MS: u32 = 340;

/// A coworker: rail avatar and chat header.
#[derive(Clone, Copy, PartialEq)]
pub struct Coworker {
    pub initials: &'static str,
    pub name: &'static str,
    /// Avatar background, a Tailwind class.
    pub color: &'static str,
}

pub const FINANCE: Coworker = Coworker { initials: "Fi", name: "Finance", color: "bg-[#f0a04b]" };
pub const GENIE: Coworker = Coworker { initials: "Ge", name: "Genie", color: "bg-[#b9a5ff]" };

const fn mate(initials: &'static str, color: &'static str) -> Coworker {
    Coworker { initials, name: initials, color }
}

/// The rail with Finance on it.
pub const FINANCE_RAIL: &[Coworker] = &[
    mate("OC", "bg-[#7dcea0]"),
    FINANCE,
    mate("Co", "bg-[#7c6cf0]"),
    mate("Vi", "bg-[#8b5cf6]"),
    mate("Tr", "bg-[#3b82f6]"),
];

/// The rail with Genie and Masterpowers on it.
pub const GENIE_RAIL: &[Coworker] = &[
    mate("OC", "bg-[#7dcea0]"),
    GENIE,
    mate("Ma", "bg-[#f472b6]"),
    mate("Co", "bg-[#7c6cf0]"),
    mate("Tr", "bg-[#3b82f6]"),
];

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    Closed,
    Open,
    Closing,
}

fn two_frames(f: impl FnOnce() + 'static) {
    request_animation_frame(move || request_animation_frame(f));
}

/// The coworker's screen: the pane's thumbnail and the full-window copy that grows out of it.
/// Create it in the section, hand it to `AgentWindow`, then call `expand` and `shrink` from the
/// script. With reduced motion it crossfades instead.
#[derive(Clone, Copy)]
pub struct AgentScreen {
    zoom: RwSignal<Zoom>,
    style: RwSignal<String>,
    run: StoredValue<u32>,
    reduced: bool,
    big: NodeRef<html::Div>,
    thumb_wide: NodeRef<html::Div>,
    thumb_strip: NodeRef<html::Div>,
}

impl AgentScreen {
    pub fn new() -> Self {
        let reduced =
            window().match_media("(prefers-reduced-motion: reduce)").ok().flatten().is_some_and(|m| m.matches());
        Self {
            zoom: RwSignal::new(Zoom::Closed),
            style: RwSignal::new(String::new()),
            run: StoredValue::new(0),
            reduced,
            big: NodeRef::new(),
            thumb_wide: NodeRef::new(),
            thumb_strip: NodeRef::new(),
        }
    }

    /// Grown, or growing, to fill the window.
    pub fn is_open(self) -> bool {
        self.zoom.get_untracked() == Zoom::Open
    }

    /// Back in the pane at once, for a reset.
    pub fn reset(self) {
        self.bump();
        self.zoom.set(Zoom::Closed);
        self.style.set(String::new());
    }

    /// Whichever thumbnail the layout shows, for the cursor.
    pub fn thumb(self) -> Aim {
        Aim::either(self.thumb_wide, self.thumb_strip)
    }

    fn bump(self) -> u32 {
        self.run
            .try_update_value(|g| {
                *g += 1;
                *g
            })
            .unwrap_or_default()
    }

    /// The transform that puts the full-window screen exactly over the visible thumbnail.
    fn over_thumb(self) -> String {
        let Some(o) = self.big.get_untracked().map(|b| b.get_bounding_client_rect()) else {
            return String::new();
        };
        let t = [self.thumb_wide, self.thumb_strip].into_iter().find_map(|r| {
            let rect = r.get_untracked()?.get_bounding_client_rect();
            (rect.width() > 0.).then_some(rect)
        });
        match t {
            Some(t) if o.width() > 0. && o.height() > 0. => format!(
                "transform:translate({:.1}px,{:.1}px) scale({:.4},{:.4});",
                t.left() - o.left(),
                t.top() - o.top(),
                t.width() / o.width(),
                t.height() / o.height()
            ),
            _ => String::new(),
        }
    }

    /// The thumbnail grows to fill the window (FLIP: start over the thumbnail, animate to none).
    pub fn expand(self) {
        let run = self.bump();
        let (g, style) = (self.run, self.style);
        let (from, to) = if self.reduced {
            ("opacity:0;transition:none;".to_string(), "opacity:1;transition:opacity 220ms ease-out;".to_string())
        } else {
            (
                format!("{}opacity:.55;transition:none;", self.over_thumb()),
                format!(
                    "transform:none;opacity:1;transition:transform {GROW_MS}ms cubic-bezier(.2,.8,.2,1),opacity 160ms ease-out;"
                ),
            )
        };
        style.set(from);
        self.zoom.set(Zoom::Open);
        two_frames(move || {
            if g.try_get_value() == Some(run) {
                style.set(to);
            }
        });
    }

    /// And back into the pane.
    pub fn shrink(self) {
        let run = self.bump();
        let to = if self.reduced {
            "opacity:0;transition:opacity 200ms ease-in;".to_string()
        } else {
            format!(
                "{}opacity:.4;transition:transform {SHRINK_MS}ms cubic-bezier(.4,0,.2,1),opacity {SHRINK_MS}ms ease-in;",
                self.over_thumb()
            )
        };
        self.zoom.set(Zoom::Closing);
        self.style.set(to);
        let (g, zoom, style) = (self.run, self.zoom, self.style);
        Timeout::new(SHRINK_MS + 30, move || {
            if g.try_get_value() == Some(run) {
                zoom.set(Zoom::Closed);
                style.set(String::new());
            }
        })
        .forget();
    }
}

impl Default for AgentScreen {
    fn default() -> Self {
        Self::new()
    }
}

/// One icon in the pane's toolbar. It exists twice, in the pane and in the narrow strip; `aim`
/// points the cursor at whichever is on screen.
#[derive(Clone, Copy)]
pub struct PaneIcon {
    pub wide: NodeRef<html::Div>,
    pub strip: NodeRef<html::Div>,
    /// Text color class.
    pub tint: Signal<&'static str>,
    /// The cursor is on it: hover background and a ring.
    pub lit: Signal<bool>,
    /// Extra classes, such as a pulse.
    pub fx: Signal<&'static str>,
}

impl PaneIcon {
    pub fn new() -> Self {
        Self {
            wide: NodeRef::new(),
            strip: NodeRef::new(),
            tint: Signal::stored("text-gc-text-muted"),
            lit: Signal::stored(false),
            fx: Signal::stored(""),
        }
    }

    pub fn aim(self) -> Aim {
        Aim::either(self.wide, self.strip)
    }

    pub fn tint(self, tint: impl Into<Signal<&'static str>>) -> Self {
        Self { tint: tint.into(), ..self }
    }

    pub fn lit(self, lit: impl Into<Signal<bool>>) -> Self {
        Self { lit: lit.into(), ..self }
    }

    pub fn fx(self, fx: impl Into<Signal<&'static str>>) -> Self {
        Self { fx: fx.into(), ..self }
    }
}

impl Default for PaneIcon {
    fn default() -> Self {
        Self::new()
    }
}

/// Monitor, plug (Plugins), wrench (Tools), globe (network route) and shield (network rule),
/// and a Settings gear when a demo opens the pane's Settings sheet.
#[derive(Clone, Copy)]
pub struct PaneIcons {
    pub monitor: PaneIcon,
    pub plug: PaneIcon,
    pub wrench: PaneIcon,
    pub globe: PaneIcon,
    pub shield: PaneIcon,
    pub gear: Option<PaneIcon>,
    /// Share the coworker as a template.
    pub share: Option<PaneIcon>,
    /// Traffic goes out through the visitor's desktop: the globe turns into route arrows.
    pub routed: Signal<bool>,
}

impl PaneIcons {
    pub fn new() -> Self {
        Self {
            monitor: PaneIcon::new(),
            plug: PaneIcon::new(),
            wrench: PaneIcon::new(),
            globe: PaneIcon::new(),
            shield: PaneIcon::new(),
            gear: None,
            share: None,
            routed: Signal::stored(false),
        }
    }
}

impl Default for PaneIcons {
    fn default() -> Self {
        Self::new()
    }
}

// Toolbar icons, as in NativeChat's computer pane.
fn svg_monitor() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"></rect><path d="M8 20h8M12 16v4"></path></svg> }
}
fn svg_plug() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 3v5M15 3v5M7 8h10v3a5 5 0 0 1-10 0zM12 16v5"></path></svg> }
}
fn svg_wrench() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"></path></svg> }
}
/// The globe, also drawn inline in chat text.
pub fn svg_globe(class: &'static str) -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class=class fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"></path></svg> }
}
fn svg_route() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 8h14l-3-3M20 16H6l3 3"></path></svg> }
}
fn svg_gear() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12.2 2h-.4a2 2 0 0 0-2 2v.2a2 2 0 0 1-1 1.7l-.4.3a2 2 0 0 1-2 0l-.2-.1a2 2 0 0 0-2.7.7l-.2.4a2 2 0 0 0 .7 2.7l.2.1a2 2 0 0 1 1 1.7v.5a2 2 0 0 1-1 1.7l-.2.1a2 2 0 0 0-.7 2.7l.2.4a2 2 0 0 0 2.7.7l.2-.1a2 2 0 0 1 2 0l.4.3a2 2 0 0 1 1 1.7v.2a2 2 0 0 0 2 2h.4a2 2 0 0 0 2-2v-.2a2 2 0 0 1 1-1.7l.4-.3a2 2 0 0 1 2 0l.2.1a2 2 0 0 0 2.7-.7l.2-.4a2 2 0 0 0-.7-2.7l-.2-.1a2 2 0 0 1-1-1.7v-.5a2 2 0 0 1 1-1.7l.2-.1a2 2 0 0 0 .7-2.7l-.2-.4a2 2 0 0 0-2.7-.7l-.2.1a2 2 0 0 1-2 0l-.4-.3a2 2 0 0 1-1-1.7V4a2 2 0 0 0-2-2z"></path><circle cx="12" cy="12" r="3"></circle></svg> }
}
fn svg_share() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v12M8 7l4-4 4 4"></path><path d="M5 12v7a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-7"></path></svg> }
}
fn svg_shield() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><path d="M12 3l8 3v6c0 5-4 8-8 9-4-1-8-4-8-9V6z"></path></svg> }
}

fn icon(i: PaneIcon, wide: bool, label: &'static str, glyph: impl IntoView + 'static) -> impl IntoView {
    let size = if wide { "h-8 w-8" } else { "h-7 w-7" };
    view! {
        <div
            node_ref=if wide { i.wide } else { i.strip }
            aria-label=label
            class=move || {
                let mut c = format!("relative grid shrink-0 place-items-center rounded-md transition-colors duration-200 {size} {}", i.tint.get());
                if i.lit.get() {
                    c.push_str(" bg-gc-bg-hover ring-2 ring-gc-brand/60");
                }
                let fx = i.fx.get();
                if !fx.is_empty() {
                    c.push(' ');
                    c.push_str(fx);
                }
                c
            }
        >
            {glyph}
        </div>
    }
}

/// Monitor, plug, wrench, globe (or route arrows), shield, then share and the gear when a demo
/// has them. `wide` is the pane; otherwise the
/// compact strip at narrow widths.
fn toolbar(icons: PaneIcons, wide: bool) -> impl IntoView {
    let routed = icons.routed;
    view! {
        {icon(icons.monitor, wide, "Screen", svg_monitor())}
        {icon(icons.plug, wide, "Plugins", svg_plug())}
        {icon(icons.wrench, wide, "Tools", svg_wrench())}
        {icon(icons.globe, wide, "Route traffic", move || if routed.get() { svg_route().into_any() } else { svg_globe("h-[18px] w-[18px]").into_any() })}
        {icon(icons.shield, wide, "Network rule", svg_shield())}
        {icons.share.map(|i| icon(i, wide, "Share as template", svg_share()))}
        {icons.gear.map(|g| icon(g, wide, "Settings", svg_gear()))}
    }
}

fn tooltip(tip: Option<Signal<Option<&'static str>>>, place: &'static str) -> impl IntoView {
    move || {
        tip.and_then(|t| t.get()).map(|text| {
            view! {
                <div class=format!("gc-card-in pointer-events-none absolute z-20 rounded-lg bg-gc-text px-3 py-2 text-[12px] leading-snug text-gc-bg shadow-xl {place}")>
                    {text}
                </div>
            }
        })
    }
}

/// The pane's thumbnail: the same screen, scaled to fit.
fn thumb(desktop: ViewFn, r: NodeRef<html::Div>, class: &'static str, open_label: bool) -> impl IntoView {
    let k = RwSignal::new(0.);
    let measure = move || {
        if let Some(el) = r.get_untracked() {
            let w = el.get_bounding_client_rect().width();
            if w > 0. {
                k.set(w / VW);
            }
        }
    };
    Effect::new(move |_| {
        if r.get().is_some() {
            request_animation_frame(measure);
        }
    });
    let resize = window_event_listener(ev::resize, move |_| measure());
    on_cleanup(move || resize.remove());
    view! {
        <div node_ref=r class=format!("group relative aspect-[16/10] shrink-0 overflow-hidden border border-gc-border {class}")>
            <div class="pointer-events-none absolute left-0 top-0 origin-top-left" style=move || format!("width:{VW}px;height:{VH}px;transform:scale({:.4})", k.get())>
                {desktop.run()}
            </div>
            {open_label.then(|| view! {
                <span class="absolute inset-0 flex items-center justify-center bg-black/20 opacity-0 transition-opacity group-hover:opacity-100">
                    <span class="inline-flex items-center gap-1.5 rounded-full bg-black/75 px-3 py-1.5 text-[13px] font-medium text-white shadow-lg">
                        <span class="material-symbols-outlined text-[16px]">"open_in_full"</span>
                        "Open"
                    </span>
                </span>
            })}
        </div>
    }
}

/// The NativeChat window. `children` is the chat thread; `desktop` is the coworker's screen,
/// drawn in the thumbnail and, grown, over the whole window; `overlay` sits on top of it all
/// (sheets, modals, the demo's pointer).
#[component]
pub fn AgentWindow(
    /// The window itself: the cursor's stage and the autoplay trigger.
    win: NodeRef<html::Div>,
    screen: AgentScreen,
    /// Whose chat this is. It can change mid-run, as when a demo cuts to another person's app.
    #[prop(into)]
    who: Signal<Coworker>,
    #[prop(into)] rail: Signal<&'static [Coworker]>,
    #[prop(optional)] icons: Option<PaneIcons>,
    /// A tooltip under the toolbar.
    #[prop(optional, into)]
    tip: Option<Signal<Option<&'static str>>>,
    #[prop(optional)] scroller: NodeRef<html::Div>,
    #[prop(optional)] composer: NodeRef<html::Div>,
    /// What's being typed into the composer.
    #[prop(optional, into)]
    draft: Option<Signal<&'static str>>,
    #[prop(into)] desktop: ViewFn,
    /// The one routine in the pane: name and when.
    #[prop(into, default = Signal::stored(("Weekly close", "Sundays")))]
    routine: Signal<(&'static str, &'static str)>,
    #[prop(optional, into)] overlay: ViewFnOnce,
    /// A sheet over the computer pane, such as Settings. Below `md`, where the pane is a strip,
    /// it slides over the right of the chat instead.
    #[prop(optional, into)]
    sheet: Option<ViewFn>,
    #[prop(optional, into)] sheet_open: Option<Signal<bool>>,
    children: Children,
) -> impl IntoView {
    let icons = icons.unwrap_or_default();
    let sheet_open = sheet_open.unwrap_or(Signal::stored(false));
    let draft = move || draft.map(|d| d.get()).unwrap_or_default();
    let zoom = screen.zoom;
    // Avatars that join the rail after the first build pop in.
    let mounted = StoredValue::new(false);
    Effect::new(move |_| mounted.set_value(true));

    view! {
        <div node_ref=win class="og-win relative z-[1] mx-auto flex h-[640px] w-full max-w-[1080px] select-none text-[13px] leading-snug">
            // Rail
            <div class="hidden w-16 shrink-0 flex-col items-center gap-3 border-r border-gc-border bg-gc-bg-elevated py-3 lg:flex">
                <div class="flex gap-1">
                    <span class="h-2.5 w-2.5 rounded-full bg-[#ff5f57]"></span>
                    <span class="h-2.5 w-2.5 rounded-full bg-[#febc2e]"></span>
                    <span class="h-2.5 w-2.5 rounded-full bg-[#28c840]"></span>
                </div>
                <For
                    each=move || rail.get().iter().copied()
                    key=|c: &Coworker| c.initials
                    children=move |c: Coworker| {
                        // Read once, so a later change of `who` doesn't pop every avatar.
                        let pop = if mounted.get_value() { " mac-pop" } else { "" };
                        view! {
                            <span class=move || format!(
                                "grid h-9 w-9 place-items-center rounded-full {} text-[11px] font-bold text-[#1a1a1a] {}{pop}",
                                c.color,
                                if c == who.get() { "ring-2 ring-gc-text/70 ring-offset-2 ring-offset-gc-bg-elevated" } else { "" }
                            )>{c.initials}</span>
                        }
                    }
                />
                <span class="mt-auto grid h-8 w-8 place-items-center rounded-full bg-gc-bg-muted text-[10px] font-bold text-gc-text-muted">"GC"</span>
            </div>

            // Chat
            <div class="relative flex min-w-0 flex-1 flex-col bg-gc-bg-card">
                <div class="flex h-12 shrink-0 items-center gap-2 border-b border-gc-border px-4">
                    <span class=move || format!("grid h-8 w-8 place-items-center rounded-full {} text-[10px] font-bold text-[#1a1a1a]", who.get().color)>{move || who.get().initials}</span>
                    <span class="font-semibold text-gc-text">{move || who.get().name}</span>
                    <span class="ml-auto grid h-8 w-8 place-items-center rounded-md bg-gc-bg-hover text-gc-text">
                        <span class="material-symbols-outlined text-[18px]">"desktop_windows"</span>
                    </span>
                </div>

                // The computer pane, folded into a strip at narrow widths.
                <div class="relative flex shrink-0 items-center gap-2 border-b border-gc-border bg-gc-bg-elevated px-3 py-2 md:hidden">
                    {thumb(desktop.clone(), screen.thumb_strip, "w-[68px] rounded-md", false)}
                    <span class="hidden min-w-0 truncate text-[11.5px] text-gc-text-muted min-[400px]:inline">{move || format!("{}'s screen", who.get().name)}</span>
                    <div class="ml-auto flex items-center gap-0.5">{toolbar(icons, false)}</div>
                    {tooltip(tip, "left-2 right-2 top-[calc(100%+4px)]")}
                </div>

                <div node_ref=scroller class="min-h-0 flex-1 overflow-y-auto">
                    <div class="flex flex-col gap-3 px-4 py-5 text-[14px] md:px-6">{children()}</div>
                </div>
                <div class="shrink-0 px-4 pb-4 pt-1">
                    <div
                        node_ref=composer
                        class=move || if draft().is_empty() {
                            "min-h-11 rounded-[22px] border border-gc-border bg-gc-bg-muted px-[18px] py-2.5 text-[14px] text-gc-text-faint"
                        } else {
                            "min-h-11 rounded-[22px] border border-gc-border bg-gc-bg-muted px-[18px] py-2.5 text-[14px] text-gc-text"
                        }
                    >
                        {move || match draft() {
                            "" => view! { {format!("Message {}", who.get().name)} }.into_any(),
                            t => view! { {t}<span class="gc-caret"></span> }.into_any(),
                        }}
                    </div>
                </div>
            </div>

            // Computer pane
            <aside class="relative hidden w-[272px] shrink-0 flex-col border-l border-gc-border bg-gc-bg-elevated md:flex lg:w-[300px]">
                <div class="relative flex h-12 shrink-0 items-center justify-between px-3">
                    {toolbar(icons, true)}
                    {tooltip(tip, "right-2 top-[calc(100%+2px)] w-[260px]")}
                </div>
                <div class="px-4 pt-2">
                    {thumb(desktop.clone(), screen.thumb_wide, "w-full rounded-xl", true)}
                    <div class="mt-2 text-center text-[12px] text-gc-text-muted">{move || format!("{}'s screen", who.get().name)}</div>
                </div>
                <div class="mt-6 px-4">
                    <div class="mb-3 text-[13px] text-gc-text-muted">"Routines"</div>
                    <div class="flex items-center gap-2 text-gc-text">
                        <span class="material-symbols-outlined text-[18px] text-gc-text-muted">"schedule"</span>
                        <span class="font-medium">{move || routine.get().0}</span>
                        <span class="text-[12px] text-gc-text-faint">{move || routine.get().1}</span>
                    </div>
                </div>
            </aside>

            {sheet.map(|sheet| view! {
                <Show when=move || sheet_open.get()>
                    <div class="gc-card-in absolute bottom-0 right-0 top-12 z-[15] flex w-[min(88%,300px)] flex-col border-l border-gc-border bg-gc-bg-elevated shadow-2xl md:top-0 md:w-[272px] md:shadow-none lg:w-[300px]">
                        {sheet.run()}
                    </div>
                </Show>
            })}

            // The screen, grown out of the pane to fill the window.
            <div class=move || if zoom.get() == Zoom::Open {
                "pointer-events-none absolute inset-0 z-20 bg-black/45 opacity-100 transition-opacity duration-300"
            } else {
                "pointer-events-none absolute inset-0 z-20 bg-black/45 opacity-0 transition-opacity duration-300"
            }></div>
            <div
                node_ref=screen.big
                class=move || if zoom.get() == Zoom::Closed {
                    "invisible absolute inset-1.5 z-30 origin-top-left overflow-hidden rounded-2xl md:inset-3"
                } else {
                    "absolute inset-1.5 z-30 origin-top-left overflow-hidden rounded-2xl shadow-[0_24px_80px_rgba(0,0,0,.55)] will-change-transform md:inset-3"
                }
                style=move || screen.style.get()
                aria-hidden=move || (zoom.get() == Zoom::Closed).to_string()
            >
                {desktop.run()}
            </div>

            {overlay.run()}
        </div>
    }
}

/// The Box's desktop: a top bar with `title` and a status slot, whatever runs on it, and a dock.
pub fn box_desktop(
    title: &'static str,
    status: impl IntoView + 'static,
    body: impl IntoView + 'static,
    dock: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        <div class="lx-desk relative h-full w-full select-none overflow-hidden text-left">
            <div class="absolute inset-x-0 top-0 z-10 flex h-8 items-center justify-between gap-2 bg-black/35 px-3 text-[11px] text-white/90">
                <span class="font-medium">"Activities"</span>
                <span class="truncate">{title}</span>
                <span class="flex items-center gap-2">
                    {status}
                    <span class="material-symbols-outlined text-[14px]">"wifi"</span>
                    <span class="material-symbols-outlined hidden text-[14px] sm:inline">"battery_full"</span>
                </span>
            </div>
            {body}
            <div class="absolute bottom-2.5 left-1/2 flex w-max max-w-[calc(100%-16px)] -translate-x-1/2 flex-wrap items-center justify-center gap-2 rounded-2xl border border-white/15 bg-black/40 px-2 py-1.5">
                {dock}
            </div>
        </div>
    }
}

/// The Box's usual dock: browser, terminal, files.
pub fn dock_apps() -> impl IntoView {
    ["public", "terminal", "folder"]
        .into_iter()
        .map(|i| view! { <span class="lx-dock-item material-symbols-outlined text-[20px] text-[#cbd5e1]">{i}</span> })
        .collect_view()
}

/// Chrome on the Box: a tab, the address bar, and the page. Chrome's own colors.
pub fn chrome(
    favicon: impl IntoView + 'static,
    tab: impl IntoView + 'static,
    host: &'static str,
    path: impl IntoView + 'static,
    page_class: &'static str,
    page_style: &'static str,
    page: impl IntoView + 'static,
) -> impl IntoView {
    view! {
        <div class="absolute bottom-[68px] left-1/2 top-11 flex w-[min(94%,780px)] -translate-x-1/2 flex-col overflow-hidden rounded-[10px] bg-white shadow-[0_18px_40px_rgba(0,0,0,.45)] ring-1 ring-black/20">
            <div class="flex h-8 shrink-0 items-end gap-1 bg-[#dee1e6] pl-2 pr-1">
                <span class="flex min-w-0 max-w-[230px] items-center gap-1.5 rounded-t-lg bg-white px-3 py-1.5 text-[11.5px] text-[#202124]">
                    {favicon}
                    <span class="truncate">{tab}</span>
                </span>
                <span class="mb-1.5 ml-1 text-[15px] leading-none text-[#5f6368]">"+"</span>
                <span class="mb-1 ml-auto flex items-center gap-2.5 px-1 text-[#5f6368]">
                    <span class="material-symbols-outlined text-[14px]">"remove"</span>
                    <span class="material-symbols-outlined text-[13px]">"crop_square"</span>
                    <span class="material-symbols-outlined text-[14px]">"close"</span>
                </span>
            </div>
            <div class="flex h-9 shrink-0 items-center gap-2 border-b border-[#e5e7eb] px-2.5 text-[#5f6368]">
                <span class="material-symbols-outlined text-[17px]">"arrow_back"</span>
                <span class="material-symbols-outlined text-[17px] opacity-40">"arrow_forward"</span>
                <span class="material-symbols-outlined text-[17px]">"refresh"</span>
                <span class="flex min-w-0 flex-1 items-center gap-1.5 rounded-full bg-[#f1f3f4] px-3 py-1 text-[12.5px]">
                    <span class="material-symbols-outlined text-[14px]">"tune"</span>
                    <span class="truncate"><span class="text-[#202124]">{host}</span>{path}</span>
                </span>
            </div>
            <div class=format!("relative min-h-0 flex-1 overflow-hidden {page_class}") style=page_style>
                {page}
            </div>
        </div>
    }
}
