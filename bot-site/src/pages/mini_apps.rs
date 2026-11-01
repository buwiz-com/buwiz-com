use crate::app::meta_content;
use crate::components::layout::Layout;
use crate::sections::mini_apps::{APPS, app_icon};
use leptos::prelude::*;
use leptos_meta::*;

/// `/mini-apps`: every Buwiz mini app, one card each.
#[component]
pub fn MiniAppsPage() -> impl IntoView {
    // Filled in by the server; see index.html.
    let forms = meta_content("buwiz-forms-origin");

    view! {
        <Layout>
            <Title text="Mini apps: eBIRForms, Books, Word and Spreadsheet | Buwiz Bot" />
            <Meta
                name="description"
                content="Buwiz mini apps run inside Buwiz Bot, in the chat and full screen: eBIRForms, Books, Word and Spreadsheet."
            />

            <div class="relative min-h-screen">
                <div class="gc-grain" aria-hidden="true"></div>
                <section class="relative py-20 md:py-28">
                    <div class="max-w-[1128px] mx-auto px-6">
                        <div class="max-w-2xl mb-12 md:mb-16">
                            <h1 class="text-4xl md:text-6xl font-black tracking-tighter text-gc-text leading-[1.05] mb-4">
                                "Mini apps"
                            </h1>
                            <p class="text-lg text-gc-text-muted max-w-[56ch]">
                                "Apps your coworker opens for you, right in the chat or full screen. Each one is a signed plugin."
                            </p>
                        </div>

                        <ul class="grid grid-cols-1 gap-5 md:grid-cols-2 md:gap-6">
                            {APPS.iter().map(|a| {
                                let site = (a.id == "app.buwiz.ebirforms").then(|| forms.clone());
                                view! {
                                    <li class="flex flex-col gap-4 rounded-2xl border border-gc-border bg-gc-bg-card p-6 md:p-8">
                                        <div class="flex items-center gap-4">
                                            {app_icon(a, "h-12 w-12", "text-[26px]")}
                                            <div class="min-w-0">
                                                <h2 class="text-xl md:text-2xl font-bold tracking-tight text-gc-text">{a.name}</h2>
                                                <div class="truncate font-mono text-[12px] text-gc-text-faint">{a.id}</div>
                                            </div>
                                        </div>
                                        <p class="text-base text-gc-text-muted">{a.line}</p>
                                        <div class="mt-auto flex flex-wrap items-center gap-x-6 gap-y-2 pt-2">
                                            <span class="text-[11px] font-semibold uppercase tracking-wide text-gc-text-faint">"Runs inside Buwiz Bot"</span>
                                            {site.map(|href| view! {
                                                <a href=href class="inline-flex items-center gap-1.5 text-sm font-semibold text-gc-text hover:text-gc-brand transition-colors">
                                                    "Open eBIRForms site"
                                                    <span class="material-symbols-outlined text-[16px]">"arrow_outward"</span>
                                                </a>
                                            })}
                                        </div>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                </section>
            </div>
        </Layout>
    }
}
