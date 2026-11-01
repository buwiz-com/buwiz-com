//! Shared parts for the self-playing product demos: a pausable timeline, a cursor that moves
//! like a hand (curved path, ease in and out, press ring), the step chips with Play/Pause and
//! Replay, and a helper that starts a demo the first time it scrolls into view.
//!
//! The timeline runs on its own clock, advanced by animation frames. Pausing stops the clock, so
//! scheduled steps and the cursor's glide both hold where they are; `speed` runs it faster or
//! slower. Every play bumps a run number; anything left over from an older run does nothing.

use std::rc::Rc;

use leptos::{ev, html, prelude::*};

type Job = Box<dyn FnOnce()>;
type Frame = Box<dyn FnMut(f64) -> bool>;

/// What a demo runs on each play (see [`Timeline::script`]).
type ScriptFn = Rc<dyn Fn(&mut Script)>;

#[derive(Default)]
struct Clock {
    now: f64,
    last: f64,
    jobs: Vec<(f64, Job)>,
    frames: Vec<Frame>,
    script: Option<ScriptFn>,
    /// The frame loop has stopped: the run ended or was stopped.
    stopped: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayState {
    Idle,
    Playing,
    Paused,
    Done,
}

fn perf_now() -> f64 {
    window().performance().map(|p| p.now()).unwrap_or_else(js_sys::Date::now)
}

/// A demo's timeline. Give it a script, then `play()` runs the script from the top.
#[derive(Clone, Copy)]
pub struct Timeline {
    run: StoredValue<u32>,
    clock: StoredValue<Clock, LocalStorage>,
    pub state: RwSignal<PlayState>,
    /// How fast the demo clock runs against real time: 1.0 as written, 2.0 twice as fast.
    /// Steps, waits and the cursor's glides all follow it; CSS transitions don't.
    pub speed: RwSignal<f64>,
}

impl Timeline {
    pub fn new() -> Self {
        let tl = Self {
            run: StoredValue::new(0),
            clock: StoredValue::new_local(Clock::default()),
            state: RwSignal::new(PlayState::Idle),
            speed: RwSignal::new(1.),
        };
        let run = tl.run;
        on_cleanup(move || {
            run.try_update_value(|r| *r += 1);
        });
        tl
    }

    /// The script runs on every play: reset the demo's state first, then lay out its steps.
    pub fn script(self, f: impl Fn(&mut Script) + 'static) {
        self.clock.update_value(|c| c.script = Some(Rc::new(f)));
    }

    pub fn play(self) {
        let Some(run) = self.run.try_update_value(|r| {
            *r += 1;
            *r
        }) else {
            return;
        };
        let script = self.clock.try_update_value(|c| {
            c.now = 0.;
            c.last = perf_now();
            c.jobs.clear();
            c.frames.clear();
            c.stopped = false;
            c.script.clone()
        });
        self.state.set(PlayState::Playing);
        if let Some(Some(script)) = script {
            script(&mut Script { tl: self, t: 0. });
        }
        self.tick(run);
    }

    pub fn toggle_pause(self) {
        self.state.update(|s| {
            *s = match *s {
                PlayState::Playing => PlayState::Paused,
                PlayState::Paused => PlayState::Playing,
                other => other,
            }
        });
    }

    /// Ends the run where it stands, so a visitor can take over. Steps still waiting are
    /// dropped; anything scheduled afterwards runs on the clock as usual.
    pub fn stop(self) {
        if self.run.try_update_value(|r| *r += 1).is_none() {
            return;
        }
        self.clock.try_update_value(|c| {
            c.jobs.clear();
            c.frames.clear();
            c.stopped = true;
        });
        self.state.set(PlayState::Done);
    }

    /// Restarts the frame loop for work scheduled after the run ended.
    fn wake(self) {
        let woke = self.clock.try_update_value(|c| {
            let woke = c.stopped && (!c.jobs.is_empty() || !c.frames.is_empty());
            if woke {
                c.stopped = false;
                c.last = perf_now();
            }
            woke
        });
        if woke == Some(true)
            && let Some(run) = self.run.try_get_value()
        {
            self.tick(run);
        }
    }

    /// Time on the demo's clock, in ms since this run started.
    pub fn now(self) -> f64 {
        self.clock.try_with_value(|c| c.now).unwrap_or(0.)
    }

    fn at(self, ms: f64, job: Job) {
        self.clock.try_update_value(|c| {
            let i = c.jobs.partition_point(|(t, _)| *t <= ms);
            c.jobs.insert(i, (ms, job));
        });
        self.wake();
    }

    /// Runs `f` after `ms` of demo time from now.
    pub fn after(self, ms: u32, f: impl FnOnce() + 'static) {
        self.at(self.now() + ms as f64, Box::new(f));
    }

    /// Calls `f` with the demo clock on every frame until it returns false.
    pub fn every_frame(self, f: impl FnMut(f64) -> bool + 'static) {
        self.clock.try_update_value(|c| c.frames.push(Box::new(f)));
        self.wake();
    }

    fn current(self, run: u32) -> bool {
        self.run.try_get_value() == Some(run)
    }

    fn tick(self, run: u32) {
        request_animation_frame(move || {
            if !self.current(run) {
                return;
            }
            let t = perf_now();
            let paused = self.state.get_untracked() == PlayState::Paused;
            let speed = self.speed.get_untracked();
            let Some((now, due)) = self.clock.try_update_value(|c| {
                // A hidden tab stops frames; don't leap ahead when it comes back.
                let dt = (t - c.last).clamp(0., 100.);
                c.last = t;
                if !paused {
                    c.now += dt * speed;
                }
                let n = c.jobs.partition_point(|(at, _)| *at <= c.now);
                (c.now, c.jobs.drain(..n).map(|(_, j)| j).collect::<Vec<_>>())
            }) else {
                return;
            };
            for job in due {
                if !self.current(run) {
                    return;
                }
                job();
            }
            if !self.current(run) {
                return;
            }
            let mut frames = self.clock.try_update_value(|c| std::mem::take(&mut c.frames)).unwrap_or_default();
            frames.retain_mut(|f| f(now));
            let busy = self.clock.try_update_value(|c| {
                frames.append(&mut c.frames);
                c.frames = frames;
                let busy = !c.jobs.is_empty() || !c.frames.is_empty();
                c.stopped = !busy;
                busy
            });
            match busy {
                Some(true) => self.tick(run),
                Some(false) => self.state.set(PlayState::Done),
                None => {}
            }
        });
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Lays out a run's steps on the timeline. `t` is where the next step lands.
pub struct Script {
    tl: Timeline,
    t: f64,
}

/// How long the cursor gets to reach a target, unless a move says otherwise.
const GLIDE: u32 = 750;

impl Script {
    /// Jump to an absolute time in the run.
    pub fn go(&mut self, ms: u32) -> &mut Self {
        self.t = ms as f64;
        self
    }

    pub fn wait(&mut self, ms: u32) -> &mut Self {
        self.t += ms as f64;
        self
    }

    /// Runs `f` at the current time.
    pub fn then(&mut self, f: impl FnOnce() + 'static) -> &mut Self {
        self.tl.at(self.t, Box::new(f));
        self
    }

    /// Types `text` a character at a time, `per_char` ms apart, handing `f` what's typed so far.
    pub fn typing(
        &mut self,
        text: &'static str,
        per_char: u32,
        f: impl Fn(&'static str) + Clone + 'static,
    ) -> &mut Self {
        for (i, c) in text.char_indices() {
            let f = f.clone();
            let end = i + c.len_utf8();
            self.wait(per_char).then(move || f(&text[..end]));
        }
        self
    }

    /// Glides the cursor to `aim`, measured when the move starts. Takes `GLIDE` ms.
    pub fn move_to(&mut self, cur: DemoCursor, aim: impl Into<Aim>) -> &mut Self {
        let aim = aim.into();
        self.then(move || cur.glide(Goal::Aim(aim), GLIDE));
        self.wait(GLIDE)
    }

    /// Glides the cursor to `aim` in `ms`, for quick taps and drags.
    pub fn move_in(&mut self, cur: DemoCursor, aim: impl Into<Aim>, ms: u32) -> &mut Self {
        self.start_move(cur, aim, ms).wait(ms)
    }

    /// Starts a glide to `aim` that takes `ms`, without waiting for it: two cursors can move
    /// together.
    pub fn start_move(&mut self, cur: DemoCursor, aim: impl Into<Aim>, ms: u32) -> &mut Self {
        let aim = aim.into();
        self.then(move || cur.glide(Goal::Aim(aim), ms))
    }

    /// Glides the cursor back to where it rests.
    pub fn move_home(&mut self, cur: DemoCursor) -> &mut Self {
        self.then(move || cur.glide(Goal::Home, GLIDE));
        self.wait(GLIDE)
    }

    /// Shows the press ring. Doesn't move the script's time.
    pub fn press(&mut self, cur: DemoCursor) -> &mut Self {
        self.then(move || cur.press())
    }

    /// Move, settle, press, then `f` once the press lands.
    pub fn click(&mut self, cur: DemoCursor, aim: impl Into<Aim>, f: impl FnOnce() + 'static) -> &mut Self {
        self.move_to(cur, aim).wait(120).press(cur).wait(170).then(f).wait(380)
    }
}

/// What the cursor points at: the first of its elements that is on screen. Two elements let a
/// step aim at whichever copy of a button the current layout shows.
#[derive(Clone, Copy)]
pub struct Aim(NodeRef<html::Div>, Option<NodeRef<html::Div>>);

impl Aim {
    pub fn either(a: NodeRef<html::Div>, b: NodeRef<html::Div>) -> Self {
        Self(a, Some(b))
    }

    fn rect(self) -> Option<web_sys::DomRect> {
        [Some(self.0), self.1].into_iter().flatten().find_map(|r| {
            let rect = r.get_untracked()?.get_bounding_client_rect();
            (rect.width() > 0. && rect.height() > 0.).then_some(rect)
        })
    }
}

impl From<NodeRef<html::Div>> for Aim {
    fn from(r: NodeRef<html::Div>) -> Self {
        Self(r, None)
    }
}

#[derive(Clone, Copy)]
enum Goal {
    Home,
    Aim(Aim),
}

/// The demo's pointer. Lives inside a positioned stage; positions are measured against it.
#[derive(Clone, Copy)]
pub struct DemoCursor {
    tl: Timeline,
    stage: NodeRef<html::Div>,
    /// Resting spot, as a fraction of the stage.
    home: (f64, f64),
    /// None while resting at home, drawn in percentages so it survives resizes.
    pos: RwSignal<Option<(f64, f64)>>,
    down: RwSignal<bool>,
    rest: StoredValue<Option<Aim>>,
    glide_id: StoredValue<u32>,
}

impl DemoCursor {
    pub fn new(tl: Timeline, stage: NodeRef<html::Div>, home: (f64, f64)) -> Self {
        let cur = Self {
            tl,
            stage,
            home,
            pos: RwSignal::new(None),
            down: RwSignal::new(false),
            rest: StoredValue::new(None),
            glide_id: StoredValue::new(0),
        };
        // Layout changes move the targets; keep the cursor on the one it rests on.
        let resize = window_event_listener(ev::resize, move |_| {
            if let Some(Some(aim)) = cur.rest.try_get_value()
                && let Some(p) = cur.point(Goal::Aim(aim))
            {
                cur.pos.set(Some(p));
            }
        });
        on_cleanup(move || resize.remove());
        cur
    }

    /// Back to the resting spot, at once.
    pub fn reset(self) {
        self.glide_id.update_value(|g| *g += 1);
        self.rest.set_value(None);
        self.pos.set(None);
        self.down.set(false);
    }

    /// Where the pointer is, in px from the stage's corner. None while it rests at home.
    pub fn at(self) -> Signal<Option<(f64, f64)>> {
        self.pos.into()
    }

    pub fn press(self) {
        self.down.set(true);
        self.tl.after(170, move || self.down.set(false));
    }

    fn point(self, goal: Goal) -> Option<(f64, f64)> {
        let a = self.stage.get_untracked()?.get_bounding_client_rect();
        match goal {
            Goal::Home => Some((a.width() * self.home.0, a.height() * self.home.1)),
            Goal::Aim(aim) => {
                let r = aim.rect()?;
                Some((r.left() - a.left() + r.width() * 0.5, r.top() - a.top() + r.height() * 0.55))
            }
        }
    }

    /// A curved, eased glide. The target is re-measured every frame, so it lands true even if
    /// the layout shifts mid-move.
    fn glide(self, goal: Goal, budget: u32) {
        let Some(start) = self.pos.get_untracked().or_else(|| self.point(Goal::Home)) else { return };
        let Some(end) = self.point(goal) else { return };
        let id = self.glide_id.try_update_value(|g| {
            *g += 1;
            *g
        });
        self.rest.set_value(match goal {
            Goal::Aim(a) => Some(a),
            Goal::Home => None,
        });
        let (sx, sy) = start;
        let (dx, dy) = (end.0 - sx, end.1 - sy);
        let dist = dx.hypot(dy);
        let bend = (dist * 0.18).min(80.) * if dx > 0. { 1. } else { -1. };
        let ctrl = (sx + dx * 0.5 - dy * 0.15 + bend * 0.2, sy + dy * 0.5 + dx * 0.12 - bend.abs() * 0.3);
        let dur = (280. + dist * 1.1).min(budget as f64).max(380f64.min(budget as f64));
        let t0 = self.tl.now();
        let mut last_end = end;
        self.tl.every_frame(move |now| {
            if self.glide_id.try_get_value() != id {
                return false;
            }
            if let Some(p) = self.point(goal) {
                last_end = p;
            }
            let u = ((now - t0) / dur).clamp(0., 1.);
            let e = if u < 0.5 { 4. * u * u * u } else { 1. - (-2. * u + 2.).powi(3) / 2. };
            let (a, b, c) = ((1. - e) * (1. - e), 2. * (1. - e) * e, e * e);
            self.pos.set(Some((a * sx + b * ctrl.0 + c * last_end.0, a * sy + b * ctrl.1 + c * last_end.1)));
            if u < 1. {
                return true;
            }
            if matches!(goal, Goal::Home) {
                self.pos.set(None);
            }
            false
        });
    }
}

/// Draws a `DemoCursor`. Put it last inside the stage. `tag` labels whose pointer it is.
#[component]
pub fn DemoPointer(cursor: DemoCursor, #[prop(optional)] tag: Option<&'static str>) -> impl IntoView {
    let DemoCursor { pos, down, home, .. } = cursor;
    view! {
        <div
            class="pointer-events-none absolute left-0 top-0 z-30 will-change-transform"
            style=move || match pos.get() {
                Some((x, y)) => format!("transform: translate({x:.1}px, {y:.1}px);"),
                None => format!("left: {:.1}%; top: {:.1}%;", home.0 * 100., home.1 * 100.),
            }
            aria-hidden="true"
        >
            <span class=move || if down.get() {
                "absolute -left-3 -top-3 w-6 h-6 rounded-full bg-gc-brand/40 scale-100 transition-transform duration-150"
            } else {
                "absolute -left-3 -top-3 w-6 h-6 rounded-full bg-gc-brand/40 scale-0 transition-transform duration-150"
            }></span>
            <svg viewBox="0 0 16 24" class="relative w-4 h-6 drop-shadow-[0_2px_3px_rgba(0,0,0,.5)]">
                <path d="M1 1 L1 19 L6 14 L9.5 22 L12.5 20.7 L9 13 L15 13 Z" fill="white" stroke="black" stroke-width="1.2" />
            </svg>
            {tag.map(|t| view! {
                <span class="absolute left-3.5 top-5 whitespace-nowrap rounded-[4px] bg-[#16a34a] px-1.5 py-px font-mono text-[10px] font-semibold leading-[15px] text-white shadow">{t}</span>
            })}
        </div>
    }
}

/// The demo's steps: the current one filled, past ones checked off in green.
#[component]
pub fn StepChips(steps: &'static [&'static str], #[prop(into)] current: Signal<Option<usize>>) -> impl IntoView {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            view! {
                <span
                    aria-current=move || (current.get() == Some(i)).then_some("step")
                    class=move || match current.get() {
                    Some(on) if i == on => "rounded-full border border-gc-text bg-gc-text px-3 py-1 text-[12px] font-semibold text-gc-bg",
                    Some(on) if i < on => "rounded-full border border-gc-accent/50 px-3 py-1 text-[12px] text-gc-ok-text",
                    _ => "rounded-full border border-gc-border px-3 py-1 text-[12px] text-gc-text-faint",
                    }
                >{*s}</span>
            }
        })
        .collect_view()
}

const CONTROL: &str = "inline-flex items-center gap-1.5 rounded-full border border-gc-border px-4 py-1.5 text-[13px] font-semibold text-gc-text hover:border-gc-brand/40 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-gc-brand";

/// Runs the demo again from the top.
#[component]
pub fn ReplayButton(timeline: Timeline) -> impl IntoView {
    view! {
        <button type="button" class=CONTROL on:click=move |_| timeline.play()>
            <span class="material-symbols-outlined text-[16px]">"replay"</span>
            "Replay"
        </button>
    }
}

/// Speed chips (1×, 2×, 4× by default): the one picked sets the timeline's `speed`.
#[component]
pub fn SpeedChips(timeline: Timeline, #[prop(default = &[1., 2., 4.])] speeds: &'static [f64]) -> impl IntoView {
    let speed = timeline.speed;
    view! {
        <div class="inline-flex rounded-full border border-gc-border p-0.5" role="group" aria-label="Playback speed">
            {speeds.iter().map(|&k| {
                let on = move || (speed.get() - k).abs() < 1e-6;
                view! {
                    <button
                        type="button"
                        aria-pressed=move || on().to_string()
                        class=move || if on() {
                            "rounded-full bg-gc-text px-3 py-1 font-mono text-[12.5px] font-semibold text-gc-bg"
                        } else {
                            "rounded-full px-3 py-1 font-mono text-[12.5px] font-semibold text-gc-text-muted hover:text-gc-text"
                        }
                        on:click=move |_| speed.set(k)
                    >
                        {format!("{k}×")}
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

/// Play/Pause (hidden once the run is over) and Replay.
#[component]
pub fn DemoControls(timeline: Timeline) -> impl IntoView {
    let state = timeline.state;
    view! {
        <div class="ml-auto flex gap-2">
            <Show when=move || state.get() != PlayState::Done>
                <button
                    type="button"
                    class=CONTROL
                    on:click=move |_| {
                        if state.get_untracked() == PlayState::Idle {
                            timeline.play();
                        } else {
                            timeline.toggle_pause();
                        }
                    }
                >
                    <span class="material-symbols-outlined text-[16px]">
                        {move || if state.get() == PlayState::Playing { "pause" } else { "play_arrow" }}
                    </span>
                    {move || if state.get() == PlayState::Playing { "Pause" } else { "Play" }}
                </button>
            </Show>
            <ReplayButton timeline />
        </div>
    }
}

/// Step chips and controls on one row, with an optional caption under them.
#[component]
pub fn DemoBar(
    steps: &'static [&'static str],
    #[prop(into)] current: Signal<Option<usize>>,
    timeline: Timeline,
    #[prop(optional, into)] caption: Option<Signal<&'static str>>,
    /// Room kept for the caption, so a longer one doesn't push the window down.
    #[prop(default = "min-h-5")]
    caption_min: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center gap-2 mb-5">
            <StepChips steps current />
            <DemoControls timeline />
            {caption.map(|c| view! {
                <p class=format!("basis-full {caption_min} pt-1 text-[13px] text-gc-text-muted") aria-live="polite">{move || c.get()}</p>
            })}
        </div>
    }
}

/// Whether `el` is far enough on screen for a demo to play.
pub fn in_view(el: NodeRef<html::Div>) -> bool {
    let Some(node) = el.get_untracked() else { return false };
    let h = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.);
    let r = node.get_bounding_client_rect();
    r.top() < h * 0.7 && r.bottom() > 0.
}

/// Plays the demo once, the first time `el` scrolls into view.
pub fn autoplay_when_visible(tl: Timeline, el: NodeRef<html::Div>) {
    let check = move || {
        if tl.state.try_get_untracked() == Some(PlayState::Idle) && in_view(el) {
            tl.play();
        }
    };
    let scroll = window_event_listener(ev::scroll, move |_| check());
    on_cleanup(move || scroll.remove());
    Effect::new(move |_| {
        gloo_timers::callback::Timeout::new(300, check).forget();
    });
}
