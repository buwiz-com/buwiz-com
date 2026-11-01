use crate::components::layout::Layout;
use crate::sections::{AgentCompose, AgentTemplates, Allowly, DataSwap, Egress, HomeHero, MiniApps, SecureVault};
use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use leptos_meta::*;

fn scroll_hash_into_view() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(hash) = window.location().hash() else {
        return;
    };
    let id = hash.trim_start_matches('#');
    if id.is_empty() {
        return;
    };
    if let Some(el) = window.document().and_then(|d| d.get_element_by_id(id)) {
        el.scroll_into_view();
    }
}

#[component]
pub fn Home() -> impl IntoView {
    Effect::new(move |_| {
        Timeout::new(80, scroll_hash_into_view).forget();
    });

    view! {
        <Layout>
            <Title text="Buwiz Bot: AI coworkers that work on a computer you host" />
            <Meta
                name="description"
                content="Buwiz Bot is free and open source. Give your AI a job and a computer you host. Mini apps in the chat file BIR returns, keep the books and edit documents."
            />

            <div class="relative min-h-screen">
                <div class="gc-grain" aria-hidden="true"></div>
                <HomeHero />
                <AgentCompose />
                <AgentTemplates />
                <MiniApps />
                <SecureVault />
                <DataSwap />
                <Allowly />
                <Egress />
            </div>
        </Layout>
    }
}
