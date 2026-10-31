use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::{components::*, path};

use crate::pages::home::Home;

/// Where the bundle's own files are served from (Trunk `public_url`).
pub const ASSETS: &str = "/bot/assets";

/// The `content` of `<meta name=...>` in index.html. The server fills in
/// the origins (`buwiz-account-origin`, `buwiz-forms-origin`); a plain
/// `trunk serve` keeps the production values written there.
pub fn meta_content(name: &str) -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector(&format!("meta[name=\"{name}\"]")).ok().flatten())
        .and_then(|el| el.get_attribute("content"))
        .unwrap_or_default()
}

#[derive(Copy, Clone)]
pub struct ThemeContext(pub RwSignal<String>);

/// Read a cookie value by name from document.cookie
fn get_cookie(name: &str) -> Option<String> {
    use wasm_bindgen::JsCast;
    let document = web_sys::window()?
        .document()?
        .dyn_into::<web_sys::HtmlDocument>()
        .ok()?;
    let cookies = document.cookie().ok()?;
    for part in cookies.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{}=", name)) {
            return Some(value.to_string());
        }
    }
    None
}

/// Set a cookie with proper domain handling for both prod and local dev
fn set_theme_cookie(value: &str) {
    use wasm_bindgen::JsCast;
    if let Some(document) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.dyn_into::<web_sys::HtmlDocument>().ok())
    {
        let hostname = web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .unwrap_or_default();

        let domain = if hostname.ends_with("goldcoders.dev") {
            "; domain=.goldcoders.dev"
        } else if hostname.ends_with(".localhost") {
            "; domain=localhost"
        } else {
            ""
        };

        let cookie = format!(
            "gc-theme={}; path=/; max-age=31536000; SameSite=Lax{}",
            value, domain
        );
        let _ = document.set_cookie(&cookie);
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let theme = RwSignal::new(String::from("dark"));
    provide_context(ThemeContext(theme));

    // Client-side cookie initialization and localStorage migration
    #[cfg(target_arch = "wasm32")]
    {
        // 1. Try reading from cookie first
        if let Some(cookie_theme) = get_cookie("gc-theme") {
            theme.set(cookie_theme);
        } else if let Some(window) = web_sys::window() {
            // 2. Migrate from localStorage if cookie doesn't exist yet
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(stored_theme)) = storage.get_item("gc-theme") {
                    theme.set(stored_theme.clone());
                    set_theme_cookie(&stored_theme);
                    let _ = storage.remove_item("gc-theme");
                }
            }
        }

        // Persist theme changes to cookie
        Effect::new(move |_| {
            let current_theme = theme.get();
            set_theme_cookie(&current_theme);
        });
    }

    view! {
        <Html attr:lang="en" attr:dir="ltr" attr:data-theme=move || theme.get() attr:class="antialiased" />
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />

        <Router>
            <Routes fallback=|| view! {
                <div class="min-h-screen flex items-center justify-center bg-zinc-50 dark:bg-zinc-950 text-zinc-900 dark:text-zinc-100">
                    <h1 class="text-2xl font-medium tracking-tight">"404 - Not Found"</h1>
                </div>
            }>
                <Route path=path!("/") view=Home />
            </Routes>
        </Router>
    }
}
