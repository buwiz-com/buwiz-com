use crate::app::{ThemeContext, meta_content};
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// True when the current path matches this nav href.
fn path_is_active(pathname: &str, href: &str) -> bool {
    let path = pathname.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    if href == "/" {
        path == "/"
    } else {
        path == href || path.starts_with(&format!("{href}/"))
    }
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

#[component]
pub fn Layout(children: Children) -> impl IntoView {
    let (is_menu_open, set_is_menu_open) = signal(false);

    let theme_context = use_context::<ThemeContext>();

    let toggle_theme = move |_| {
        if let Some(ctx) = theme_context {
            ctx.0.update(|theme| {
                *theme = if *theme == "dark" {
                    String::from("light")
                } else {
                    String::from("dark")
                };
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

    // Filled in by the server; see index.html.
    let account = meta_content("buwiz-account-origin");
    let (sign_in, sign_up) = (format!("{account}/login"), format!("{account}/signup"));
    let forms = meta_content("buwiz-forms-origin");

    let is_dark = move || {
        if let Some(ctx) = theme_context {
            ctx.0.get() == "dark"
        } else {
            true
        }
    };

    view! {
        <div class="min-h-screen bg-gc-bg text-gc-text font-sans selection:bg-gc-brand/20 selection:text-gc-brand flex flex-col">
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
                        <a href=forms.clone() class="text-xs tracking-widest uppercase font-semibold border-b-2 pb-1 transition-colors duration-200 cursor-pointer active:opacity-80 text-gc-text-muted border-transparent hover:text-gc-brand hover:border-gc-brand">
                            "eBIRForms"
                        </a>
                    </nav>

                    // Desktop CTA & Controls
                    <div class="hidden md:flex items-center gap-4">
                        <button on:click=toggle_theme class="p-2 rounded-full hover:bg-gc-bg-hover text-gc-text-muted hover:text-gc-brand transition-colors" aria-label="Toggle Theme">
                            <span class="material-symbols-outlined text-[20px]">
                                {move || if is_dark() { "light_mode" } else { "dark_mode" }}
                            </span>
                        </button>
                        <a href=sign_in.clone() class="text-gc-text-muted hover:text-gc-brand px-3 py-2.5 rounded-md text-xs tracking-widest uppercase font-semibold cursor-pointer active:opacity-80 transition-colors inline-block text-center">
                            "Sign in"
                        </a>
                        <a href=sign_up.clone() class="bg-gc-brand text-white px-6 py-2.5 rounded-md text-xs tracking-widest uppercase font-semibold cursor-pointer active:opacity-80 hover:bg-gc-brand-hover transition-all inline-block text-center">
                            "Create account"
                        </a>
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
                    <a href=forms on:click=move |_| set_is_menu_open.set(false) class="text-3xl tracking-tighter uppercase font-black border-b-2 pb-1 transition-colors duration-200 cursor-pointer active:opacity-80 text-gc-text-muted border-transparent hover:text-gc-brand hover:border-gc-brand">
                        "eBIRForms"
                    </a>
                </nav>
                // Mobile CTA
                <a href=sign_up on:click=move |_| set_is_menu_open.set(false) class="bg-gc-brand text-white px-10 py-4 rounded-md text-sm tracking-widest uppercase font-bold hover:bg-gc-brand-hover transition-all text-center">
                    "Create account"
                </a>
                // Mobile secondary link
                <div class="mt-10 flex items-center gap-6">
                    <a href=sign_in on:click=move |_| set_is_menu_open.set(false) class="text-xs tracking-widest uppercase text-gc-text-faint hover:text-gc-brand transition-colors">
                        "Sign in"
                    </a>
                </div>
            </div>

            <main class="flex-grow w-full">
                {children()}
            </main>

            <footer class="bg-gc-bg-elevated w-full border-t border-gc-border">
                <div class="grid grid-cols-1 md:grid-cols-12 gap-8 px-8 py-16 max-w-[1440px] mx-auto">
                    <div class="md:col-span-4">
                        <div class="flex items-center gap-4 mb-6">
                            <span class="buwiz-wordmark h-8" role="img" aria-label="Buwiz"></span>
                            <a href="https://goldcoders.dev" class="font-bold text-gc-text text-xl hover:text-gc-brand transition-colors">"by Goldcoders"</a>
                        </div>
                        <p class="text-xs tracking-tight text-gc-text-muted leading-relaxed max-w-xs">
                            "Goldcoders builds AI-native software, high-performance Rust systems, web apps, mobile apps, desktop apps, automation platforms, and agentic AI solutions for companies in the Philippines and worldwide."
                        </p>
                    </div>

                    <div class="md:col-span-2 md:col-start-7">
                        <div class="text-xs tracking-tight font-bold text-gc-text mb-6 uppercase">"Company"</div>
                        <ul class="space-y-3">
                            <li><a href="https://goldcoders.dev/#about" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"About"</a></li>
                            <li><a href="https://goldcoders.dev/services" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"Services"</a></li>
                            <li><a href="https://goldcoders.dev/projects" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"Projects"</a></li>
                            <li><a href="https://goldcoders.dev/techstacks" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"Tech Stack"</a></li>
                        </ul>
                    </div>

                    <div class="md:col-span-2">
                        <div class="text-xs tracking-tight font-bold text-gc-text mb-6 uppercase">"Connect"</div>
                        <ul class="space-y-3">
                            <li><a href="https://github.com/goldcoders-corp" target="_blank" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"GitHub"</a></li>
                            <li><a href="https://www.linkedin.com/company/goldcoders-corp" target="_blank" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"LinkedIn"</a></li>
                            <li><a href="https://www.facebook.com/goldcoders.corp" target="_blank" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"Facebook"</a></li>
                            <li><a href="mailto:support@goldcoders.dev" class="text-xs tracking-tight text-gc-text-muted hover:text-gc-brand transition-all duration-200">"Email"</a></li>
                        </ul>
                    </div>

                    <div class="md:col-span-2">
                        <div class="text-xs tracking-tight font-bold text-gc-text mb-6 uppercase">"Availability"</div>
                        <div class="flex items-center gap-2 mb-2">
                            <span class="w-2 h-2 rounded-full bg-gc-accent animate-pulse"></span>
                            <p class="font-mono text-[10px] text-gc-text-muted uppercase tracking-widest">"Systems Nominal"</p>
                        </div>
                    </div>

                    <div class="md:col-span-12 pt-8 mt-8 border-t border-gc-border flex flex-col md:flex-row justify-between items-center gap-4">
                        <p class="text-xs tracking-tight text-gc-text-muted">
                            "© 2026 Buwiz Bot by Goldcoders. AI-Native Software, Rust Systems, and Agentic Automation."
                        </p>
                        <div class="flex gap-6">
                            <span class="font-mono text-[10px] uppercase tracking-tighter text-gc-text-faint">"MANILA / SINGAPORE / DISTRIBUTED"</span>
                        </div>
                    </div>
                </div>
            </footer>
        </div>
    }
}
