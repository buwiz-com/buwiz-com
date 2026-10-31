use crate::components::layout::Layout;
use crate::sections::{
    AgentCompose, AgentTemplates, Allowly, DataSwap, Egress, HomeHero, MiniApps, SecureVault,
};
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
            <Title text="Meet Buwiz Bot | Buwiz" />
            <Meta
                name="description"
                content="Give your AI a job. Give it a computer. Let it work. Specialized coworkers with skills, tools, knowledge, connectors, and a computer you host. No lock-in."
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
