use leptos::prelude::*;

const CALENDLY: &str = "https://calendly.com/goldcoders-support/30min";

#[component]
pub fn HomeClose() -> impl IntoView {
    view! {
        <section class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card overflow-hidden">
            <div class="absolute inset-0 pointer-events-none bg-[radial-gradient(ellipse_at_bottom,_var(--tw-gradient-stops))] from-gc-brand/10 via-transparent to-transparent"></div>
            <div class="relative max-w-[1440px] mx-auto px-6 md:px-12 text-center flex flex-col items-center">
                <h2 class="text-3xl md:text-5xl lg:text-6xl font-black tracking-tighter text-gc-text leading-[1.08] mb-5 max-w-3xl">
                    "Run agents on your terms."
                </h2>
                <p class="text-lg text-gc-text-muted leading-relaxed max-w-2xl mb-10">
                    "Own the data, the computers, and the model bill. We will map specialized agents onto the work you already do."
                </p>
                <div class="flex flex-col sm:flex-row items-center gap-3">
                    <a
                        href=CALENDLY
                        target="_blank"
                        rel="noopener noreferrer"
                        class="w-full sm:w-auto bg-gc-brand text-white px-8 py-4 rounded-md text-sm font-bold tracking-wide hover:bg-gc-brand-hover active:scale-[0.98] transition-all text-center"
                    >
                        "Free consultation"
                    </a>
                    <a
                        href="https://goldcoders.dev/projects"
                        class="w-full sm:w-auto px-8 py-4 rounded-md text-sm font-semibold text-gc-text border border-gc-border hover:border-gc-brand/40 active:scale-[0.98] transition-all text-center"
                    >
                        "See the systems"
                    </a>
                </div>
            </div>
        </section>
    }
}
