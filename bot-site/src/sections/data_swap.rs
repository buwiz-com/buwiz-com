use leptos::{html, prelude::*};

use crate::components::demo_player::{DemoControls, Script, Timeline, autoplay_when_visible};

/// Coming soon: personal details swap into look-alikes on the way to the cloud and swap back on
/// the way home. One card, top to bottom: your device, the swap gate, the cloud. A message
/// travels down through the gate, each value gets a stand-in sticker, the AI answers with the
/// stand-ins, and the answer travels back up and peels the stickers off.
const CAPTIONS: [&str; 5] = [
    "You type real details, like you always would.",
    "On the way out, each personal detail swaps into a look-alike at the gate.",
    "The AI only ever gets stand-ins. It can still do the job with them.",
    "On the way back, the stand-ins swap back to the real thing.",
    "So you read real names and numbers. The cloud never saw them.",
];

/// Each value: the real one, and its stand-in.
const VALUES: [(&str, &str); 4] = [
    ("juan.delacruz@gmail.com", "maria.santos@example.net"),
    ("123-456-789-000", "900-111-222-000"),
    ("34-1234567-8", "00-0000001-9"),
    ("12 Rizal St, Makati", "7 Sample Ave, Pasig"),
];

#[derive(Clone, Copy)]
enum Seg {
    T(&'static str),
    V(usize),
}
use Seg::{T, V};

const ASK: &[Seg] = &[
    T("Email "),
    V(0),
    T(" his 2316. TIN "),
    V(1),
    T(", SSS "),
    V(2),
    T(". Mail it to "),
    V(3),
    T("."),
];
const REPLY: &[Seg] =
    &[T("Done. Emailed "), V(0), T(" with TIN "), V(1), T(", and the hard copy goes to "), V(3), T(".")];
const TYPED: &str =
    "Email juan.delacruz@gmail.com his 2316. TIN 123-456-789-000, SSS 34-1234567-8. Mail it to 12 Rizal St, Makati.";

/// Down to the gate, or from the gate onward.
const FLY: u32 = 1100;
/// Between one value's swap and the next.
const SWAP_GAP: u32 = 260;
/// After the last swap, before the bubble moves on.
const SETTLE: u32 = 350;

const BUBBLE: &str = "max-w-[94%] rounded-2xl px-3 py-[9px] text-[13.5px] leading-[1.65]";
/// Yours sit on the right, the AI's on the left.
const MINE: &str = "self-end";
const THEIRS: &str = "self-start";
const ME: &str = "ds-me rounded-br-[5px] bg-gc-text text-gc-bg";
const ME_CLOUD: &str = "ds-me ds-cme rounded-br-[5px] text-gc-bg";
const AI: &str = "rounded-bl-[5px] bg-gc-bg-muted text-gc-text";
const AI_CLOUD: &str = "rounded-bl-[5px] bg-gc-bg-card text-gc-text ring-1 ring-gc-border-hover";

const ZONE_HEAD: &str =
    "flex items-center gap-2 font-mono text-[12px] font-semibold uppercase tracking-[0.06em] text-gc-text-muted";
const PILL: &str = "ml-auto rounded-full border border-gc-border-hover px-[7px] py-px font-mono text-[10px] font-semibold";
const SIDE: &str =
    "absolute left-3.5 font-mono text-[10px] font-semibold uppercase tracking-[0.08em] text-gc-text-muted";

/// A bubble's place in a thread: not there yet, laid out but hidden while a copy flies to it,
/// or shown.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Empty,
    Held,
    Shown,
}

#[derive(Clone, Copy)]
struct Bubble {
    slot: RwSignal<Slot>,
    el: NodeRef<html::Div>,
}

impl Bubble {
    fn new() -> Self {
        Self { slot: RwSignal::new(Slot::Empty), el: NodeRef::new() }
    }
}

/// Which way the flying copy goes.
#[derive(Clone, Copy, PartialEq)]
enum Trip {
    /// Your message, out to the cloud.
    Out,
    /// The answer, back to you.
    Back,
}

#[derive(Clone)]
struct Spark {
    id: u32,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
}

#[derive(Clone, Copy)]
struct Demo {
    tl: Timeline,
    card: NodeRef<html::Div>,
    gate: NodeRef<html::Div>,
    ghost_el: NodeRef<html::Div>,
    you_ask: Bubble,
    you_reply: Bubble,
    cloud_ask: Bubble,
    cloud_reply: Bubble,
    /// The copy in flight, and where it is in the card. Hidden until first measured.
    ghost: RwSignal<Option<Trip>>,
    ghost_at: RwSignal<Option<(f64, f64)>>,
    /// Which of the flying copy's values wear their sticker.
    sw: [RwSignal<bool>; 4],
    hot: RwSignal<bool>,
    sparks: RwSignal<Vec<Spark>>,
    next_spark: StoredValue<u32>,
    step: RwSignal<Option<usize>>,
    draft: RwSignal<&'static str>,
}

fn reduced_motion() -> bool {
    window()
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

fn ease(u: f64) -> f64 {
    if u < 0.5 { 4. * u * u * u } else { 1. - (-2. * u + 2.).powi(3) / 2. }
}

impl Demo {
    fn reset(self) {
        self.step.set(Some(0));
        self.draft.set("");
        for b in [self.you_ask, self.you_reply, self.cloud_ask, self.cloud_reply] {
            b.slot.set(Slot::Empty);
        }
        self.ghost.set(None);
        self.ghost_at.set(None);
        self.hot.set(false);
        self.sparks.set(Vec::new());
    }

    /// The card's padding box, which the flying copy and the sparks are placed against.
    fn origin(self) -> Option<(f64, f64)> {
        let card = self.card.get_untracked()?;
        let r = card.get_bounding_client_rect();
        Some((r.left() + card.client_left() as f64, r.top() + card.client_top() as f64))
    }

    /// Moves the flying copy: from its bubble down (or up) to the gate, holds there while the
    /// values swap, then on to the slot waiting for it. Every frame re-measures both ends, so a
    /// resize mid-flight still lands true.
    fn fly(self, trip: Trip, hold: f64) {
        let (src, dest, right) = match trip {
            Trip::Out => (self.you_ask.el, self.cloud_ask.el, true),
            Trip::Back => (self.cloud_reply.el, self.you_reply.el, false),
        };
        let t0 = self.tl.now();
        let fly = FLY as f64;
        let reduce = reduced_motion();
        self.tl.every_frame(move |now| {
            if self.ghost.get_untracked() != Some(trip) {
                return false;
            }
            let (Some((ox, oy)), Some(gate), Some(ghost), Some(src), Some(dest)) = (
                self.origin(),
                self.gate.get_untracked(),
                self.ghost_el.get_untracked(),
                src.get_untracked(),
                dest.get_untracked(),
            ) else {
                return true;
            };
            let g = ghost.get_bounding_client_rect();
            let (w, h) = (g.width(), g.height());
            let at = |r: web_sys::DomRect| (if right { r.right() - ox - w } else { r.left() - ox }, r.top() - oy);
            let a = at(src.get_bounding_client_rect());
            let b = at(dest.get_bounding_client_rect());
            let gr = gate.get_bounding_client_rect();
            let mid = (a.0, gr.top() - oy + gr.height() / 2. - h / 2.);
            let t = now - t0;
            let (p, q, u) = if t < fly {
                (a, mid, t / fly)
            } else if t < fly + hold {
                (mid, mid, 1.)
            } else {
                (mid, b, ((t - fly - hold) / fly).min(1.))
            };
            let e = if reduce { 1. } else { ease(u) };
            self.ghost_at.set(Some((p.0 + (q.0 - p.0) * e, p.1 + (q.1 - p.1) * e)));
            t < 2. * fly + hold
        });
    }

    /// A little burst where value `k` just changed face.
    fn burst(self, k: usize) {
        let (Some((ox, oy)), Some(ghost)) = (self.origin(), self.ghost_el.get_untracked()) else { return };
        let Ok(Some(el)) = ghost.query_selector(&format!("[data-v=\"{k}\"]")) else { return };
        let r = el.get_bounding_client_rect();
        let (x, y) = (r.left() - ox + r.width() / 2., r.top() - oy + r.height() / 2.);
        let mut ids = Vec::new();
        self.sparks.update(|v| {
            for _ in 0..6 {
                let id = self.next_spark.try_update_value(|n| {
                    *n += 1;
                    *n
                });
                let Some(id) = id else { return };
                ids.push(id);
                v.push(Spark {
                    id,
                    x,
                    y,
                    dx: js_sys::Math::random() * 60. - 30.,
                    dy: js_sys::Math::random() * 40. - 20.,
                });
            }
        });
        self.tl.after(650, move || self.sparks.update(|v| v.retain(|s| !ids.contains(&s.id))));
    }

    /// Lays out one trip through the gate on the script.
    fn trip(self, s: &mut Script, trip: Trip) {
        let (dest, vals, stickers): (Bubble, &'static [usize], bool) = match trip {
            Trip::Out => (self.cloud_ask, &[0, 1, 2, 3], true),
            Trip::Back => (self.you_reply, &[0, 1, 3], false),
        };
        let hold = vals.len() as u32 * SWAP_GAP + SETTLE;
        s.then(move || {
            for k in self.sw {
                k.set(!stickers);
            }
            dest.slot.set(Slot::Held);
            self.ghost_at.set(None);
            self.ghost.set(Some(trip));
            self.fly(trip, hold as f64);
        })
        .wait(FLY)
        .then(move || self.hot.set(true));
        for &k in vals {
            s.then(move || {
                self.sw[k].set(stickers);
                self.burst(k);
            })
            .wait(SWAP_GAP);
        }
        s.wait(SETTLE).then(move || self.hot.set(false)).wait(FLY).then(move || {
            dest.slot.set(Slot::Shown);
            self.ghost.set(None);
            self.ghost_at.set(None);
        });
    }
}

/// One value: the real face and its sticker. Only the face that shows takes up room.
fn token(k: usize, on: Signal<bool>) -> impl IntoView {
    let (real, fake) = VALUES[k];
    view! {
        <span class=format!("ds-v ds-k{}", k + 1) class:ds-on=move || on.get() data-v=k.to_string()>
            <span class="ds-real" aria-hidden=move || on.get().then_some("true")>{real}</span>
            <span class="ds-fake" aria-hidden=move || (!on.get()).then_some("true")>{fake}</span>
        </span>
    }
}

/// The words and values of a message. Punctuation right after a value stays on its line.
fn text(segs: &'static [Seg], on: impl Fn(usize) -> Signal<bool>) -> impl IntoView {
    let mut out = Vec::new();
    let mut rest = segs;
    while let Some((seg, tail)) = rest.split_first() {
        rest = tail;
        match *seg {
            T(t) => out.push(t.into_any()),
            V(k) => match tail.first() {
                Some(&T(next)) if next.starts_with(['.', ',']) => {
                    let (mark, after) = next.split_at(1);
                    out.push(view! { <span class="whitespace-nowrap">{token(k, on(k))}{mark}</span> }.into_any());
                    out.push(after.into_any());
                    rest = &tail[1..];
                }
                _ => out.push(token(k, on(k)).into_any()),
            },
        }
    }
    out
}

/// A bubble in one of the threads, real or stand-in throughout.
fn bubble(b: Bubble, side: &'static str, look: &'static str, segs: &'static [Seg], stand_ins: bool) -> impl IntoView {
    move || {
        let slot = b.slot.get();
        (slot != Slot::Empty).then(move || {
            let shown = if slot == Slot::Held { "invisible" } else { "gc-card-in" };
            let class = format!("{BUBBLE} {side} {look} {shown}");
            view! {
                <div node_ref=b.el class=class>
                    {text(segs, move |_| Signal::stored(stand_ins))}
                </div>
            }
        })
    }
}

/// The copy in flight. Same look as the bubble it becomes in your thread.
fn ghost(d: Demo) -> impl IntoView {
    move || {
        d.ghost.get().map(|trip| {
            let (look, segs) = match trip {
                Trip::Out => (ME, ASK),
                Trip::Back => (AI, REPLY),
            };
            let sw = d.sw;
            view! {
                <div
                    node_ref=d.ghost_el
                    class=format!("pointer-events-none absolute left-0 top-0 z-10 rounded-2xl px-3 py-[9px] text-[13.5px] leading-[1.65] shadow-lg will-change-transform {look}")
                    style=move || match d.ghost_at.get() {
                        Some((x, y)) => format!("max-width:calc((100% - 28px) * 0.94);transform:translate({x:.1}px,{y:.1}px)"),
                        None => "max-width:calc((100% - 28px) * 0.94);visibility:hidden".to_string(),
                    }
                    aria-hidden="true"
                >
                    {text(segs, move |k| sw[k].into())}
                </div>
            }
        })
    }
}

#[component]
pub fn DataSwap() -> impl IntoView {
    let tl = Timeline::new();
    let d = Demo {
        tl,
        card: NodeRef::new(),
        gate: NodeRef::new(),
        ghost_el: NodeRef::new(),
        you_ask: Bubble::new(),
        you_reply: Bubble::new(),
        cloud_ask: Bubble::new(),
        cloud_reply: Bubble::new(),
        ghost: RwSignal::new(None),
        ghost_at: RwSignal::new(None),
        sw: std::array::from_fn(|_| RwSignal::new(false)),
        hot: RwSignal::new(false),
        sparks: RwSignal::new(Vec::new()),
        next_spark: StoredValue::new(0),
        step: RwSignal::new(None),
        draft: RwSignal::new(""),
    };

    tl.script(move |s| {
        d.reset();
        s.wait(500)
            .typing(TYPED, 18, move |t| d.draft.set(t))
            .wait(300)
            .then(move || {
                d.draft.set("");
                d.you_ask.slot.set(Slot::Shown);
            })
            .wait(700)
            .then(move || d.step.set(Some(1)));
        d.trip(s, Trip::Out);
        s.then(move || d.step.set(Some(2)))
            .wait(900)
            .then(move || d.cloud_reply.slot.set(Slot::Shown))
            .wait(1100)
            .then(move || d.step.set(Some(3)));
        d.trip(s, Trip::Back);
        s.then(move || d.step.set(Some(4)));
    });

    autoplay_when_visible(tl, d.card);

    let caption = move || d.step.get().map(|i| CAPTIONS[i]).unwrap_or("");

    view! {
        <section id="swap" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1440px] mx-auto px-6 md:px-12">
                <div class="grid gap-8 min-[900px]:grid-cols-[minmax(0,1fr)_440px] min-[900px]:items-start min-[900px]:gap-x-12 xl:grid-cols-[minmax(0,1fr)_480px] xl:gap-x-20">
                    <div class="grid max-w-[600px] gap-4 min-[900px]:sticky min-[900px]:top-24">
                        <div class="text-[11px] font-bold tracking-wide uppercase text-gc-gold">"Coming soon"</div>
                        <h2 class="text-3xl lg:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] text-balance">
                            "Real on your screen. Stand-ins in the cloud."
                        </h2>
                        <p class="text-lg text-gc-text-muted max-w-[52ch]">
                            "Your emails, TIN, SSS and address swap into look-alikes the moment they leave your device, and swap back the moment the answer returns. You never see a fake. The cloud never sees the real thing."
                        </p>
                        <a
                            href="https://github.com/hexuria/cred-swap"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="inline-flex w-fit items-center gap-1 text-[13px] font-medium text-gc-text-muted underline decoration-gc-border-strong underline-offset-4 hover:text-gc-text hover:decoration-gc-text"
                        >
                            "Built on cred-swap"
                            <span class="material-symbols-outlined text-[14px]">"open_in_new"</span>
                        </a>
                        <div class="mt-2 flex items-center gap-4">
                            <div class="flex flex-1 gap-[5px]" aria-hidden="true">
                                {(0..CAPTIONS.len()).map(|i| view! {
                                    <i class=move || if d.step.get().is_some_and(|s| i <= s) {
                                        "h-1 flex-1 rounded-full bg-gc-text transition-colors duration-300"
                                    } else {
                                        "h-1 flex-1 rounded-full bg-gc-text/15 transition-colors duration-300"
                                    }></i>
                                }).collect_view()}
                            </div>
                            <DemoControls timeline=tl />
                        </div>
                        <p class="min-h-11 text-[14px] font-medium text-gc-text" aria-live="polite">{caption}</p>
                    </div>

                    <div
                        node_ref=d.card
                        class="ds-card relative flex w-full max-w-[520px] select-none flex-col overflow-hidden rounded-3xl border border-gc-border-hover bg-gc-bg-card shadow-2xl min-[900px]:max-w-none"
                    >
                        <div class="flex flex-col gap-2 px-3.5 pt-3.5 pb-2.5">
                            <div class=ZONE_HEAD>
                                <span aria-hidden="true">"📱"</span>
                                "Your device"
                                <span class=PILL>"real"</span>
                            </div>
                            <div class="flex min-h-[190px] flex-1 flex-col justify-end gap-2">
                                {bubble(d.you_ask, MINE, ME, ASK, false)}
                                {bubble(d.you_reply, THEIRS, AI, REPLY, false)}
                            </div>
                            <div class=move || if d.draft.get().is_empty() {
                                "flex min-h-[42px] items-center rounded-full border border-gc-border-hover px-3.5 text-[14px] text-gc-text-faint"
                            } else {
                                "flex min-h-[42px] items-center rounded-full border border-gc-border-hover px-3.5 text-[14px] text-gc-text"
                            }>
                                // Shows the start while it fits, then the tail as it grows.
                                <div class="flex min-w-0 flex-1 justify-end overflow-hidden">
                                    <span class="min-w-full shrink-0 whitespace-nowrap">
                                        {move || match d.draft.get() {
                                            "" => view! { "Message Finance" }.into_any(),
                                            t => view! { {t}<span class="gc-caret"></span> }.into_any(),
                                        }}
                                    </span>
                                </div>
                            </div>
                        </div>

                        <div
                            node_ref=d.gate
                            class=move || if d.hot.get() {
                                "ds-gate ds-hot relative grid h-[92px] shrink-0 place-items-center"
                            } else {
                                "ds-gate relative grid h-[92px] shrink-0 place-items-center"
                            }
                        >
                            <span class=format!("{SIDE} top-2")>"↑ stays with you"</span>
                            <div class="ds-scan" aria-hidden="true"></div>
                            <div class="ds-core relative z-[2] flex items-center gap-2.5 rounded-full border border-gc-border-hover bg-gc-bg-card py-1.5 pl-1.5 pr-4 text-[13px] font-semibold text-gc-text">
                                <span class="ds-core-ic grid h-[26px] w-[26px] place-items-center rounded-full bg-gc-bg-muted text-[14px]" aria-hidden="true">"⇅"</span>
                                "Swap"
                            </div>
                            <span class=format!("{SIDE} bottom-2")>"↓ leaves your device"</span>
                        </div>

                        <div class="flex flex-col gap-2 bg-gc-brand/[0.07] px-3.5 pt-3 pb-3.5">
                            <div class=ZONE_HEAD>
                                <svg viewBox="0 0 34 22" class="h-[18px] w-7 text-gc-text-muted" aria-hidden="true">
                                    <path d="M9 21h17a7 7 0 0 0 0-14 9 9 0 0 0-17 2A6 6 0 0 0 9 21z" fill="none" stroke="currentColor" stroke-width="1.8" />
                                </svg>
                                "Cloud · AI model"
                                <span class=PILL>"stand-ins"</span>
                            </div>
                            <div class="flex min-h-[190px] flex-col gap-2">
                                {bubble(d.cloud_ask, MINE, ME_CLOUD, ASK, true)}
                                {bubble(d.cloud_reply, THEIRS, AI_CLOUD, REPLY, true)}
                            </div>
                        </div>

                        {ghost(d)}
                        <For
                            each=move || d.sparks.get()
                            key=|s| s.id
                            children=|s| view! {
                                <span
                                    class="ds-spark"
                                    style=format!("left:{:.1}px;top:{:.1}px;--dx:{:.1}px;--dy:{:.1}px", s.x, s.y, s.dx, s.dy)
                                ></span>
                            }
                        />
                    </div>
                </div>
            </div>
        </section>
    }
}
