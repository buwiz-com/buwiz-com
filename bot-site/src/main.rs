use buwiz_bot_site::App;
use leptos::prelude::*;

fn main() {
    // set up logging
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();

    // The server sends each page already drawn, for crawlers that do not
    // run the app (bot-site/prerender.py). The app draws it again itself.
    if let Some(prerendered) = document().get_element_by_id("prerender") {
        prerendered.remove();
    }

    mount_to_body(|| {
        view! {
            <App />
        }
    })
}
