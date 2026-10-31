use gloo_timers::callback::Interval;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    ox: f32,
    oy: f32,
}

struct Packet {
    a: usize,
    b: usize,
    t: f32,
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0
    }

    fn f32(&mut self) -> f32 {
        (self.next() >> 8) as f32 / 16_777_216.0
    }
}

fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten())
        .map(|mq| mq.matches())
        .unwrap_or(false)
}

fn brand_rgb() -> (u8, u8, u8) {
    let hex = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| w_computed(&el));
    parse_hex(hex.as_deref().unwrap_or("#3b82f6"))
}

fn w_computed(el: &web_sys::Element) -> Option<String> {
    let window = web_sys::window()?;
    let style = window.get_computed_style(el).ok().flatten()?;
    let value = style.get_property_value("--gc-brand").ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_hex(input: &str) -> (u8, u8, u8) {
    let h = input.trim().trim_start_matches('#');
    if h.len() >= 6 {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(59);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(130);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(246);
        (r, g, b)
    } else {
        (59, 130, 246)
    }
}

/// Mouse-reactive agent graph. Pointer position is stored off the Leptos
/// render tree so the animation frame never retriggers the component.
#[component]
pub fn AgentField() -> impl IntoView {
    let canvas_ref = NodeRef::<html::Canvas>::new();

    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        if prefers_reduced_motion() {
            paint_static(&canvas);
            return;
        }
        start_field(canvas);
    });

    view! {
        <canvas
            node_ref=canvas_ref
            class="absolute inset-0 w-full h-full pointer-events-none"
            aria-hidden="true"
        ></canvas>
    }
}

fn paint_static(canvas: &web_sys::HtmlCanvasElement) {
    let Some(ctx) = context(canvas) else {
        return;
    };
    let (w, h, dpr) = size_canvas(canvas);
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.clear_rect(0.0, 0.0, w as f64, h as f64);
    let (r, g, b) = brand_rgb();
    let mut rng = Rng(0xC0FFEE);
    for _ in 0..48 {
        let x = rng.f32() * w;
        let y = rng.f32() * h;
        ctx.set_fill_style_str(&format!("rgba({r},{g},{b},0.28)"));
        ctx.begin_path();
        let _ = ctx.arc(x as f64, y as f64, 1.6, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }
}

fn start_field(canvas: web_sys::HtmlCanvasElement) {
    let Some(ctx) = context(&canvas) else {
        return;
    };

    let mouse = Rc::new(Cell::new((-9999.0f32, -9999.0f32)));
    let particles = Rc::new(RefCell::new(Vec::<Particle>::new()));
    let packets = Rc::new(RefCell::new(Vec::<Packet>::new()));
    let rng = Rc::new(RefCell::new(Rng(0xA11CE)));

    rebuild(&canvas, &particles, &rng);
    step(&canvas, &ctx, &mouse, &particles, &packets, &rng);

    let mouse_move = SendWrapper::new({
        let mouse = mouse.clone();
        let canvas = canvas.clone();
        window_event_listener(ev::mousemove, move |evt| {
            let rect = canvas.get_bounding_client_rect();
            mouse.set((
                evt.client_x() as f32 - rect.left() as f32,
                evt.client_y() as f32 - rect.top() as f32,
            ));
        })
    });

    let mouse_leave = SendWrapper::new({
        let mouse = mouse.clone();
        window_event_listener(ev::mouseleave, move |_| {
            mouse.set((-9999.0, -9999.0));
        })
    });

    let on_resize = SendWrapper::new({
        let canvas = canvas.clone();
        let particles = particles.clone();
        let rng = rng.clone();
        window_event_listener(ev::resize, move |_| {
            rebuild(&canvas, &particles, &rng);
        })
    });

    let ctx = Rc::new(ctx);
    let interval = SendWrapper::new(Interval::new(16, {
        let canvas = canvas.clone();
        let ctx = ctx.clone();
        let mouse = mouse.clone();
        let particles = particles.clone();
        let packets = packets.clone();
        let rng = rng.clone();
        move || {
            step(&canvas, &ctx, &mouse, &particles, &packets, &rng);
        }
    }));

    on_cleanup(move || {
        drop(interval);
        drop(mouse_move);
        drop(mouse_leave);
        drop(on_resize);
    });
}

fn context(canvas: &web_sys::HtmlCanvasElement) -> Option<web_sys::CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|obj| obj.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
}

fn size_canvas(canvas: &web_sys::HtmlCanvasElement) -> (f32, f32, f64) {
    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio().clamp(1.0, 2.0))
        .unwrap_or(1.0);
    let w = canvas.client_width().max(1) as f32;
    let h = canvas.client_height().max(1) as f32;
    canvas.set_width((w as f64 * dpr) as u32);
    canvas.set_height((h as f64 * dpr) as u32);
    (w, h, dpr)
}

fn rebuild(
    canvas: &web_sys::HtmlCanvasElement,
    particles: &Rc<RefCell<Vec<Particle>>>,
    rng: &Rc<RefCell<Rng>>,
) {
    let (w, h, _) = size_canvas(canvas);
    let cols = ((w / 88.0).round() as usize).clamp(6, 14);
    let rows = ((h / 88.0).round() as usize).clamp(4, 10);
    let mut rng = rng.borrow_mut();
    let mut next = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let ox = (col as f32 + 0.5) * (w / cols as f32) + (rng.f32() - 0.5) * 18.0;
            let oy = (row as f32 + 0.5) * (h / rows as f32) + (rng.f32() - 0.5) * 18.0;
            next.push(Particle {
                x: ox,
                y: oy,
                vx: 0.0,
                vy: 0.0,
                ox,
                oy,
            });
        }
    }
    *particles.borrow_mut() = next;
}

fn step(
    canvas: &web_sys::HtmlCanvasElement,
    ctx: &web_sys::CanvasRenderingContext2d,
    mouse: &Rc<Cell<(f32, f32)>>,
    particles: &Rc<RefCell<Vec<Particle>>>,
    packets: &Rc<RefCell<Vec<Packet>>>,
    rng: &Rc<RefCell<Rng>>,
) {
    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio().clamp(1.0, 2.0))
        .unwrap_or(1.0);
    let w = canvas.client_width().max(1) as f32;
    let h = canvas.client_height().max(1) as f32;
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.clear_rect(0.0, 0.0, w as f64, h as f64);

    let (mx, my) = mouse.get();
    let (br, bg, bb) = brand_rgb();

    {
        let mut pts = particles.borrow_mut();
        for p in pts.iter_mut() {
            let dxm = mx - p.x;
            let dym = my - p.y;
            let dist2 = dxm * dxm + dym * dym;
            if dist2 < 22_000.0 && dist2 > 1.0 {
                let dist = dist2.sqrt();
                let force = (148.0 - dist) * 0.00055;
                p.vx += dxm / dist * force;
                p.vy += dym / dist * force;
            }
            p.vx += (p.ox - p.x) * 0.018;
            p.vy += (p.oy - p.y) * 0.018;
            p.vx *= 0.86;
            p.vy *= 0.86;
            p.x += p.vx;
            p.y += p.vy;
        }
    }

    let pts = particles.borrow();
    let link_r = 118.0;
    let link_r2 = link_r * link_r;

    for i in 0..pts.len() {
        for j in (i + 1)..pts.len() {
            let dx = pts[i].x - pts[j].x;
            let dy = pts[i].y - pts[j].y;
            let d2 = dx * dx + dy * dy;
            if d2 > link_r2 {
                continue;
            }
            let alpha = (1.0 - d2.sqrt() / link_r) * 0.42;
            ctx.set_stroke_style_str(&format!("rgba({br},{bg},{bb},{alpha:.3})"));
            ctx.set_line_width(1.0);
            ctx.begin_path();
            ctx.move_to(pts[i].x as f64, pts[i].y as f64);
            ctx.line_to(pts[j].x as f64, pts[j].y as f64);
            ctx.stroke();
        }
    }

    {
        let mut bags = packets.borrow_mut();
        if bags.len() < 6 {
            let mut rng = rng.borrow_mut();
            if rng.f32() < 0.08 && pts.len() > 4 {
                let a = (rng.next() as usize) % pts.len();
                let mut b = (rng.next() as usize) % pts.len();
                if b == a {
                    b = (a + 1) % pts.len();
                }
                bags.push(Packet { a, b, t: 0.0 });
            }
        }
        bags.retain_mut(|pkt| {
            pkt.t += 0.016;
            pkt.t < 1.0
        });
        for pkt in bags.iter() {
            if pkt.a >= pts.len() || pkt.b >= pts.len() {
                continue;
            }
            let x = pts[pkt.a].x + (pts[pkt.b].x - pts[pkt.a].x) * pkt.t;
            let y = pts[pkt.a].y + (pts[pkt.b].y - pts[pkt.a].y) * pkt.t;
            ctx.set_fill_style_str(&format!("rgba({br},{bg},{bb},0.9)"));
            ctx.begin_path();
            let _ = ctx.arc(x as f64, y as f64, 2.2, 0.0, std::f64::consts::TAU);
            ctx.fill();
        }
    }

    for p in pts.iter() {
        let dxm = mx - p.x;
        let dym = my - p.y;
        let hot = dxm * dxm + dym * dym < 12_000.0;
        let alpha = if hot { 0.95 } else { 0.55 };
        let radius = if hot { 2.8 } else { 1.9 };
        ctx.set_fill_style_str(&format!("rgba({br},{bg},{bb},{alpha})"));
        ctx.begin_path();
        let _ = ctx.arc(p.x as f64, p.y as f64, radius, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }
}
