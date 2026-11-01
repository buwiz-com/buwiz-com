use crate::app::ThemeContext;
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

            <main class="flex-grow w-full">
                {children()}
            </main>

            // One short line on what Buwiz is, links to real pages only (each
            // named for what it is, so the anchor text says it), and the
            // copyright. index.html carries the same facts as JSON-LD and as
            // plain links for crawlers that do not run the app.
            <footer class="bg-gc-bg-elevated w-full border-t border-gc-border">
                <div class="flex flex-col gap-8 px-6 md:px-8 py-12 max-w-[1440px] mx-auto">
                    <div class="flex flex-col md:flex-row md:items-start md:justify-between gap-8">
                        <div class="flex flex-col gap-4 max-w-md">
                            <a href="/" aria-label="Buwiz home" class="w-fit">
                                <span class="buwiz-wordmark h-7" role="img" aria-label="Buwiz"></span>
                            </a>
                            <p class="text-sm text-gc-text-muted leading-relaxed">
                                "Buwiz Bot is an AI coworker for your desktop. Its mini apps open right in the chat, for you and your AI agent: eBIRForms to prepare and file BIR tax returns, Books for bookkeeping, and Word and Spreadsheet for documents."
                            </p>
                        </div>
                        <nav aria-label="Footer">
                            <ul class="flex flex-col gap-3 text-sm">
                                <li><a href="/mini-apps" class="text-gc-text-muted hover:text-gc-brand transition-colors">"Mini apps"</a></li>
                                <li><a href="/download" class="text-gc-text-muted hover:text-gc-brand transition-colors">"Download for Mac, Windows and Linux"</a></li>
                                <li><a href="https://ebirforms.buwiz.com/" class="text-gc-text-muted hover:text-gc-brand transition-colors">"eBIRForms: BIR tax filing"</a></li>
                                <li><a href="https://account.buwiz.com/" class="text-gc-text-muted hover:text-gc-brand transition-colors">"Buwiz account"</a></li>
                                <li><a href="https://github.com/buwiz-bot/buwiz-bot" rel="noopener" class="text-gc-text-muted hover:text-gc-brand transition-colors">"Source code on GitHub"</a></li>
                            </ul>
                        </nav>
                    </div>
                    <p class="pt-6 border-t border-gc-border text-xs text-gc-text-faint">
                        "© 2026 Buwiz · Made in the Philippines"
                    </p>
                </div>
            </footer>
        </div>
    }
}
