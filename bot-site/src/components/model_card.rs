//! A mock of NativeChat's Model card, played by a demo's script: the model's name over its door
//! and effort, an effort popover (⚡, the effort's name, the model's name ›, ↺, and a slider that
//! snaps Low → Ultra), and a Models list grouped Current / Subscription / Gateway. It sits in the
//! Settings sheet over the agent window's computer pane. Replies carry a badge saying who paid,
//! your plan or an API key, and name the model and effort on hover.
//!
//! Nothing here takes clicks: the script opens, picks and drags, and the demo's cursor aims at the
//! elements this exposes.

use leptos::{html, prelude::*};

use crate::components::agent_window::Coworker;

#[derive(Clone, Copy, PartialEq)]
pub enum Door {
    Subscription,
    Gateway,
}

impl Door {
    pub fn label(self) -> &'static str {
        match self {
            Door::Subscription => "Subscription",
            Door::Gateway => "Gateway",
        }
    }
}

pub struct Model {
    pub name: &'static str,
    pub door: Door,
}

pub const MODELS: [Model; 6] = [
    Model { name: "GPT-6.1 Sol", door: Door::Subscription },
    Model { name: "GPT-6 Astra", door: Door::Subscription },
    Model { name: "GPT-6 Luna", door: Door::Subscription },
    Model { name: "Fable 5.1", door: Door::Gateway },
    Model { name: "Gemini 4 Argon", door: Door::Gateway },
    Model { name: "Grok 4.6", door: Door::Gateway },
];
pub const ASTRA: usize = 1;
pub const FABLE: usize = 3;
pub const ARGON: usize = 4;

pub const EFFORTS: [&str; 6] = ["Low", "Medium", "High", "XHigh", "Max", "Ultra"];
pub const HIGH: usize = 2;
pub const MAX: usize = 4;
pub const ULTRA: usize = 5;

#[derive(Clone, Copy, PartialEq)]
pub enum Pop {
    Closed,
    Effort,
    Models,
}

/// What the cursor rests on: it gets the hover look.
#[derive(Clone, Copy, PartialEq)]
pub enum Lit {
    None,
    Back,
    Card,
    Fast,
    Name,
    Row(usize),
}

/// The card's state and the elements the cursor aims at.
#[derive(Clone, Copy)]
pub struct ModelPick {
    pub model: RwSignal<usize>,
    pub effort: RwSignal<usize>,
    pub fast: RwSignal<bool>,
    pub pop: RwSignal<Pop>,
    pub lit: RwSignal<Lit>,
    /// The Settings sheet's back chevron.
    pub back: NodeRef<html::Div>,
    pub card: NodeRef<html::Div>,
    pub fast_btn: NodeRef<html::Div>,
    /// "Model ›" in the effort popover, which opens the Models list.
    pub name: NodeRef<html::Div>,
    pub rows: [NodeRef<html::Div>; 6],
    /// One mark per slider stop, on the track where the thumb's center lands.
    pub stops: [NodeRef<html::Div>; 6],
    root: NodeRef<html::Div>,
    place: RwSignal<String>,
}

impl ModelPick {
    pub fn new() -> Self {
        Self {
            model: RwSignal::new(0),
            effort: RwSignal::new(HIGH),
            fast: RwSignal::new(false),
            pop: RwSignal::new(Pop::Closed),
            lit: RwSignal::new(Lit::None),
            back: NodeRef::new(),
            card: NodeRef::new(),
            fast_btn: NodeRef::new(),
            name: NodeRef::new(),
            rows: std::array::from_fn(|_| NodeRef::new()),
            stops: std::array::from_fn(|_| NodeRef::new()),
            root: NodeRef::new(),
            place: RwSignal::new(String::new()),
        }
    }

    /// GPT-6.1 Sol at High, ⚡ off, popover closed.
    pub fn reset(self) {
        self.model.set(0);
        self.effort.set(HIGH);
        self.fast.set(false);
        self.pop.set(Pop::Closed);
        self.lit.set(Lit::None);
    }

    pub fn open(self) {
        self.measure();
        self.pop.set(Pop::Effort);
    }

    pub fn models(self) {
        self.measure();
        self.pop.set(Pop::Models);
    }

    /// Picks model `i` and goes back to the effort view.
    pub fn choose(self, i: usize) {
        self.model.set(i);
        self.pop.set(Pop::Effort);
    }

    /// The popover hangs off the card's positioned ancestor (the sheet), not the scrolling body,
    /// so the body can't clip it. It opens toward whichever side of the card has more room and
    /// never runs past the sheet's edge.
    fn measure(self) {
        let (Some(card), Some(root)) = (self.card.get_untracked(), self.root.get_untracked()) else { return };
        let Some(host) = card.offset_parent() else { return };
        let c = card.get_bounding_client_rect();
        // Opening upward clears the "Model" label too, so it stays readable over the popover.
        let label_top = root.get_bounding_client_rect().top();
        let h = host.get_bounding_client_rect();
        let top = h.top() + host.client_top() as f64;
        let bottom = top + host.client_height() as f64;
        let left = c.left() - h.left() - host.client_left() as f64;
        let (gap, edge) = (8.0, 8.0);
        let (above, below) = (label_top - top, bottom - c.bottom());
        let side = if below >= above {
            format!("top: {:.0}px; max-height: {:.0}px", c.bottom() - top + gap, below - gap - edge)
        } else {
            format!("bottom: {:.0}px; max-height: {:.0}px", bottom - label_top + gap, above - gap - edge)
        };
        self.place.set(format!("left: {left:.0}px; width: {:.0}px; {side}", c.width()));
    }
}

impl Default for ModelPick {
    fn default() -> Self {
        Self::new()
    }
}

/// NativeChat's blue-to-violet effort scale, hue 0.61 → 0.74.
fn effort_hue(stop: usize) -> f32 {
    (0.61 + 0.13 * stop as f32 / (EFFORTS.len() - 1) as f32) * 360.
}

/// The badge under a reply: who paid for it, and the model and effort on hover.
#[component]
pub fn ReplyBadge(model: usize, effort: usize, fast: bool) -> impl IntoView {
    let m = &MODELS[model];
    let tip = format!("{} · {}{}", m.name, EFFORTS[effort], if fast { " · ⚡ Fast" } else { "" });
    let (class, text) = match m.door {
        Door::Subscription => (
            "group relative inline-flex cursor-default items-center gap-1 rounded-md border border-gc-accent/45 px-2 py-0.5 text-[11.5px] font-medium text-gc-accent",
            if fast { "Subscription ⚡ · your plan" } else { "Subscription · your plan" },
        ),
        Door::Gateway => (
            "group relative inline-flex cursor-default items-center gap-1 rounded-md border border-gc-brand/45 px-2 py-0.5 text-[11.5px] font-medium text-gc-brand",
            "Gateway · API key",
        ),
    };
    let label = format!("{text}, {tip}");
    view! {
        <span class=class aria-label=label>
            {text}
            <span class="pointer-events-none absolute bottom-full left-0 z-10 mb-1.5 hidden whitespace-nowrap rounded-md bg-gc-text px-2 py-1 text-[11.5px] font-medium text-gc-bg shadow-lg group-hover:block">
                {tip}
            </span>
        </span>
    }
}

#[component]
pub fn ModelCard(pick: ModelPick) -> impl IntoView {
    let ModelPick { model, effort, fast, pop, lit, .. } = pick;

    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if pop.get_untracked() != Pop::Closed {
            pick.measure();
        }
    });
    on_cleanup(move || resize.remove());

    let detail = move || {
        let mut parts = vec![MODELS[model.get()].door.label(), EFFORTS[effort.get()]];
        if fast.get() {
            parts.push("⚡ Fast");
        }
        parts.join(" · ")
    };

    let row = move |i: usize, m: &'static Model| {
        view! {
            <div
                node_ref=pick.rows[i]
                class=move || if lit.get() == Lit::Row(i) {
                    "flex items-center justify-between rounded-md bg-gc-bg-hover px-2 py-1.5 text-[13px] text-gc-text"
                } else {
                    "flex items-center justify-between rounded-md px-2 py-1.5 text-[13px] text-gc-text"
                }
            >
                <span class="truncate">{m.name}</span>
                {move || (model.get() == i).then(|| view! { <span class="material-symbols-outlined text-[15px] text-gc-text-muted">"check"</span> })}
            </div>
        }
    };
    let group = move |door: Door| {
        view! {
            <div class="px-2 pb-0.5 pt-2 text-[11px] text-gc-text-faint">{door.label()}</div>
            {MODELS.iter().enumerate().filter(|(_, m)| m.door == door).map(|(i, m)| row(i, m)).collect_view()}
        }
    };

    let last = (EFFORTS.len() - 1) as f32;
    let pct = move || effort.get() as f32 / last;

    let effort_view = move || view! {
        <div class="p-3">
            <div class="grid grid-cols-[auto_1fr_auto] items-center gap-2">
                <div
                    node_ref=pick.fast_btn
                    class=move || {
                        let ring = if lit.get() == Lit::Fast { "ring-2 ring-gc-brand/50" } else { "" };
                        if fast.get() {
                            format!("grid h-8 w-8 place-items-center rounded-md border border-gc-gold/50 bg-gc-gold/10 text-gc-gold {ring}")
                        } else {
                            format!("grid h-8 w-8 place-items-center rounded-md border border-gc-border text-gc-text-faint opacity-60 grayscale {ring}")
                        }
                    }
                >
                    "⚡"
                </div>
                <div class="flex min-w-0 flex-col items-center">
                    <span
                        class="text-[13px] font-semibold"
                        style=move || format!("color: hsl({:.0} 75% 62%)", effort_hue(effort.get()))
                    >
                        {move || EFFORTS[effort.get()]}
                    </span>
                    <div
                        node_ref=pick.name
                        class=move || if lit.get() == Lit::Name {
                            "max-w-full truncate rounded px-1 text-[13px] text-gc-text underline"
                        } else {
                            "max-w-full truncate rounded px-1 text-[13px] text-gc-text"
                        }
                    >
                        {move || MODELS[model.get()].name}" ›"
                    </div>
                </div>
                <div class="grid h-8 w-8 place-items-center rounded-md text-gc-text-muted">"↺"</div>
            </div>
            // Track, fill and thumb. The fill is a pill ending at the thumb's center; it glows at
            // Ultra.
            <div
                class="relative mt-3 h-7"
                style=move || {
                    let glow = if effort.get() == ULTRA { "0 0 14px hsl(266 75% 60% / .7)" } else { "0 0 0 transparent" };
                    format!("--end: hsl({:.0} 75% 58%); --glow: {glow}", effort_hue(effort.get()))
                }
                role="slider"
                aria-label="Effort"
                aria-valuemin="0"
                aria-valuemax=(EFFORTS.len() - 1).to_string()
                aria-valuenow=move || effort.get().to_string()
                aria-valuetext=move || EFFORTS[effort.get()]
            >
                <div class="og-effort-track absolute inset-x-0 top-0.5 h-6 rounded-full bg-gc-bg-muted">
                    <div
                        class="og-effort-fill absolute inset-y-0 left-0 rounded-full transition-[width] duration-150 ease-out"
                        // At Low the thumb would leave a sliver of fill peeking past its round edge.
                        style=move || if effort.get() == 0 {
                            "width: 0px".to_string()
                        } else {
                            format!("width: calc(14px + {:.4} * (100% - 28px))", pct())
                        }
                    ></div>
                </div>
                <div
                    class="absolute top-0 h-7 w-7 rounded-full bg-white shadow-[0_1px_4px_rgb(0_0_0/0.35)] transition-[left] duration-150 ease-out"
                    style=move || format!("left: calc({:.4} * (100% - 28px))", pct())
                ></div>
                {(0..EFFORTS.len()).map(|i| view! {
                    <div
                        node_ref=pick.stops[i]
                        class="pointer-events-none absolute top-0 h-7 w-px"
                        style=format!("left: calc(14px + {:.4} * (100% - 28px))", i as f32 / last)
                    ></div>
                }).collect_view()}
            </div>
            // Each name sits centered under its stop, where the thumb's center lands.
            <div class="relative mt-1 h-4 text-[10px] text-gc-text-faint">
                {EFFORTS.iter().enumerate().map(|(i, e)| view! {
                    <span
                        class="absolute top-0 -translate-x-1/2 whitespace-nowrap"
                        style=format!("left: calc(14px + {:.4} * (100% - 28px))", i as f32 / last)
                    >
                        {*e}
                    </span>
                }).collect_view()}
            </div>
        </div>
    };

    let models_view = move || view! {
        <div class="flex min-h-0 flex-col p-2">
            <div class="flex shrink-0 items-center gap-1 px-1 py-1 text-[13px] font-semibold text-gc-text">
                <span class="material-symbols-outlined text-[16px]">"chevron_left"</span>
                "Models"
            </div>
            <div class="shrink-0 px-2 pb-0.5 pt-1.5 text-[11px] text-gc-text-faint">"Current"</div>
            <div class="flex shrink-0 items-center justify-between rounded-md bg-gc-bg-muted px-2 py-1.5 text-[13px] text-gc-text">
                <span class="truncate">{move || MODELS[model.get()].name}</span>
                <span class="text-[11px] text-gc-text-muted">{move || MODELS[model.get()].door.label()}</span>
            </div>
            <div class="min-h-0 overflow-hidden">
                {group(Door::Subscription)}
                {group(Door::Gateway)}
            </div>
        </div>
    };

    view! {
        <div node_ref=pick.root class="mb-4">
            <div class="mb-1.5 text-[11px] text-gc-text-muted">"Model"</div>
            <div
                node_ref=pick.card
                class=move || if pop.get() != Pop::Closed || lit.get() == Lit::Card {
                    "flex w-full items-center justify-between gap-3 rounded-xl border border-gc-brand/60 bg-gc-bg-muted px-3 py-3 text-left ring-2 ring-gc-brand/15 transition-colors"
                } else {
                    "flex w-full items-center justify-between gap-3 rounded-xl border border-gc-border bg-gc-bg-muted px-3 py-3 text-left transition-colors"
                }
            >
                <span class="min-w-0">
                    <span class="block truncate text-[13px] text-gc-text">{move || MODELS[model.get()].name}</span>
                    <span class="mt-0.5 block text-[11px] text-gc-text-muted">{detail}</span>
                </span>
                <span class="material-symbols-outlined text-[18px] text-gc-text-muted">"expand_more"</span>
            </div>

            <Show when=move || pop.get() != Pop::Closed>
                <div
                    role="dialog"
                    aria-label="Model"
                    class="gc-card-in absolute z-50 flex flex-col overflow-hidden rounded-xl border border-gc-border bg-gc-bg-elevated shadow-2xl"
                    style=move || pick.place.get()
                >
                    {move || match pop.get() {
                        Pop::Models => models_view().into_any(),
                        _ => effort_view().into_any(),
                    }}
                </div>
            </Show>
        </div>
    }
}

/// The coworker's Settings, as a sheet over the computer pane: back, the avatar, its name, and
/// the Model card.
#[component]
pub fn SettingsSheet(who: Coworker, pick: ModelPick) -> impl IntoView {
    let lit = pick.lit;
    view! {
        <div class="flex h-12 shrink-0 items-center gap-1 px-2">
            <div
                node_ref=pick.back
                class=move || if lit.get() == Lit::Back {
                    "grid h-8 w-8 place-items-center rounded-md bg-gc-bg-hover text-gc-text"
                } else {
                    "grid h-8 w-8 place-items-center rounded-md text-gc-text-muted"
                }
            >
                <span class="material-symbols-outlined">"chevron_left"</span>
            </div>
            <span class="flex-1 text-center font-semibold text-gc-text">"Settings"</span>
            <span class="grid h-8 w-8 place-items-center text-gc-text-muted">
                <span class="material-symbols-outlined">"close"</span>
            </span>
        </div>
        // Not positioned: the Model popover anchors to the sheet, past this scroll box.
        <div class="min-h-0 flex-1 overflow-y-auto px-4 pb-6">
            <div class="hidden justify-center pb-4 pt-1 md:flex">
                <span class=format!("grid h-14 w-14 place-items-center rounded-full {} text-[17px] font-bold text-[#1a1a1a]", who.color)>
                    {who.initials}
                </span>
            </div>
            <div class="mb-1.5 text-[11px] text-gc-text-muted">"Name"</div>
            <div class="mb-4 flex h-10 items-center rounded-xl border border-gc-border bg-gc-bg-muted px-3 text-gc-text">{who.name}</div>
            <ModelCard pick />
        </div>
    }
}
