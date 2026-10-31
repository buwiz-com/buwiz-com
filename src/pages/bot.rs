//! The Buwiz Bot page, at the root of the Bot site once the site is split
//! (see [`crate::canonical::Site`]). One short hero and nothing to
//! download until the Bot has a signed release. Styles are in
//! `src/home.css`.

use super::*;

pub(super) const HEAD: Head = Head {
    title: "Buwiz Bot",
    description: "Buwiz Bot is a native Mac assistant that runs your bots, your forms and your books in one fast app. Coming soon for Macs with Apple silicon.",
    index: true,
};

/// `forms_origin` is the filing site's, for its link.
#[component]
pub(super) async fn bot_page(forms_origin: String) -> Result<impl View> {
    Ok(view! {
        <main class="home landing">
            <section class="hero-wrap bot-hero">
                <div class="hero-glow" aria-hidden="true"></div>
                <div class="bot-copy">
                    <h1>"Buwiz Bot"</h1>
                    <p class="hero-lead">
                        "A native Mac assistant that runs your bots, your forms and your books in one fast app."
                    </p>
                    <p class="bot-soon">"Coming soon · Mac with Apple silicon"</p>
                    <p class="bot-also">
                        "Also from Buwiz: "
                        <a href=(forms_origin)>"eBIRForms"</a>
                        " · Buwiz Books"
                    </p>
                </div>
            </section>
        </main>
    })
}
