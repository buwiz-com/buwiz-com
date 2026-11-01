//! The home hero: a self-playing film in the shared agent window. Three acts, one coworker, one
//! thread. In each, the cursor opens Settings in the computer pane, picks the model and effort on
//! the Model card, sends a message, and the computer screen grows to show the work and shrinks
//! back. The reply's badge says who paid: your plan or an API key.
//!
//! 1. GPT-6 Astra · Ultra · ⚡ Fast, on the plan, paints the Mona Lisa in Paint.
//! 2. Fable 5.1 · Max, on an API key, has Claude Code fix Maze Run's wall bug.
//! 3. Gemini 4 Argon · High, on an API key, boxes a receipt and books it in Buwiz.
//!
//! It plays at 2× by default and loops while it's on screen. Nothing in it takes clicks.

use leptos::{ev, html, prelude::*};

use crate::components::agent_window::{AgentScreen, AgentWindow, GENIE, GENIE_RAIL, PaneIcon, PaneIcons};
use crate::components::demo_player::{
    Aim, DemoCursor, DemoPointer, PlayState, ReplayButton, Script, SpeedChips, StepChips, Timeline, in_view,
};
use crate::components::hero_apps::{
    App, BUG_WALK, CLAUDE_LINES, FIELD_COUNT, PAINT_MS, Run, Screens, desktop, lisa_svg, receipt_svg,
};
use crate::components::model_card::{
    ARGON, ASTRA, FABLE, HIGH, Lit, MAX, ModelPick, Pop, ReplyBadge, SettingsSheet, ULTRA,
};

const ACTS: &[&str] = &["1 · Astra paints", "2 · Fable fixes the game", "3 · Argon books a receipt"];

const ASK_PAINT: &str = "Paint the Mona Lisa, pixel by pixel";
const ASK_MAZE: &str = "Players walk through walls in Maze Run. Fix it.";
const ASK_BOOK: &str = "Book this.";

#[derive(Clone)]
struct Msg {
    id: u32,
    body: Body,
}

#[derive(Clone, Copy)]
enum Body {
    Me(&'static str),
    /// The receipt photo, with "Book this."
    MeReceipt,
    Ge(&'static str),
    /// Act 1's reply: the picture, and its badge.
    Painted,
    Fixed,
    /// Act 3's reply: the journal entry as a card.
    Booked,
}

/// Act 3's photo, dragged into the chat.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Off,
    Held,
}

#[derive(Clone, Copy)]
struct Film {
    tl: Timeline,
    cur: DemoCursor,
    act: RwSignal<Option<usize>>,
    caption: RwSignal<&'static str>,
    msgs: RwSignal<Vec<Msg>>,
    next: StoredValue<u32>,
    draft: RwSignal<&'static str>,
    pick: ModelPick,
    settings: RwSignal<bool>,
    gear_lit: RwSignal<bool>,
    drag: RwSignal<Drag>,
    screens: Screens,
    screen: AgentScreen,
    icons: PaneIcons,
    win: NodeRef<html::Div>,
    scroller: NodeRef<html::Div>,
    composer: NodeRef<html::Div>,
    /// Where the photo comes in from: the window's left edge.
    drag_from: NodeRef<html::Div>,
}

impl Film {
    fn reset(self) {
        self.msgs.set(Vec::new());
        self.draft.set("");
        self.pick.reset();
        self.settings.set(false);
        self.gear_lit.set(false);
        self.drag.set(Drag::Off);
        self.screens.reset();
        self.screen.reset();
        self.cur.reset();
    }

    fn act(self, n: usize, caption: &'static str) {
        self.act.set(Some(n));
        self.caption.set(caption);
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
        // Twice: cards that slide in settle a frame later.
        let down = move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        };
        request_animation_frame(down);
        gloo_timers::callback::Timeout::new(320, down).forget();
    }
}

/// Rest on `aim` with `lit` showing, press, then `f`.
fn tap(s: &mut Script, f_: Film, aim: impl Into<Aim>, lit: Lit, f: impl FnOnce() + 'static) {
    let pick = f_.pick;
    s.move_to(f_.cur, aim)
        .then(move || pick.lit.set(lit))
        .wait(220)
        .press(f_.cur)
        .wait(170)
        .then(move || {
            pick.lit.set(Lit::None);
            f();
        })
        .wait(320);
}

/// Settings › Model: pick `model`, drag effort from `from` to `to`, set ⚡, back to the screen.
fn pick_model(s: &mut Script, f: Film, model: usize, from: usize, to: usize, fast: Option<bool>) {
    let p = f.pick;
    let (gear, lit) = (f.icons.gear.map(|g| g.aim()), f.gear_lit);
    if let Some(gear) = gear {
        s.move_to(f.cur, gear)
            .then(move || lit.set(true))
            .wait(220)
            .press(f.cur)
            .wait(170)
            .then(move || {
                lit.set(false);
                f.settings.set(true);
            })
            .wait(450);
    }
    tap(s, f, p.card, Lit::Card, move || p.open());
    s.wait(200);
    tap(s, f, p.name, Lit::Name, move || p.models());
    s.wait(200);
    tap(s, f, p.rows[model], Lit::Row(model), move || p.choose(model));
    s.wait(250);
    if from != to {
        // Grab the thumb and drag it; it snaps through each stop on the way.
        let steps: Vec<usize> = if to > from { (from + 1..=to).collect() } else { (to..from).rev().collect() };
        let per = 260;
        s.move_to(f.cur, p.stops[from]).wait(120).press(f.cur).wait(120);
        s.start_move(f.cur, p.stops[to], per * steps.len() as u32);
        for e in steps {
            s.wait(per).then(move || p.effort.set(e));
        }
        s.wait(150).press(f.cur).wait(350);
    }
    if let Some(on) = fast {
        tap(s, f, p.fast_btn, Lit::Fast, move || p.fast.set(on));
        s.wait(250);
    }
    tap(s, f, p.back, Lit::Back, move || {
        p.pop.set(Pop::Closed);
        f.settings.set(false);
    });
}

/// Type `text` into the composer and send it.
fn send(s: &mut Script, f: Film, text: &'static str, body: Body) {
    s.move_to(f.cur, f.composer).wait(150).typing(text, 34, move |t| f.draft.set(t)).wait(300).then(move || {
        f.draft.set("");
        f.push(body);
    });
}

fn act_paint(s: &mut Script, f: Film) {
    let sc = f.screens;
    s.then(move || f.act(0, "Settings › Model: GPT-6 Astra from your subscription, effort Ultra, ⚡ Fast.")).wait(500);
    pick_model(s, f, ASTRA, HIGH, ULTRA, Some(true));
    send(s, f, ASK_PAINT, Body::Me(ASK_PAINT));
    s.wait(500)
        .then(move || f.push(Body::Ge("On it. Opening Paint.")))
        .move_home(f.cur)
        .then(move || {
            sc.reset_paint();
            sc.app.set(App::Paint);
            f.screen.expand();
            f.caption.set("Genie's screen grows. Paint at 800%: the face goes in block by block.");
        })
        .wait(700)
        .then(move || sc.paint(f.tl))
        .wait(4600)
        .then(move || f.caption.set("Zooming out to 100%: the whole picture."))
        .wait(PAINT_MS - 4600 + 1600)
        .then(move || f.screen.shrink())
        .wait(600)
        .then(move || {
            f.push(Body::Painted);
            f.caption.set("Paid by your plan: Subscription ⚡. Hover the badge for the model and effort.");
        })
        .wait(2600);
}

fn act_maze(s: &mut Script, f: Film) {
    let sc = f.screens;
    s.then(move || f.act(1, "Next job, next model: Fable 5.1 through the Gateway, on an API key. Effort Max, ⚡ off."))
        .wait(400);
    pick_model(s, f, FABLE, ULTRA, MAX, Some(false));
    send(s, f, ASK_MAZE, Body::Me(ASK_MAZE));
    let step = 300.;
    s.wait(500)
        .then(move || f.push(Body::Ge("Reproducing it first.")))
        .move_home(f.cur)
        .then(move || {
            sc.maze(Run::Bug);
            sc.app.set(App::Maze);
            f.screen.expand();
            f.caption.set("The bug, live: the player walks right into a wall.");
        })
        .wait(800)
        .then(move || sc.walk(f.tl, BUG_WALK.to_vec(), 1, step))
        .wait((step * (BUG_WALK.len() - 1) as f64) as u32)
        .then(move || sc.crash.set(true))
        .wait(1700)
        .then(move || {
            sc.lines.set(0);
            sc.app.set(App::Code);
            f.caption.set("Claude Code finds it in src/player.rs: the move checks where you are, not where you go.");
        })
        .wait(500);
    for i in 1..=CLAUDE_LINES {
        let gap = match i {
            2 | 4 | 7 | 10 => 650,
            8 | 9 => 380,
            _ => 520,
        };
        s.then(move || sc.lines.set(i)).wait(gap);
    }
    let path = sc.solution();
    let corner = 4;
    let (head, tail) = (path[..=corner].to_vec(), path[corner..].to_vec());
    let (at, n_head, n_tail) = (path[corner], head.len(), tail.len());
    let step = 170.;
    s.wait(900)
        .then(move || {
            sc.maze(Run::Loading);
            sc.app.set(App::Maze);
            f.caption.set("Reload: walls hold now, and the player threads the maze to the exit.");
        })
        .wait(700)
        .then(move || sc.maze(Run::Fixed))
        .wait(400)
        .then(move || sc.walk(f.tl, head, 1, step))
        .wait((step * (n_head - 1) as f64) as u32)
        .then(move || sc.bump(f.tl, at, (1., 0.), 420.))
        .wait(480)
        .then(move || sc.walk(f.tl, tail, n_head, step))
        .wait((step * (n_tail - 1) as f64) as u32 + 80)
        .then(move || sc.solved.set(true))
        .wait(1700)
        .then(move || f.screen.shrink())
        .wait(600)
        .then(move || {
            f.push(Body::Fixed);
            f.caption.set("Paid with an API key: Gateway. Hover the badge: Fable 5.1 · Max.");
        })
        .wait(2600);
}

fn act_book(s: &mut Script, f: Film) {
    let sc = f.screens;
    s.then(move || f.act(2, "Reading a photo: Gemini 4 Argon through the Gateway, effort High.")).wait(400);
    pick_model(s, f, ARGON, MAX, HIGH, None);
    // Drag the receipt in from the window's edge and drop it on the composer.
    s.move_to(f.cur, f.drag_from)
        .press(f.cur)
        .then(move || {
            f.drag.set(Drag::Held);
            f.caption.set("Drop a receipt into the chat: “Book this.”");
        })
        .wait(250)
        .move_to(f.cur, f.composer)
        .press(f.cur)
        .then(move || f.drag.set(Drag::Off))
        .wait(200)
        .typing(ASK_BOOK, 40, move |t| f.draft.set(t))
        .wait(300)
        .then(move || {
            f.draft.set("");
            f.push(Body::MeReceipt);
        })
        .wait(500)
        .then(move || f.push(Body::Ge("Reading it.")))
        .move_home(f.cur)
        .then(move || {
            sc.boxes.set(0);
            sc.classified.set(false);
            sc.app.set(App::Vision);
            f.screen.expand();
            f.caption.set("Argon boxes every field, one after another: vendor, date, items, VAT, total.");
        })
        .wait(900);
    for i in 1..=FIELD_COUNT {
        s.then(move || sc.boxes.set(i)).wait(420);
    }
    s.wait(300)
        .then(move || {
            sc.classified.set(true);
            f.caption.set("Classified in 0.4s: a client meal, Representation, VAT split out.");
        })
        .wait(2300)
        .then(move || {
            sc.posted.set(0);
            sc.app.set(App::Ledger);
            f.caption.set("Then it posts the journal entry in Buwiz Books' Ledger: JE-0412.");
        })
        .wait(900);
    for i in 1..=3 {
        s.then(move || sc.posted.set(i)).wait(450);
    }
    s.wait(2200)
        .then(move || f.screen.shrink())
        .wait(600)
        .then(move || {
            f.push(Body::Booked);
            f.caption.set("Booked, on an API key. Three jobs, three models, one coworker.");
        })
        .wait(4200)
        .then(move || {
            if in_view(f.win) {
                f.tl.play();
            }
        });
}

const BUBBLE_ME: &str = "gc-card-in max-w-[85%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg";
const BUBBLE_GE: &str = "gc-card-in max-w-[85%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text";

fn reply(body: impl IntoView + 'static, badge: impl IntoView + 'static) -> impl IntoView {
    view! {
        <div class="gc-card-in flex max-w-[88%] flex-col items-start gap-1.5 self-start">
            {body}
            {badge}
        </div>
    }
}

fn message(m: Msg) -> AnyView {
    match m.body {
        Body::Me(t) => view! { <div class=BUBBLE_ME>{t}</div> }.into_any(),
        Body::MeReceipt => view! {
            <div class="gc-card-in flex max-w-[85%] flex-col items-end gap-1.5 self-end">
                <div class="overflow-hidden rounded-xl border border-gc-border bg-[#e9e6dc] p-1.5">
                    {receipt_svg("block h-[104px] w-auto", None)}
                </div>
                <div class="rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg">{ASK_BOOK}</div>
            </div>
        }
        .into_any(),
        Body::Ge(t) => view! { <div class=BUBBLE_GE>{t}</div> }.into_any(),
        Body::Painted => reply(
            view! {
                <div class="flex items-center gap-3 rounded-2xl bg-gc-bg-muted p-2 pr-4 text-gc-text">
                    {lisa_svg("block h-[72px] w-[54px] shrink-0 rounded-md")}
                    <span>"Done. Saved "<b>"mona-lisa.png"</b>", 24 × 32 blocks."</span>
                </div>
            },
            view! { <ReplyBadge model=ASTRA effort=ULTRA fast=true /> },
        )
        .into_any(),
        Body::Fixed => reply(
            view! {
                <div class="rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text">
                    "Fixed: collision now checks the next tile, not the current one. 9 tests pass and the maze plays clean."
                </div>
            },
            view! { <ReplyBadge model=FABLE effort=MAX fast=false /> },
        )
        .into_any(),
        Body::Booked => reply(
            view! {
                <div class="w-[min(340px,100%)] rounded-2xl border border-gc-border bg-gc-bg-card p-3.5 text-[13px]">
                    <div class="mb-2 flex items-center gap-2 font-semibold text-gc-text">
                        <span class="grid h-5 w-5 place-items-center rounded-md bg-[#22c55e] text-[11px] font-bold text-white">"B"</span>
                        "New ledger entry"
                    </div>
                    <div class="grid grid-cols-[auto_1fr_auto] gap-x-3 gap-y-1">
                        <span class="text-gc-text-muted">"Date"</span><span class="col-span-2 text-gc-text">"Oct 8, 2026"</span>
                        <span class="text-gc-text-muted">"Vendor"</span><span class="col-span-2 text-gc-text">"Jollibee Makati"</span>
                        <span class="text-gc-text-muted">"Debit"</span><span class="text-gc-text">"Representation"</span><span class="text-right tabular-nums text-gc-text">"₱1,152.61"</span>
                        <span class="text-gc-text-muted">"Debit"</span><span class="text-gc-text">"Input VAT"</span><span class="text-right tabular-nums text-gc-text">"₱138.39"</span>
                        <span class="text-gc-text-muted">"Credit"</span><span class="text-gc-text">"Cash"</span><span class="text-right tabular-nums text-gc-text">"₱1,291.00"</span>
                    </div>
                    <div class="mt-2.5 text-[12.5px] font-medium text-gc-accent">"✓ Posted to Buwiz Ledger · JE-0412"</div>
                </div>
            },
            view! { <ReplyBadge model=ARGON effort=HIGH fast=false /> },
        )
        .into_any(),
    }
}

/// The hero's film: act chips, speed, Replay, a caption, and the agent window.
#[component]
pub fn HeroFilm() -> impl IntoView {
    let win = NodeRef::<html::Div>::new();
    let tl = Timeline::new();
    tl.speed.set(2.);
    let gear_lit = RwSignal::new(false);
    let icons = PaneIcons { gear: Some(PaneIcon::new().lit(gear_lit)), ..PaneIcons::new() };
    let f = Film {
        tl,
        cur: DemoCursor::new(tl, win, (0.93, 0.95)),
        act: RwSignal::new(None),
        caption: RwSignal::new(""),
        msgs: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
        draft: RwSignal::new(""),
        pick: ModelPick::new(),
        settings: RwSignal::new(false),
        gear_lit,
        drag: RwSignal::new(Drag::Off),
        screens: Screens::new(),
        screen: AgentScreen::new(),
        icons,
        win,
        scroller: NodeRef::new(),
        composer: NodeRef::new(),
        drag_from: NodeRef::new(),
    };

    tl.script(move |s| {
        f.reset();
        s.wait(300);
        act_paint(s, f);
        act_maze(s, f);
        act_book(s, f);
    });

    // It's at the top of the page: play right away. After a loop that ended off screen, play
    // again when it scrolls back into view.
    Effect::new(move |_| {
        gloo_timers::callback::Timeout::new(250, move || {
            if tl.state.try_get_untracked() == Some(PlayState::Idle) {
                tl.play();
            }
        })
        .forget();
    });
    let scroll = window_event_listener(ev::scroll, move |_| {
        if tl.state.try_get_untracked() == Some(PlayState::Done) && in_view(win) {
            tl.play();
        }
    });
    on_cleanup(move || scroll.remove());

    let cur_at = f.cur.at();
    let pick = f.pick;

    view! {
        <div class="mx-auto w-full max-w-[1080px]">
            <div class="mb-4 flex flex-wrap items-center gap-2">
                <StepChips steps=ACTS current=f.act />
                <div class="ml-auto flex items-center gap-2">
                    <SpeedChips timeline=tl />
                    <ReplayButton timeline=tl />
                </div>
                <p class="min-h-5 basis-full pt-1 text-left text-[13px] text-gc-text-muted" aria-live="polite">{move || f.caption.get()}</p>
            </div>

            <AgentWindow
                win
                screen=f.screen
                who=GENIE
                rail=GENIE_RAIL
                icons=f.icons
                scroller=f.scroller
                composer=f.composer
                draft=f.draft
                routine=("Morning brief", "Weekdays")
                desktop=move || desktop(f.screens)
                sheet=move || view! { <SettingsSheet who=GENIE pick /> }
                sheet_open=f.settings
                overlay=move || view! {
                    <div node_ref=f.drag_from class="pointer-events-none absolute left-1 top-[42%] h-px w-px"></div>
                    <div class=move || if f.drag.get() == Drag::Held {
                        "pointer-events-none absolute inset-x-3 bottom-3 top-14 z-[16] rounded-2xl border-2 border-dashed border-gc-brand/60 bg-gc-brand/5 md:right-[284px] lg:right-[312px] lg:left-[76px]"
                    } else {
                        "hidden"
                    }></div>
                    {move || (f.drag.get() == Drag::Held).then(|| {
                        let (x, y) = cur_at.get().unwrap_or_default();
                        view! {
                            <div
                                class="pointer-events-none absolute left-0 top-0 z-[25] w-[64px] overflow-hidden rounded-md shadow-xl ring-1 ring-black/20"
                                style=format!("transform: translate({:.1}px, {:.1}px) rotate(-6deg)", x - 30., y - 20.)
                            >
                                {receipt_svg("block h-auto w-full", None)}
                            </div>
                        }
                    })}
                    <DemoPointer cursor=f.cur />
                }
            >
                <For each=move || f.msgs.get() key=|m| m.id children=message />
            </AgentWindow>
        </div>
    }
}
