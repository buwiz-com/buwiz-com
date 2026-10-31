use crate::components::{AgentField, HeroFilm};
use leptos::html;
use leptos::prelude::*;

/// First section: Meet Buwiz Bot copy, then the self-playing film in the agent window.
#[component]
pub fn HomeHero() -> impl IntoView {
    let stage = NodeRef::<html::Header>::new();

    Effect::new(move |_| {
        let Some(el) = stage.get() else {
            return;
        };
        let node = el.clone();
        let handle = window_event_listener(leptos::ev::mousemove, move |evt| {
            let rect = node.get_bounding_client_rect();
            let x = ((evt.client_x() as f64 - rect.left()) / rect.width().max(1.0)) * 100.0;
            let y = ((evt.client_y() as f64 - rect.top()) / rect.height().max(1.0)) * 100.0;
            let _ = node.style().set_property("--mx", &format!("{x:.1}%"));
            let _ = node.style().set_property("--my", &format!("{y:.1}%"));
        });
        on_cleanup(move || drop(handle));
    });

    view! {
        <header
            node_ref=stage
            class="relative min-h-[100dvh] block pt-20 md:pt-24 pb-8 gc-spot"
        >
            <AgentField />
            <div class="absolute inset-0 pointer-events-none bg-gradient-to-b from-transparent via-transparent to-gc-bg/80"></div>

            <div class="relative z-10 w-full mx-auto px-4 md:px-8">
                <div class="max-w-3xl mx-auto mb-8 md:mb-10 text-center">
                    <h1 class="text-4xl md:text-6xl font-black tracking-tighter leading-[1.08] text-gc-text mb-4">
                        "Meet Buwiz Bot"
                    </h1>
                    <p class="text-lg md:text-2xl font-semibold text-gc-text mb-3 max-w-2xl mx-auto">
                        "Give your AI a job. Give it a computer. Let it work."
                    </p>
                    <p class="text-base md:text-lg text-gc-text-muted mb-3 max-w-2xl mx-auto">
                        "Specialized coworkers with skills, tools, knowledge, connectors, and a computer you host."
                    </p>
                    <p class="text-base md:text-lg font-semibold text-gc-text mb-6">
                        "No lock-in. Just AI coworkers you control."
                    </p>
                    <div class="flex flex-col sm:flex-row items-center justify-center gap-3">
                        <a
                            href="#models"
                            class="w-full sm:w-auto bg-gc-brand text-white px-7 py-3.5 rounded-md text-sm font-bold tracking-wide hover:bg-gc-brand-hover active:scale-[0.98] transition-all text-center"
                        >
                            "See the stack"
                        </a>
                        <a
                            href="https://calendly.com/goldcoders-support/30min"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="w-full sm:w-auto px-7 py-3.5 rounded-md text-sm font-semibold text-gc-text border border-gc-border bg-gc-bg-card/80 hover:border-gc-brand/40 active:scale-[0.98] transition-all text-center"
                        >
                            "Contact sales"
                        </a>
                    </div>
                </div>

                <div id="models" class="max-w-[1080px] mx-auto mt-16 md:mt-24 mb-8 md:mb-10 scroll-mt-24">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Use the best model for the job."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[60ch]">
                        "No single model is best at everything. One paints, one writes code, one reads what it sees. Your coworker switches mid-thread, on your own plan or an API key, and keeps the same chat, files and computer. Each reply says which one answered and who paid."
                    </p>
                </div>

                <HeroFilm />
            </div>
        </header>
    }
}
