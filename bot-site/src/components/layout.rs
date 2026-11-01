use crate::app::ThemeContext;
use crate::bot_meta::{BotMeta, REPO_URL};
use crate::components::AgentField;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// True when the current path matches this nav href.
fn path_is_active(pathname: &str, href: &str) -> bool {
    let path = pathname.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    if href == "/" { path == "/" } else { path == href || path.starts_with(&format!("{href}/")) }
}

/// Desktop / mobile nav link with active + hover underline (theme tokens).
#[component]
fn NavItem(
    href: &'static str,
    label: &'static str,
    /// Extra classes for size/weight (desktop vs mobile).
    #[prop(into)]
    base_class: String,
    #[prop(optional)] on_click: Option<Callback<ev::MouseEvent>>,
) -> impl IntoView {
    let location = use_location();
    let href_owned = href.to_string();

    let class = move || {
        let active = path_is_active(&location.pathname.get(), &href_owned);
        // underline via border-b so light/dark both use gc-brand / gc-text tokens
        let state = if active {
            "text-gc-brand border-gc-brand"
        } else {
            "text-gc-text-muted border-transparent hover:text-gc-brand hover:border-gc-brand"
        };
        format!(
            "{base} border-b-2 pb-1 transition-colors duration-200 cursor-pointer active:opacity-80 {state}",
            base = base_class,
            state = state
        )
    };

    view! {
        <a
            href=href
            class=class
            aria-current=move || {
                path_is_active(&location.pathname.get(), href).then_some("page")
            }
            on:click=move |ev| {
                if let Some(cb) = on_click {
                    cb.run(ev);
                }
            }
        >
            {label}
        </a>
    }
}

/// The particle field and the mouse glow, fixed behind every page: one
/// viewport-sized canvas, so it costs the same however long the page is.
#[component]
fn Backdrop() -> impl IntoView {
    let layer = NodeRef::<html::Div>::new();

    Effect::new(move |_| {
        let Some(el) = layer.get() else {
            return;
        };
        let node: web_sys::HtmlElement = el.clone().into();
        let handle = window_event_listener(ev::mousemove, move |evt| {
            let Some(window) = web_sys::window() else {
                return;
            };
            let width = window.inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(1.0).max(1.0);
            let height = window.inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(1.0).max(1.0);
            let x = evt.client_x() as f64 / width * 100.0;
            let y = evt.client_y() as f64 / height * 100.0;
            let _ = node.style().set_property("--mx", &format!("{x:.1}%"));
            let _ = node.style().set_property("--my", &format!("{y:.1}%"));
        });
        on_cleanup(move || drop(handle));
    });

    view! {
        <div node_ref=layer class="fixed inset-0 z-0 pointer-events-none gc-spot" aria-hidden="true">
            <AgentField />
        </div>
    }
}

#[component]
pub fn Layout(children: Children) -> impl IntoView {
    let stars = use_context::<BotMeta>().unwrap_or_default().star_label();
    let (is_menu_open, set_is_menu_open) = signal(false);

    let theme_context = use_context::<ThemeContext>();

    let toggle_theme = move |_| {
        if let Some(ctx) = theme_context {
            ctx.0.update(|theme| {
                *theme = if *theme == "dark" { String::from("light") } else { String::from("dark") };
            });
        }
    };

    let toggle_menu = move |_| {
        set_is_menu_open.update(|v| *v = !*v);
    };

    // Close menu helper for mobile links
    let close_menu = Callback::new(move |_: ev::MouseEvent| {
        set_is_menu_open.set(false);
    });

    let is_dark = move || {
        if let Some(ctx) = theme_context { ctx.0.get() == "dark" } else { true }
    };

    view! {
        <div class="min-h-screen bg-gc-bg text-gc-text font-sans selection:bg-gc-brand/20 selection:text-gc-brand flex flex-col">
            <Backdrop />
            <header class="bg-gc-bg-elevated/95 backdrop-blur-md border-b border-gc-border sticky top-0 z-50">
                <div class="flex justify-between items-center w-full px-6 md:px-8 h-16 md:h-20 max-w-[1440px] mx-auto">
                    // Logo
                    <div class="flex items-center gap-2 md:gap-4 relative z-50">
                        <a href="/" aria-label="Buwiz home" class="flex items-center gap-3 cursor-pointer active:opacity-80 transition-all duration-200">
                            <span class="buwiz-wordmark h-7 md:h-8" role="img" aria-label="Buwiz"></span>
                        </a>
                    </div>

                    // Desktop Navigation
                    <nav class="hidden md:flex items-center gap-8" aria-label="Primary">
                        <NavItem href="/" label="Home" base_class="text-xs tracking-widest uppercase font-semibold" />
                        <NavItem href="/mini-apps" label="Mini apps" base_class="text-xs tracking-widest uppercase font-semibold" />
                    </nav>

                    // Desktop CTA & Controls
                    <div class="hidden md:flex items-center gap-4">
                        <button on:click=toggle_theme class="p-2 rounded-full hover:bg-gc-bg-hover text-gc-text-muted hover:text-gc-brand transition-colors" aria-label="Toggle Theme">
                            <span class="material-symbols-outlined text-[20px]">
                                {move || if is_dark() { "light_mode" } else { "dark_mode" }}
                            </span>
                        </button>
                    </div>

                    // Mobile controls
                    <div class="md:hidden flex items-center gap-4 relative z-50">
                        <button on:click=toggle_theme class="p-2 rounded-full hover:bg-gc-bg-hover text-gc-text-muted transition-colors" aria-label="Toggle Theme">
                            <span class="material-symbols-outlined text-[20px]">
                                {move || if is_dark() { "light_mode" } else { "dark_mode" }}
                            </span>
                        </button>

                        // Mobile hamburger button
                        <button
                            on:click=toggle_menu
                            class="flex flex-col justify-center items-center w-10 h-10 space-y-1.5 focus:outline-none -mr-1"
                            aria-label="Toggle navigation menu"
                        >
                            <span class=move || format!("w-6 h-[2px] bg-gc-text transform transition duration-300 ease-in-out origin-center {}", if is_menu_open.get() { "rotate-45 translate-y-[8px]" } else { "" })></span>
                            <span class=move || format!("w-6 h-[2px] bg-gc-text transition duration-300 ease-in-out {}", if is_menu_open.get() { "opacity-0 scale-0" } else { "opacity-100 scale-100" })></span>
                            <span class=move || format!("w-6 h-[2px] bg-gc-text transform transition duration-300 ease-in-out origin-center {}", if is_menu_open.get() { "-rotate-45 -translate-y-[8px]" } else { "" })></span>
                        </button>
                    </div>
                </div>

            </header>

            // Mobile fullscreen overlay menu (moved outside header to avoid backdrop-blur containing block)
            <div class=move || format!("fixed inset-0 bg-gc-bg z-40 transform transition-transform duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] flex flex-col justify-center items-center md:hidden pt-16 {}", if is_menu_open.get() { "translate-x-0" } else { "translate-x-full" })>
                <nav class="flex flex-col items-center gap-8 mb-12" aria-label="Mobile primary">
                    <NavItem
                        href="/"
                        label="Home"
                        base_class="text-3xl tracking-tighter uppercase font-black"
                        on_click=close_menu
                    />
                    <NavItem
                        href="/mini-apps"
                        label="Mini apps"
                        base_class="text-3xl tracking-tighter uppercase font-black"
                        on_click=close_menu
                    />
                </nav>
            </div>

            <main class="relative z-10 flex-grow w-full">
                {children()}
            </main>

            // Two short rows: what Buwiz is and where to go, then the
            // copyright and the licence. index.html carries the same facts as
            // JSON-LD for crawlers that do not run the app.
            <footer class="relative z-10 w-full border-t border-gc-border bg-gc-bg-elevated/70 backdrop-blur-sm">
                <div class="flex flex-col gap-4 px-6 md:px-8 py-8 max-w-[1440px] mx-auto text-sm">
                    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                        <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-4">
                            <a href="/" aria-label="Buwiz home" class="w-fit">
                                <span class="buwiz-wordmark h-6" role="img" aria-label="Buwiz"></span>
                            </a>
                            <p class="text-gc-text-muted">"Free, open-source AI coworkers."</p>
                        </div>
                        <nav aria-label="Footer" class="flex items-center gap-6">
                            <a href="/mini-apps" class="text-gc-text-muted hover:text-gc-brand transition-colors">"Mini apps"</a>
                            <a href=REPO_URL rel="noopener" class="inline-flex items-center gap-2 text-gc-text-muted hover:text-gc-brand transition-colors">
                                "GitHub"
                                {stars.map(|stars| view! { <span class="text-gc-text-faint tabular-nums">{stars}</span> })}
                            </a>
                        </nav>
                    </div>
                    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between text-xs">
                        <p class="text-gc-text-faint">"© 2026 Buwiz · Made in the Philippines"</p>
                        <span class="w-fit rounded-full border border-[#3fb58f]/30 bg-[#3fb58f]/10 px-2.5 py-0.5 font-semibold text-[#3fb58f]">
                            "Free and open source · Apache-2.0"
                        </span>
                    </div>
                </div>
            </footer>
        </div>
    }
}
