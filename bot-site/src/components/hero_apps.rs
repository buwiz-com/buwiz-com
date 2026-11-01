//! What Genie's computer shows in the hero film: Paint painting the Mona Lisa, Maze Run in
//! Chrome, Claude Code in a terminal, a vision viewer boxing a receipt, and Buwiz Books' Ledger.
//! These keep their real-world colors, not the site's theme.
//!
//! The pane's thumbnail lays the desktop out at 960×600; grown, it takes the window's size, down
//! to a phone's. So the pictures are SVG that scales to fit, and the app layouts switch on their
//! own width (container queries), not the viewport's.

use std::fmt::Write;

use leptos::prelude::*;

use crate::components::agent_window::{box_desktop, chrome};
use crate::components::demo_player::Timeline;

#[derive(Clone, Copy, PartialEq)]
pub enum App {
    Desk,
    Paint,
    Maze,
    Code,
    Vision,
    Ledger,
}

/// Maze Run: the buggy build, the reload, the fixed build.
#[derive(Clone, Copy, PartialEq)]
pub enum Run {
    Bug,
    Loading,
    Fixed,
}

// ---------------------------------------------------------------------------------------------
// The Mona Lisa, 24 × 32 blocks.

const W: usize = 24;
const H: usize = 32;
const LISA: [&str; H] = [
    "aaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaHhhhhHaaaaaaaaa",
    "aaaaaaahhhffffhhhaaaaaaa",
    "gggggghhfffffffFhhgggggg",
    "ggggghhhfffffffFhhhggggg",
    "ggggghhhfFFffFFFhhhggggg",
    "bbbbbhhhfFeffefFhhhwwwww",
    "bbbbbhhhffffFffFhhhwwwww",
    "brbbbhhhfffFFffFhhhbbbbb",
    "ccrcchhhffFmmFfFhhhccccc",
    "cccrchhhFfffffFFhhhccccc",
    "cccrrhhhhFfffFFhhhhccccc",
    "cccchhhhhhnnnNhhhhhhcccc",
    "ccchhhhhhnnnnnNhhhhhhccc",
    "cddhhhhhnnnnnnnNhhhhhddc",
    "dddhhhhnnnnnnnnnNhhhhddd",
    "ssddhhhnnnnnnnnnNhhhddss",
    "ssSdddhoooooooooohdddSss",
    "sssSddddddddddddddddSsss",
    "sssSddddddddddddddddSsss",
    "ssssSddddddddddddddSssss",
    "ssssSSddddddddddddSSssss",
    "ssllSSddddddddddddSSllss",
    "sssllSSddddddddddSSSlsss",
    "sssllSSddddddddddSSllsss",
    "sssSkkkkKdddddddSSssssss",
    "ssSkkkkkkkKdddkkKsssssss",
    "ssKkkkkkkkkkkkkKssssssss",
    "SSKkkkkkKkkkkkKSSSSSSSSS",
    "SSSKkkKkkKkkkKSSSSSSSSSS",
    "dddddddddddddddddddddddd",
];
/// Sky, haze, lake, land, path; hair; face, eyes, mouth; chest, neckline; dress, sleeves; hands.
const PALETTE: [(u8, &str); 21] = [
    (b'a', "#a9b796"),
    (b'g', "#8d9f8a"),
    (b'w', "#b3c2ad"),
    (b'b', "#7d8f6a"),
    (b'c', "#5f6b3f"),
    (b'r', "#a59c72"),
    (b'H', "#3b2818"),
    (b'h', "#24180f"),
    (b'f', "#e2bb82"),
    (b'F', "#bb8d56"),
    (b'e', "#3e2a18"),
    (b'm', "#a35e3c"),
    (b'n', "#dcb077"),
    (b'N', "#b8895a"),
    (b'o', "#a5843f"),
    (b'd', "#271d14"),
    (b's', "#8c6b36"),
    (b'S', "#5b4222"),
    (b'l', "#a8874a"),
    (b'k', "#e0b67b"),
    (b'K', "#b28452"),
];
/// The swatches in Paint's toolbar.
const SWATCHES: [u8; 9] = *b"hfFngcdsk";

fn cell_color(cell: usize) -> u8 {
    LISA[cell / W].as_bytes()[cell % W]
}

fn hex(c: u8) -> &'static str {
    PALETTE.iter().find(|(k, _)| *k == c).map(|(_, v)| *v).unwrap_or("#000")
}

/// Paint goes in 4×4 blocks, nearest the eyes first, so the zoomed-in camera sees the face.
fn paint_order() -> Vec<u16> {
    let mut blocks: Vec<(usize, usize)> = (0..H / 4).flat_map(|by| (0..W / 4).map(move |bx| (bx, by))).collect();
    let d = |&(bx, by): &(usize, usize)| {
        let (x, y) = (bx as f64 * 4. + 2. - 12., by as f64 * 4. + 2. - 7.5);
        x * x + y * y
    };
    blocks.sort_by(|a, b| d(a).total_cmp(&d(b)).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));
    blocks
        .into_iter()
        .flat_map(|(bx, by)| (0..16).map(move |k| ((by * 4 + k / 4) * W + bx * 4 + k % 4) as u16))
        .collect()
}

/// One path per color for the cells painted so far.
fn art_paths(cells: impl Iterator<Item = usize>) -> Vec<(&'static str, String)> {
    let mut by: Vec<(u8, String)> = PALETTE.iter().map(|(c, _)| (*c, String::new())).collect();
    for cell in cells {
        let (x, y) = (cell % W, cell / W);
        if let Some((_, d)) = by.iter_mut().find(|(c, _)| *c == cell_color(cell)) {
            let _ = write!(d, "M{x} {y}h1v1h-1z");
        }
    }
    by.into_iter().filter(|(_, d)| !d.is_empty()).map(|(c, d)| (hex(c), d)).collect()
}

/// The finished picture, for the chat.
pub fn lisa_svg(class: &'static str) -> impl IntoView {
    view! {
        <svg viewBox="0 0 24 32" class=class shape-rendering="crispEdges" aria-label="Pixel-art Mona Lisa" role="img">
            {art_paths(0..W * H).into_iter().map(|(fill, d)| view! { <path fill=fill d=d></path> }).collect_view()}
        </svg>
    }
}

const PAINT_ZOOMED: f64 = 4400.;
const PAINT_OUT: f64 = 5200.;
/// How long Paint takes, zoomed in and then zooming out.
pub const PAINT_MS: u32 = (PAINT_ZOOMED + PAINT_OUT) as u32;
/// Cells painted at 800%: the first four blocks.
const ZOOMED_CELLS: usize = 64;

// ---------------------------------------------------------------------------------------------
// Maze Run.

const MAZE: [&str; 11] = [
    "#################",
    "#.....#.........#",
    "#####.###.#.###.#",
    "#...#...#.#...#.#",
    "###.###.#####.###",
    "#...#...#...#...#",
    "#.#.#.###.#.###.#",
    "#.#.#.#...#.....#",
    "#.###.#.#######.#",
    "#.......#.......#",
    "#################",
];
const MW: usize = 17;
const MH: usize = 11;
const START: (u8, u8) = (1, 1);
const EXIT: (u8, u8) = (9, 15);
/// The buggy build lets the player step into the wall at (1, 6).
pub const BUG_WALK: [(u8, u8); 6] = [(1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6)];
const BUG_WALL: (u8, u8) = (1, 6);

fn wall(r: u8, c: u8) -> bool {
    MAZE[r as usize].as_bytes()[c as usize] == b'#'
}

/// The shortest way from the start to the exit, through open cells only.
fn solve() -> Vec<(u8, u8)> {
    let mut prev = vec![None::<(u8, u8)>; MW * MH];
    let mut seen = [false; MW * MH];
    let idx = |(r, c): (u8, u8)| r as usize * MW + c as usize;
    let mut queue = std::collections::VecDeque::from([START]);
    seen[idx(START)] = true;
    while let Some(p) = queue.pop_front() {
        if p == EXIT {
            break;
        }
        let (r, c) = p;
        for n in [(r + 1, c), (r - 1, c), (r, c + 1), (r, c - 1)] {
            if !wall(n.0, n.1) && !seen[idx(n)] {
                seen[idx(n)] = true;
                prev[idx(n)] = Some(p);
                queue.push_back(n);
            }
        }
    }
    let mut path = vec![EXIT];
    while let Some(p) = prev[idx(*path.last().unwrap_or(&START))] {
        path.push(p);
    }
    path.reverse();
    path
}

fn walls_path() -> String {
    let mut d = String::new();
    for r in 0..MH as u8 {
        for c in 0..MW as u8 {
            if wall(r, c) {
                let _ = write!(d, "M{c} {r}h1v1h-1z");
            }
        }
    }
    d
}

fn center((r, c): (u8, u8)) -> (f64, f64) {
    (c as f64 + 0.5, r as f64 + 0.5)
}

// ---------------------------------------------------------------------------------------------
// Claude Code.

#[derive(Clone, Copy)]
enum Ln {
    Me(&'static str),
    Tool(&'static str, &'static str),
    Out(&'static str),
    Say(&'static str),
    Del(&'static str),
    Add(&'static str),
    Pass(&'static str),
}

const CLAUDE: [Ln; 12] = [
    Ln::Me("Players walk through walls in Maze Run. Fix it."),
    Ln::Tool("Bash", "cargo run --example replay"),
    Ln::Out("player entered a wall at (1, 6) on move 5"),
    Ln::Tool("Read", "src/player.rs"),
    Ln::Out("Read 48 lines"),
    Ln::Say("Found it: the move checks the tile the player is on, not the tile it's moving to."),
    Ln::Tool("Update", "src/player.rs"),
    Ln::Del("31 -     if !grid.wall(self.pos) {"),
    Ln::Add("31 +     if !grid.wall(next) {"),
    Ln::Tool("Bash", "cargo test"),
    Ln::Pass("test result: ok. 9 passed; 0 failed"),
    Ln::Say("Fixed. Collision checks the next tile now. Reloading the game."),
];
pub const CLAUDE_LINES: usize = CLAUDE.len();

// ---------------------------------------------------------------------------------------------
// The receipt and what the vision model finds on it.

/// Field, value, color, and its box on the receipt (x, y, w, h in the receipt's units).
/// x, y, w, h in the receipt's units.
type Rect = (f64, f64, f64, f64);

const FIELDS: [(&str, &str, &str, Rect); 6] = [
    ("VENDOR", "Jollibee · Makati", "#2563eb", (134., 28., 220., 55.)),
    ("DATE", "Oct 8, 2026 · 12:41", "#7c3aed", (113., 131., 158., 19.)),
    ("ITEM", "2pc Chickenjoy ×3 · ₱897.00", "#0e7490", (113., 167., 264., 19.)),
    ("ITEM", "Jolly Spaghetti ×2 · ₱394.00", "#0e7490", (113., 187., 264., 19.)),
    ("TAX", "VAT 12% · ₱138.39", "#b45309", (113., 241., 264., 19.)),
    ("TOTAL", "₱1,291.00", "#059669", (113., 265., 264., 23.)),
];
pub const FIELD_COUNT: usize = FIELDS.len();

// ---------------------------------------------------------------------------------------------

/// Everything Genie's screen shows, driven by the hero's script.
#[derive(Clone, Copy)]
pub struct Screens {
    pub app: RwSignal<App>,
    painted: RwSignal<usize>,
    zoom: RwSignal<f64>,
    focus: RwSignal<(f64, f64)>,
    pub run: RwSignal<Run>,
    player: RwSignal<(f64, f64)>,
    trail: RwSignal<usize>,
    pub crash: RwSignal<bool>,
    bump: RwSignal<bool>,
    pub solved: RwSignal<bool>,
    pub lines: RwSignal<usize>,
    pub boxes: RwSignal<usize>,
    pub classified: RwSignal<bool>,
    pub posted: RwSignal<usize>,
    order: StoredValue<Vec<u16>>,
    path: StoredValue<Vec<(u8, u8)>>,
}

impl Screens {
    pub fn new() -> Self {
        let order = paint_order();
        let first = block_center(order[0] as usize);
        Self {
            app: RwSignal::new(App::Desk),
            painted: RwSignal::new(0),
            zoom: RwSignal::new(8.),
            focus: RwSignal::new(first),
            run: RwSignal::new(Run::Bug),
            player: RwSignal::new(center(START)),
            trail: RwSignal::new(1),
            crash: RwSignal::new(false),
            bump: RwSignal::new(false),
            solved: RwSignal::new(false),
            lines: RwSignal::new(0),
            boxes: RwSignal::new(0),
            classified: RwSignal::new(false),
            posted: RwSignal::new(0),
            order: StoredValue::new(order),
            path: StoredValue::new(solve()),
        }
    }

    pub fn reset(self) {
        self.app.set(App::Desk);
        self.reset_paint();
        self.maze(Run::Bug);
        self.lines.set(0);
        self.boxes.set(0);
        self.classified.set(false);
        self.posted.set(0);
    }

    pub fn reset_paint(self) {
        self.painted.set(0);
        self.zoom.set(8.);
        let first = self.order.with_value(|o| block_center(o[0] as usize));
        self.focus.set(first);
    }

    /// Maze Run at the start, in `run`'s build.
    pub fn maze(self, run: Run) {
        self.run.set(run);
        self.player.set(center(START));
        self.trail.set(1);
        self.crash.set(false);
        self.bump.set(false);
        self.solved.set(false);
    }

    /// The fixed build's way out, start to exit.
    pub fn solution(self) -> Vec<(u8, u8)> {
        self.path.get_value()
    }

    /// Paints the picture: four blocks at 800% with the camera on each, then a smooth zoom out to
    /// 100% while the rest fills in. Takes `PAINT_MS`.
    pub fn paint(self, tl: Timeline) {
        let t0 = tl.now();
        let total = W * H;
        let order = self.order;
        let mut last = t0;
        let mut f = self.focus.get_untracked();
        let mut from: Option<(f64, f64)> = None;
        tl.every_frame(move |now| {
            let t = now - t0;
            let dt = (now - last).max(0.);
            last = now;
            if t < PAINT_ZOOMED {
                let n = (ZOOMED_CELLS as f64 * t / PAINT_ZOOMED) as usize;
                let target = order.with_value(|o| block_center(o[n.min(ZOOMED_CELLS - 1)] as usize));
                let k = (dt / 200.).min(1.);
                f = (f.0 + (target.0 - f.0) * k, f.1 + (target.1 - f.1) * k);
                self.painted.set(n);
                self.zoom.set(8.);
                self.focus.set(f);
                return true;
            }
            let u = ((t - PAINT_ZOOMED) / PAINT_OUT).clamp(0., 1.);
            let e = u * u * (3. - 2. * u);
            let a = *from.get_or_insert(f);
            let n = ZOOMED_CELLS + ((total - ZOOMED_CELLS) as f64 * u.powf(1.25)).round() as usize;
            self.painted.set(n.min(total));
            self.zoom.set((8f64.ln() * (1. - e)).exp());
            self.focus.set((a.0 + (12. - a.0) * e, a.1 + (16. - a.1) * e));
            u < 1.
        });
    }

    /// Walks the player along `cells`, one every `step` ms; the trail counts on from `trail0`.
    pub fn walk(self, tl: Timeline, cells: Vec<(u8, u8)>, trail0: usize, step: f64) {
        let t0 = tl.now();
        let n = cells.len();
        tl.every_frame(move |now| {
            let p = ((now - t0) / step).max(0.);
            let i = (p.floor() as usize).min(n - 1);
            let (a, b) = (center(cells[i]), center(cells[(i + 1).min(n - 1)]));
            let u = if i + 1 < n { p - i as f64 } else { 0. };
            self.player.set((a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u));
            self.trail.set(trail0 + i);
            p < (n - 1) as f64
        });
    }

    /// The player nudges toward `dir` from `at` and the wall holds: it flashes and pushes back.
    pub fn bump(self, tl: Timeline, at: (u8, u8), dir: (f64, f64), ms: f64) {
        let t0 = tl.now();
        let c = center(at);
        self.bump.set(true);
        tl.every_frame(move |now| {
            let u = ((now - t0) / ms).clamp(0., 1.);
            let off = (std::f64::consts::PI * u).sin() * 0.3;
            self.player.set((c.0 + dir.0 * off, c.1 + dir.1 * off));
            if u >= 1. {
                self.bump.set(false);
            }
            u < 1.
        });
    }
}

impl Default for Screens {
    fn default() -> Self {
        Self::new()
    }
}

fn block_center(cell: usize) -> (f64, f64) {
    ((cell % W / 4) as f64 * 4. + 2., (cell / W / 4) as f64 * 4. + 2.)
}

// ---------------------------------------------------------------------------------------------
// The desktop and its windows.

/// A window on the Box, the same size and place as Chrome. `dark` for the terminal and viewer.
fn app_win(
    icon: impl IntoView + 'static,
    title: &'static str,
    dark: bool,
    body: impl IntoView + 'static,
) -> impl IntoView {
    let (frame, bar) = if dark {
        (
            "absolute bottom-[68px] left-1/2 top-11 flex w-[min(94%,780px)] -translate-x-1/2 flex-col overflow-hidden rounded-[10px] bg-[#0d1117] shadow-[0_18px_40px_rgba(0,0,0,.45)] ring-1 ring-white/10",
            "flex h-8 shrink-0 items-center gap-2 border-b border-white/10 bg-[#1c2128] px-3 text-[12px] text-[#c9d1d9]",
        )
    } else {
        (
            "absolute bottom-[68px] left-1/2 top-11 flex w-[min(94%,780px)] -translate-x-1/2 flex-col overflow-hidden rounded-[10px] bg-[#f3f3f3] shadow-[0_18px_40px_rgba(0,0,0,.45)] ring-1 ring-black/20",
            "flex h-8 shrink-0 items-center gap-2 border-b border-black/10 bg-[#ebebeb] px-3 text-[12px] text-[#1f2328]",
        )
    };
    view! {
        <div class=frame>
            <div class=bar>
                {icon}
                <span class="min-w-0 truncate">{title}</span>
                <span class="ml-auto flex shrink-0 items-center gap-2.5 opacity-70">
                    <span class="material-symbols-outlined text-[14px]">"remove"</span>
                    <span class="material-symbols-outlined text-[13px]">"crop_square"</span>
                    <span class="material-symbols-outlined text-[14px]">"close"</span>
                </span>
            </div>
            <div class="@container relative min-h-0 flex-1 overflow-hidden">{body}</div>
        </div>
    }
}

/// Genie's Box: whichever app the script has up, and a dock that marks it.
pub fn desktop(s: Screens) -> impl IntoView {
    let dock =
        [("palette", "Paint"), ("public", "Chrome"), ("terminal", "Terminal"), ("center_focus_strong", "Vision")];
    let on = move |i: usize| match s.app.get() {
        App::Paint => i == 0,
        App::Maze | App::Ledger => i == 1,
        App::Code => i == 2,
        App::Vision => i == 3,
        App::Desk => false,
    };
    box_desktop(
        "Buwiz Bot Box · Genie",
        (),
        move || match s.app.get() {
            App::Desk => ().into_any(),
            App::Paint => paint_app(s).into_any(),
            App::Maze => maze_app(s).into_any(),
            App::Code => claude_app(s).into_any(),
            App::Vision => vision_app(s).into_any(),
            App::Ledger => buwiz_app(s).into_any(),
        },
        dock.into_iter()
            .enumerate()
            .map(|(i, (glyph, label))| {
                view! {
                    <span class="relative" aria-label=label>
                        <span class=move || if on(i) {
                            "lx-dock-item material-symbols-outlined bg-white/20 text-[20px] text-white"
                        } else {
                            "lx-dock-item material-symbols-outlined text-[20px] text-[#cbd5e1]"
                        }>{glyph}</span>
                        <span class=move || if on(i) {
                            "absolute -bottom-1 left-1/2 h-1 w-1 -translate-x-1/2 rounded-full bg-white"
                        } else {
                            "hidden"
                        }></span>
                    </span>
                }
            })
            .collect_view(),
    )
}

// Paint ----------------------------------------------------------------------------------------

fn paint_app(s: Screens) -> impl IntoView {
    let total = W * H;
    let order = s.order;
    let paths = Memo::new(move |_| {
        let n = s.painted.get();
        order.with_value(|o| art_paths(o[..n.min(o.len())].iter().map(|&c| c as usize)))
    });
    let brush = move || {
        let n = s.painted.get();
        (n < total).then(|| order.with_value(|o| o[n] as usize))
    };
    let swatch = move || brush().map(cell_color).unwrap_or(b'h');
    let view_box = move || {
        let z = s.zoom.get();
        let (fx, fy) = s.focus.get();
        let (w, h) = (26. / z, 34. / z);
        format!("{:.3} {:.3} {:.3} {:.3}", fx - w / 2., fy - h / 2., w, h)
    };
    let grid = {
        let mut d = String::new();
        for x in 1..W {
            let _ = write!(d, "M{x} 0v{H}");
        }
        for y in 1..H {
            let _ = write!(d, "M0 {y}h{W}");
        }
        d
    };
    let tools = ["brush", "edit", "format_color_fill", "colorize", "ink_eraser", "text_fields"];
    app_win(
        view! { <span class="material-symbols-outlined text-[15px] text-[#0b72d9]">"palette"</span> },
        "mona-lisa.png — Paint",
        false,
        view! {
            <div class="flex h-full flex-col text-[#1f2328]">
                <div class="flex h-7 shrink-0 items-center gap-4 px-3 text-[12px] text-[#3b3b3b]">
                    <span>"File"</span><span>"Edit"</span><span>"View"</span><span>"Image"</span><span class="hidden @sm:inline">"Colors"</span><span class="hidden @sm:inline">"Help"</span>
                </div>
                <div class="flex h-10 shrink-0 items-center gap-1 overflow-hidden border-y border-black/10 bg-[#f9f9f9] px-2">
                    {tools.iter().enumerate().map(|(i, t)| view! {
                        <span class=if i == 0 {
                            "material-symbols-outlined grid h-7 w-7 shrink-0 place-items-center rounded bg-[#dbe8f8] text-[17px] text-[#0b72d9] ring-1 ring-[#0b72d9]/40"
                        } else {
                            "material-symbols-outlined grid h-7 w-7 shrink-0 place-items-center rounded text-[17px] text-[#444]"
                        }>{*t}</span>
                    }).collect_view()}
                    <span class="mx-1.5 h-6 w-px shrink-0 bg-black/15"></span>
                    {SWATCHES.iter().map(|&c| view! {
                        <span
                            class=move || if swatch() == c {
                                "h-5 w-5 shrink-0 rounded-full ring-2 ring-[#0b72d9] ring-offset-1"
                            } else {
                                "h-5 w-5 shrink-0 rounded-full ring-1 ring-black/15"
                            }
                            style=format!("background:{}", hex(c))
                        ></span>
                    }).collect_view()}
                </div>
                <div class="relative min-h-0 flex-1 bg-[#d9dde4]">
                    <svg class="absolute inset-0 h-full w-full" viewBox=view_box preserveAspectRatio="xMidYMid meet" shape-rendering="crispEdges" aria-hidden="true">
                        <rect x="0.18" y="0.25" width="24" height="32" fill="#000" opacity="0.14"></rect>
                        <rect width="24" height="32" fill="#fff"></rect>
                        {move || paths.get().into_iter().map(|(fill, d)| view! { <path fill=fill d=d></path> }).collect_view()}
                        <path
                            d=grid
                            fill="none"
                            stroke="#000"
                            stroke-width="1"
                            vector-effect="non-scaling-stroke"
                            opacity=move || format!("{:.3}", ((s.zoom.get() - 2.5) / 4.).clamp(0., 1.) * 0.28)
                        ></path>
                        {move || brush().map(|c| view! {
                            <rect x=(c % W).to_string() y=(c / W).to_string() width="1" height="1" fill="none" stroke="#0b72d9" stroke-width="2" vector-effect="non-scaling-stroke"></rect>
                        })}
                    </svg>
                </div>
                <div class="flex h-6 shrink-0 items-center gap-3 border-t border-black/10 bg-[#f3f3f3] px-3 text-[11px] text-[#555]">
                    <span class="hidden @sm:inline">"240 × 320px"</span>
                    <span class="hidden @md:inline">"Brush · 10px"</span>
                    <span class="ml-auto flex items-center gap-2">
                        <span>"−"</span>
                        <span class="relative h-1 w-16 rounded-full bg-black/15 @sm:w-24">
                            <span
                                class="absolute -top-1 h-3 w-1.5 rounded-sm bg-[#0b72d9]"
                                style=move || format!("left: calc({:.3} * (100% - 6px))", s.zoom.get().ln() / 8f64.ln())
                            ></span>
                        </span>
                        <span>"+"</span>
                        <span class="w-10 text-right font-semibold tabular-nums text-[#1f2328]">{move || format!("{:.0}%", s.zoom.get() * 100.)}</span>
                    </span>
                </div>
            </div>
        },
    )
}

// Maze Run -------------------------------------------------------------------------------------

fn maze_app(s: Screens) -> impl IntoView {
    let walls = walls_path();
    let trail = move || {
        let n = s.trail.get();
        let cells: Vec<(u8, u8)> = match s.run.get() {
            Run::Bug => BUG_WALK[..n.min(BUG_WALK.len())].to_vec(),
            Run::Loading => Vec::new(),
            Run::Fixed => s.path.with_value(|p| p[..n.min(p.len())].to_vec()),
        };
        // A line through the cells walked so far, on to where the player is now.
        let mut d = String::new();
        for (i, cell) in cells.into_iter().enumerate() {
            let (x, y) = center(cell);
            let _ = write!(d, "{}{x} {y}", if i == 0 { "M" } else { "L" });
        }
        if !d.is_empty() {
            let (x, y) = s.player.get();
            let _ = write!(d, "L{x:.3} {y:.3}");
        }
        d
    };
    let status = move || {
        if s.run.get() == Run::Loading {
            view! { <span class="text-[#8b93a7]">"Reloading…"</span> }.into_any()
        } else if s.crash.get() {
            view! { <span class="font-semibold text-[#f87171]">"✗ player went through a wall"</span> }.into_any()
        } else if s.solved.get() {
            view! { <span class="font-semibold text-[#34d399]">"✓ reached the exit · 0 errors"</span> }.into_any()
        } else {
            view! { <span class="text-[#8b93a7]">"Playing… arrow keys to move"</span> }.into_any()
        }
    };
    let (wr, wc) = BUG_WALL;
    chrome(
        view! { <span class="grid h-3.5 w-3.5 shrink-0 place-items-center rounded-[3px] bg-[#0b1020] text-[8px] font-black leading-none text-[#fbbf24]">"M"</span> },
        "Maze Run",
        "localhost:8080",
        "/",
        "bg-[#0b1020] text-[#e6e9f2]",
        "",
        view! {
            <div class="flex h-full flex-col items-center gap-2 p-3">
                <div class="flex w-full max-w-[560px] items-center justify-between text-[12px]">
                    <span class="font-black tracking-[0.2em] text-[#fbbf24]">"MAZE RUN"</span>
                    <span class="text-[#8b93a7]">
                        {move || if s.run.get() == Run::Fixed { "Level 3 · build 0.4.1" } else { "Level 3 · build 0.4.0" }}
                    </span>
                </div>
                <div class="relative min-h-0 w-full flex-1">
                    <svg class="absolute inset-0 h-full w-full" viewBox="0 0 17 11" preserveAspectRatio="xMidYMid meet" aria-label="Maze Run board" role="img">
                        <rect width="17" height="11" rx="0.25" fill="#0b1020"></rect>
                        <path d=walls fill="#2f6fe4"></path>
                        <path d=trail fill="none" stroke="#f59e0b" stroke-width="0.2" stroke-linecap="round" stroke-linejoin="round" opacity="0.6"></path>
                        <rect
                            x=wc.to_string()
                            y=wr.to_string()
                            width="1"
                            height="1"
                            class=move || if s.crash.get() { "hv-flash" } else if s.bump.get() { "hv-hold" } else { "" }
                            fill=move || if s.crash.get() { "#ef4444" } else if s.bump.get() { "#93c5fd" } else { "#2f6fe4" }
                        ></rect>
                        <rect x="15.12" y="9.12" width="0.76" height="0.76" rx="0.14" fill="#10b981"></rect>
                        <path d="M15.38 9.3v.42M15.38 9.3l.32.12-.32.12" stroke="#ecfdf5" stroke-width="0.06" fill="none"></path>
                        <rect x="1.2" y="1.2" width="0.6" height="0.6" rx="0.1" fill="none" stroke="#fbbf24" stroke-width="0.04" opacity="0.6"></rect>
                        {move || {
                            let (x, y) = s.player.get();
                            let fill = if s.crash.get() { "#ef4444" } else { "#fbbf24" };
                            view! {
                                <circle cx=format!("{x:.3}") cy=format!("{y:.3}") r="0.44" fill=fill opacity="0.25"></circle>
                                <circle cx=format!("{x:.3}") cy=format!("{y:.3}") r="0.28" fill=fill stroke="#fff7d6" stroke-width="0.06"></circle>
                            }
                        }}
                    </svg>
                    {move || (s.run.get() == Run::Loading).then(|| view! {
                        <div class="absolute inset-0 grid place-items-center bg-[#0b1020]">
                            <span class="h-7 w-7 animate-spin rounded-full border-[3px] border-[#fbbf24]/25 border-t-[#fbbf24]"></span>
                        </div>
                    })}
                </div>
                <div class="text-[13px]" aria-live="polite">{status}</div>
            </div>
        },
    )
}

// Claude Code ----------------------------------------------------------------------------------

fn claude_line(l: Ln) -> impl IntoView {
    match l {
        Ln::Me(t) => view! {
            <div class="hv-line-in flex gap-2 text-[#8b949e]"><span>">"</span><span class="text-[#e6edf3]">{t}</span></div>
        }
        .into_any(),
        Ln::Tool(name, arg) => view! {
            <div class="hv-line-in mt-1.5 flex gap-2"><span class="text-[#3fb950]">"●"</span><span><b class="text-[#e6edf3]">{name}</b><span class="text-[#8b949e]">"("{arg}")"</span></span></div>
        }
        .into_any(),
        Ln::Out(t) => view! {
            <div class="hv-line-in flex gap-2 pl-3 text-[#8b949e]"><span>"⎿"</span><span>{t}</span></div>
        }
        .into_any(),
        Ln::Say(t) => view! {
            <div class="hv-line-in mt-1.5 flex gap-2 text-[#e6edf3]"><span>"●"</span><span>{t}</span></div>
        }
        .into_any(),
        Ln::Del(t) => view! {
            <div class="hv-line-in ml-5 mt-0.5 whitespace-pre rounded-sm bg-[#4b1113] px-2 text-[#ffa198]">{t}</div>
        }
        .into_any(),
        Ln::Add(t) => view! {
            <div class="hv-line-in ml-5 whitespace-pre rounded-sm bg-[#0f3d22] px-2 text-[#7ee787]">{t}</div>
        }
        .into_any(),
        Ln::Pass(t) => view! {
            <div class="hv-line-in flex gap-2 pl-3 text-[#3fb950]"><span class="text-[#8b949e]">"⎿"</span><span>{t}</span></div>
        }
        .into_any(),
    }
}

fn claude_app(s: Screens) -> impl IntoView {
    app_win(
        view! { <span class="material-symbols-outlined text-[15px] text-[#8b949e]">"terminal"</span> },
        "genie@box: ~/maze-run — claude",
        true,
        view! {
            <div class="flex h-full flex-col gap-2 p-3 font-mono text-[10.5px] leading-[1.55] text-[#c9d1d9] @lg:text-[12.5px]">
                <div class="shrink-0 rounded-md border border-[#d97757]/70 px-2.5 py-1.5">
                    <div><span class="text-[#d97757]">"✻"</span>" Welcome to "<b class="text-[#e6edf3]">"Claude Code"</b></div>
                    <div class="text-[#8b949e]">"cwd: ~/maze-run"</div>
                </div>
                <div class="min-h-0 flex-1 overflow-hidden break-words">
                    {move || CLAUDE[..s.lines.get().min(CLAUDE_LINES)].iter().map(|&l| claude_line(l)).collect_view()}
                    {move || (s.lines.get() < CLAUDE_LINES).then(|| view! {
                        <div class="mt-1.5 animate-pulse text-[#d97757]">"✻ Working… "<span class="text-[#8b949e]">"(esc to interrupt)"</span></div>
                    })}
                </div>
                <div class="shrink-0 rounded-md border border-[#30363d] px-2.5 py-1 text-[#8b949e]">"> "</div>
            </div>
        },
    )
}

// The receipt ----------------------------------------------------------------------------------

/// A thermal receipt; with `boxes`, the vision model's boxes and labels land on it one by one.
/// Text and boxes share one coordinate space (`textLength` pins each line's width), so the boxes
/// sit exactly on the lines at any size.
pub fn receipt_svg(class: &'static str, boxes: Option<RwSignal<usize>>) -> impl IntoView {
    const MONO: &str = "ui-monospace,SFMono-Regular,Menlo,monospace";
    let line = |x: f64, y: f64, size: f64, len: f64, anchor: &'static str, bold: bool, t: &'static str| {
        view! {
            <text
                x=x.to_string()
                y=y.to_string()
                font-size=size.to_string()
                font-weight=if bold { "700" } else { "400" }
                text-anchor=anchor
                textLength=len.to_string()
                lengthAdjust="spacingAndGlyphs"
                font-family=MONO
                fill="#222"
            >
                {t}
            </text>
        }
    };
    let pair = move |y: f64, l: &'static str, ll: f64, r: &'static str, rl: f64, size: f64, bold: bool| {
        view! { {line(118., y, size, ll, "start", bold, l)}{line(372., y, size, rl, "end", bold, r)} }
    };
    let dash = |y: f64| view! { <path d=format!("M116 {y}H372") stroke="#555" stroke-width="1" stroke-dasharray="4 3"></path> };
    let bars = [
        3., 1., 2., 1., 1., 3., 2., 1., 1., 2., 3., 1., 2., 2., 1., 3., 1., 1., 2., 1., 3., 2., 1., 2., 1., 1., 3., 1.,
        2., 1., 2., 3., 1., 1., 2., 1., 1., 3., 2., 1., 2., 1., 3., 1.,
    ];
    let mut bx = 150.;
    let mut bar_d = String::new();
    for (i, w) in bars.iter().enumerate() {
        if i % 2 == 0 {
            let _ = write!(bar_d, "M{bx} 392h{w}v36h-{w}z");
        }
        bx += w + 1.;
    }
    let (vb, gutter) = if boxes.is_some() { ("0 0 400 470", true) } else { ("104 10 280 450", false) };
    view! {
        <svg viewBox=vb class=class role="img" aria-label="Receipt from Jollibee Makati, total 1,291 pesos">
            <rect x="106" y="13" width="280" height="450" fill="#000" opacity="0.18"></rect>
            <rect x="104" y="10" width="280" height="450" fill="#fbfaf6"></rect>
            {line(244., 46., 20., 118., "middle", true, "JOLLIBEE")}
            {line(244., 64., 11., 196., "middle", false, "Jollibee Foods Corp. · Makati")}
            {line(244., 79., 10., 210., "middle", false, "Glorietta 3, Ayala Center, Makati")}
            {line(244., 93., 10., 200., "middle", false, "VAT REG TIN 000-388-474-00117")}
            {dash(108.)}
            {pair(126., "OR# 0412-88213", 104., "DINE IN", 52., 12., false)}
            {line(118., 144., 12., 148., "start", false, "10/08/2026  12:41 PM")}
            {dash(158.)}
            {pair(180., "2PC CHICKENJOY x3", 128., "897.00", 46., 12., false)}
            {pair(200., "JOLLY SPAGHETTI x2", 136., "394.00", 46., 12., false)}
            {dash(216.)}
            {pair(236., "VATable Sales", 96., "1,152.61", 62., 12., false)}
            {pair(254., "VAT 12%", 54., "138.39", 46., 12., false)}
            {pair(280., "TOTAL", 50., "1,291.00", 74., 15., true)}
            {pair(302., "CASH", 30., "1,500.00", 62., 12., false)}
            {pair(320., "CHANGE", 44., "209.00", 46., 12., false)}
            {dash(336.)}
            {line(244., 358., 12., 160., "middle", false, "THANK YOU, KAIBIGAN!")}
            {line(244., 376., 9., 206., "middle", false, "THIS SERVES AS AN OFFICIAL RECEIPT")}
            <path d=bar_d fill="#222"></path>
            {line(244., 444., 9., 90., "middle", false, "0412882130")}
            {gutter.then(|| boxes.map(|n| {
                view! {
                    {move || (n.get() < FIELD_COUNT).then(|| view! {
                        <rect class="hv-scan" x="104" y="10" width="280" height="3" fill="#a78bfa" opacity="0.8"></rect>
                    })}
                    {FIELDS.iter().enumerate().map(|(i, &(label, _, color, (x, y, w, h)))| {
                        let cy = y + h / 2.;
                        view! {
                            {move || (n.get() > i).then(|| view! {
                                <g class="hv-snap">
                                    <rect x=x.to_string() y=y.to_string() width=w.to_string() height=h.to_string() rx="3" fill=color fill-opacity="0.1" stroke=color stroke-width="2"></rect>
                                    <path d=format!("M92 {cy}H{x}") stroke=color stroke-width="1.5"></path>
                                    <rect x="6" y=(cy - 8.).to_string() width="86" height="16" rx="4" fill=color></rect>
                                    <text x="49" y=(cy + 3.5).to_string() font-size="9.5" font-weight="700" text-anchor="middle" fill="#fff" font-family=MONO letter-spacing="0.5">{label}</text>
                                </g>
                            })}
                        }
                    }).collect_view()}
                }
            }))}
        </svg>
    }
}

fn classified_card() -> impl IntoView {
    view! {
        <div class="hv-snap rounded-lg border border-white/10 bg-[#161b22] p-3 text-[12px] text-[#c9d1d9] shadow-lg">
            <div class="mb-1.5 flex items-center gap-2 font-semibold text-white">
                <span class="h-2 w-2 rounded-full bg-[#a78bfa]"></span>
                "Classified · 0.4s"
            </div>
            <div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
                <span class="text-[#8b949e]">"Type"</span><b class="text-white">"Meal · client"</b>
                <span class="text-[#8b949e]">"Category"</span><b class="text-white">"Representation"</b>
                <span class="text-[#8b949e]">"VAT"</span><b class="text-white">"input, 12%"</b>
            </div>
            <div class="mt-1.5 text-[#34d399]">"6 fields · 98% sure"</div>
        </div>
    }
}

fn vision_app(s: Screens) -> impl IntoView {
    app_win(
        view! { <span class="material-symbols-outlined text-[15px] text-[#a78bfa]">"center_focus_strong"</span> },
        "receipt.jpg — Vision · Gemini 4 Argon",
        true,
        view! {
            <div class="flex h-full bg-[#0b0e14]">
                <div class="relative min-w-0 flex-1 p-2 @lg:p-3">
                    {receipt_svg("h-full w-full", Some(s.boxes))}
                    {move || s.classified.get().then(|| view! {
                        <div class="absolute inset-x-2 bottom-2 @lg:hidden">{classified_card()}</div>
                    })}
                </div>
                <div class="hidden w-[250px] shrink-0 flex-col gap-2 border-l border-white/10 p-3 text-[12px] @lg:flex">
                    <div class="text-[11px] uppercase tracking-wider text-[#8b949e]">"Fields"</div>
                    {FIELDS.iter().enumerate().map(|(i, &(label, value, color, _))| view! {
                        {move || (s.boxes.get() > i).then(|| view! {
                            <div class="hv-line-in flex items-baseline gap-2">
                                <span class="w-14 shrink-0 font-mono text-[10px] font-bold" style=format!("color:{color}")>{label}</span>
                                <span class="min-w-0 text-[#e6edf3]">{value}</span>
                            </div>
                        })}
                    }).collect_view()}
                    <div class="mt-auto">{move || s.classified.get().then(classified_card)}</div>
                </div>
            </div>
        },
    )
}

// Buwiz Books ----------------------------------------------------------------------------------

/// A ledger line: date, party, its color and initials, memo, source chip, category, debit, credit.
struct Entry {
    day: &'static str,
    party: &'static str,
    color: &'static str,
    initials: &'static str,
    memo: &'static str,
    src: (&'static str, &'static str, &'static str),
    category: &'static str,
    debit: &'static str,
    credit: &'static str,
}

/// JE-0412, posted by Genie: the source chip is its avatar.
const JE: [Entry; 3] = [
    Entry {
        day: "8",
        party: "Jollibee Makati",
        color: "#dc2626",
        initials: "JM",
        memo: "JE-0412 · Meal · client",
        src: ("Ge", "#b9a5ff", "#1a1a1a"),
        category: "Representation",
        debit: "₱1,152.61",
        credit: "",
    },
    Entry {
        day: "8",
        party: "Jollibee Makati",
        color: "#dc2626",
        initials: "JM",
        memo: "JE-0412 · VAT 12%",
        src: ("Ge", "#b9a5ff", "#1a1a1a"),
        category: "Input VAT",
        debit: "₱138.39",
        credit: "",
    },
    Entry {
        day: "8",
        party: "Jollibee Makati",
        color: "#dc2626",
        initials: "JM",
        memo: "JE-0412 · paid in cash",
        src: ("Ge", "#b9a5ff", "#1a1a1a"),
        category: "Cash",
        debit: "",
        credit: "₱1,291.00",
    },
];

const BEFORE: [Entry; 4] = [
    Entry {
        day: "7",
        party: "Meralco",
        color: "#f97316",
        initials: "ME",
        memo: "Electricity · September",
        src: ("B", "#d97706", "#fff"),
        category: "Utilities",
        debit: "₱8,412.50",
        credit: "",
    },
    Entry {
        day: "6",
        party: "PLDT Home",
        color: "#2563eb",
        initials: "PH",
        memo: "Fiber 1 Gbps",
        src: ("B", "#d97706", "#fff"),
        category: "Communication",
        debit: "₱2,899.00",
        credit: "",
    },
    Entry {
        day: "5",
        party: "Grab Philippines",
        color: "#16a34a",
        initials: "GP",
        memo: "Client visit, BGC",
        src: ("I", "#6366f1", "#fff"),
        category: "Transportation",
        debit: "₱486.00",
        credit: "",
    },
    Entry {
        day: "3",
        party: "Acme Retail Inc.",
        color: "#7c3aed",
        initials: "AR",
        memo: "INV-0193",
        src: ("INV", "#0891b2", "#fff"),
        category: "Sales Revenue",
        debit: "",
        credit: "₱64,000.00",
    },
];

const LEDGER_COLS: &str = "grid grid-cols-[56px_minmax(0,1fr)_34px_118px_86px_86px] items-center";

fn ledger_row(e: &'static Entry, new: bool) -> impl IntoView {
    let avatar = |size: &'static str| {
        view! {
            <span class=format!("grid {size} shrink-0 place-items-center rounded-full text-[9.5px] font-bold text-white") style=format!("background:{}", e.color)>{e.initials}</span>
        }
    };
    let (src, src_bg, src_fg) = e.src;
    let amount =
        if e.debit.is_empty() { ("Cr", e.credit, "text-[#16a34a]") } else { ("Dr", e.debit, "text-[#1e293b]") };
    view! {
        <div class=if new {
            "hv-row-in relative border-b border-[#bbf7d0] bg-[#f0fdf4]"
        } else {
            "relative border-b border-[#f1f5f9]"
        }>
            {new.then(|| view! { <span class="absolute inset-y-0 left-0 w-1 bg-[#22c55e]"></span> })}
            // Wide: Buwiz's grid row.
            <div class=format!("hidden {LEDGER_COLS} @lg:grid")>
                <div class="py-2 text-center">
                    <div class="text-[12.5px] font-semibold leading-tight">{format!("Oct {}", e.day)}</div>
                    <div class="text-[10px] text-[#94a3b8]">"2026"</div>
                </div>
                <div class="flex min-w-0 items-center gap-2 px-2 py-2">
                    {avatar("h-7 w-7")}
                    <div class="min-w-0">
                        <div class="truncate text-[12.5px] font-medium">{e.party}</div>
                        <div class="truncate text-[10.5px] text-[#94a3b8]">{e.memo}</div>
                    </div>
                </div>
                <div class="flex justify-center">
                    <span class="grid h-5 min-w-5 place-items-center rounded-full px-1 text-[8px] font-bold" style=format!("background:{src_bg};color:{src_fg}")>{src}</span>
                </div>
                <div class="flex min-w-0 items-center gap-1.5 px-2">
                    <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-[#94a3b8]"></span>
                    <span class="truncate text-[12px] text-[#475569]">{e.category}</span>
                </div>
                <div class="px-2 text-right text-[12.5px] font-semibold tabular-nums">{e.debit}</div>
                <div class="px-2 text-right text-[12.5px] font-semibold tabular-nums text-[#16a34a]">{e.credit}</div>
            </div>
            // Narrow: Buwiz's card row.
            <div class="flex items-start gap-2.5 px-3 py-2.5 @lg:hidden">
                {avatar("mt-0.5 h-7 w-7")}
                <div class="min-w-0 flex-1">
                    <div class="truncate text-[13px] font-semibold">{e.party}</div>
                    <div class="truncate text-[11.5px] text-[#64748b]">{format!("Oct {}, 2026 · {}", e.day, e.category)}</div>
                </div>
                <div class="flex shrink-0 items-baseline gap-1 whitespace-nowrap">
                    <span class="text-[10.5px] font-medium text-[#94a3b8]">{amount.0}</span>
                    <span class=format!("text-[13px] font-semibold tabular-nums {}", amount.2)>{amount.1}</span>
                </div>
            </div>
        </div>
    }
}

fn buwiz_app(s: Screens) -> impl IntoView {
    let nav = [
        ("home", "Home"),
        ("inbox", "Inbox"),
        ("menu_book", "Ledger"),
        ("receipt_long", "Invoices"),
        ("request_quote", "Bills"),
        ("bar_chart", "Reports"),
        ("account_balance", "Taxes"),
    ];
    chrome(
        view! { <span class="grid h-3.5 w-3.5 shrink-0 place-items-center rounded-[3px] bg-[#22c55e] text-[9px] font-bold leading-none text-white">"B"</span> },
        "Ledger · Buwiz Books",
        "books.buwiz.com",
        "/ledger",
        "@container bg-white text-[#1e293b]",
        "font-family:Inter,system-ui,-apple-system,sans-serif;",
        view! {
            <div class="flex h-full">
                <aside class="hidden w-[150px] shrink-0 flex-col gap-0.5 border-r border-[#e2e8f0] bg-[#f8fafc] p-2 text-[12px] @2xl:flex">
                    <div class="mb-2 flex items-center gap-2 px-1.5 pt-1">
                        <span class="grid h-6 w-6 place-items-center rounded-md bg-[#22c55e] text-[12px] font-bold text-white">"B"</span>
                        <span class="font-semibold">"Buwiz Books"</span>
                    </div>
                    <div class="mb-2 truncate rounded-md border border-[#e2e8f0] bg-white px-2 py-1 text-[11px] text-[#64748b]">"Goldcoders Corp"</div>
                    {nav.iter().map(|&(icon, label)| view! {
                        <div class=if label == "Ledger" {
                            "flex items-center gap-2 rounded-md bg-[#e2e8f0] px-2 py-1.5 font-medium text-[#1e293b]"
                        } else {
                            "flex items-center gap-2 rounded-md px-2 py-1.5 text-[#64748b]"
                        }>
                            <span class="material-symbols-outlined text-[15px]">{icon}</span>
                            {label}
                        </div>
                    }).collect_view()}
                </aside>
                <div class="relative flex min-w-0 flex-1 flex-col">
                    <div class="flex shrink-0 items-center gap-2 bg-gradient-to-r from-[#1a6b3c] to-[#27ae60] px-3 py-2 text-white">
                        <span class="material-symbols-outlined text-[16px] text-white/70">"view_sidebar"</span>
                        <span class="text-[14px] font-semibold">"Ledger"</span>
                        <div class="flex flex-1 justify-center">
                            <div class="flex items-center rounded-full bg-white/10 p-0.5 text-[11.5px] font-medium">
                                <span class="rounded-full bg-white/20 px-2.5 py-0.5">"View"</span>
                                <span class="px-2.5 py-0.5 text-white/55">"Edit"</span>
                            </div>
                        </div>
                        <span class="material-symbols-outlined text-[16px] text-white/60">"open_in_full"</span>
                    </div>
                    <div class="flex shrink-0 items-center gap-2 border-b border-[#e2e8f0] px-3 py-2">
                        <div class="flex min-w-0 flex-1 items-center gap-1.5 rounded-md border border-[#e2e8f0] px-2 py-1 text-[11.5px] text-[#94a3b8]">
                            <span class="material-symbols-outlined text-[14px]">"search"</span>
                            <span class="truncate">"Search transactions"</span>
                        </div>
                        <span class="shrink-0 rounded-md border border-[#e2e8f0] px-2 py-1 text-[11.5px] text-[#475569]">"Oct 2026"</span>
                    </div>
                    <div class=format!("hidden shrink-0 border-b border-[#e5e7eb] bg-[#f8fafc] text-[9.5px] font-medium uppercase tracking-wider text-[#94a3b8] {LEDGER_COLS} @lg:grid")>
                        <div class="py-1 text-center">"Date"</div>
                        <div class="px-2 py-1">"Party"</div>
                        <div class="py-1 text-center">"Src"</div>
                        <div class="px-2 py-1">"Category"</div>
                        <div class="px-2 py-1 text-right">"Debit"</div>
                        <div class="px-2 py-1 text-right">"Credit"</div>
                    </div>
                    <div class="min-h-0 flex-1 overflow-hidden">
                        {JE.iter().enumerate().map(|(i, e)| view! { {move || (s.posted.get() > i).then(|| ledger_row(e, true))} }).collect_view()}
                        {BEFORE.iter().map(|e| ledger_row(e, false)).collect_view()}
                    </div>
                    {move || (s.posted.get() >= JE.len()).then(|| view! {
                        <div class="gc-card-in absolute bottom-3 right-3 flex items-center gap-2 rounded-lg bg-[#0f172a] px-3 py-2 text-[12px] text-white shadow-lg">
                            <span class="text-[#22c55e]">"✓"</span>
                            "JE-0412 posted · balanced"
                        </div>
                    })}
                </div>
            </div>
        },
    )
}
