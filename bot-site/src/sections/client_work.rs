use crate::app::ASSETS;
use leptos::prelude::*;

struct Client {
    name: &'static str,
    what: &'static str,
    href: &'static str,
    /// Under [`ASSETS`].
    logo: &'static str,
    span: &'static str,
}

const CLIENTS: [Client; 4] = [
    Client {
        name: "Meaningful Ventures Greenland",
        what: "Production tax and accounting platform on Taxman.",
        href: "https://digits.mvgreenland.com",
        logo: "images/logos/mvg_original.png",
        span: "lg:col-span-7",
    },
    Client {
        name: "WatchCrunch",
        what: "iOS, Android, and web for a global enthusiast community.",
        href: "https://www.watchcrunch.com",
        logo: "images/logos/watchcrunch.png",
        span: "lg:col-span-5",
    },
    Client {
        name: "Stylar",
        what: "Generative image suite on web and iOS.",
        href: "https://www.stylar.com",
        logo: "images/logos/stylar.png",
        span: "lg:col-span-5",
    },
    Client {
        name: "Bad Character Scanner",
        what: "Enterprise tooling for hidden-character attacks in source.",
        href: "https://badcharacterscanner.com",
        logo: "images/logos/badcharacter.svg",
        span: "lg:col-span-7",
    },
];

/// Production work for other companies. Kept below the agent stack on purpose.
#[component]
pub fn ClientWork() -> impl IntoView {
    view! {
        <section id="about" class="relative py-24 md:py-32">
            <div class="max-w-[1440px] mx-auto px-6 md:px-12">
                <div class="max-w-2xl mb-12 md:mb-16">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "The same team ships for other companies."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[46ch]">
                        "Platforms, mobile, and security tools already in production. Agentic AI is the new layer, not the only thing we build."
                    </p>
                </div>

                <div class="grid grid-cols-1 lg:grid-cols-12 gap-5 md:gap-6">
                    {CLIENTS.iter().map(|client| {
                        let span = client.span;
                        view! {
                            <a
                                href=client.href
                                target="_blank"
                                rel="noopener noreferrer"
                                class=format!("{span} group rounded-2xl border border-gc-border bg-gc-bg-card p-8 md:p-10 hover:-translate-y-1 hover:border-gc-brand/30 transition-all duration-300")
                            >
                                <div class="w-16 h-16 md:w-20 md:h-20 rounded-2xl border border-gc-border bg-gc-bg-muted flex items-center justify-center mb-8 group-hover:scale-105 transition-transform">
                                    <img src=format!("{ASSETS}/{}", client.logo) alt="" class="w-12 h-12 md:w-14 md:h-14 object-contain" />
                                </div>
                                <h3 class="text-xl md:text-2xl font-bold tracking-tight text-gc-text mb-2">
                                    {client.name}
                                </h3>
                                <p class="text-sm md:text-base text-gc-text-muted leading-relaxed">
                                    {client.what}
                                </p>
                            </a>
                        }
                    }).collect_view()}
                </div>

                <div class="mt-12 pt-8 border-t border-gc-border flex flex-col md:flex-row md:items-center md:justify-between gap-4">
                    <p class="text-sm text-gc-text-muted">
                        "Also shipping Taxman, eBIRForms, Cargo Runner, and WAZ."
                    </p>
                    <a
                        href="https://goldcoders.dev/projects"
                        class="inline-flex items-center gap-2 text-sm font-semibold text-gc-text hover:text-gc-brand transition-colors"
                    >
                        "All systems"
                        <span class="material-symbols-outlined text-[16px]">"arrow_forward"</span>
                    </a>
                </div>
            </div>
        </section>
    }
}
