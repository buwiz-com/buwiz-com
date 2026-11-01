use leptos::{html, prelude::*};

use crate::components::agent_window::{
    AgentScreen, AgentWindow, FINANCE, FINANCE_RAIL, box_desktop, chrome, dock_apps,
};
use crate::components::demo_player::{DemoBar, DemoCursor, DemoPointer, Script, Timeline, autoplay_when_visible};
use crate::components::demo_widgets::DemoMenu;

/// Log in once: NativeChat's request-user form asks for GitHub, Apple Passwords fills it with
/// Touch ID, and Chrome on Finance's computer signs in. Finance's screen lives in the pane; it
/// grows to fill the window while Chrome signs in and shrinks back once the session is saved.
/// Plays itself with a moving cursor.
///
/// Every field here is a drawn box, never an `<input>`, so the visitor's own password manager
/// has nothing to offer.
const USER: &str = "codeitlikemiley";
const DOTS: &str = "••••••••••••";
const OTP: &str = "482 913";

// Phases.
const IDLE: u8 = 0;
const PICKING: u8 = 1;
const TOUCH: u8 = 2;
const FILLED: u8 = 3;
const SUBMITTING: u8 = 4;
const DONE: u8 = 5;

/// What Chrome shows on Finance's computer.
const GH_LOGIN: u8 = 0;
const GH_OTP: u8 = 1;
const GH_IN: u8 = 2;

const STEPS: &[&str] = &["Form appears", "Saved logins", "Touch ID", "Continue", "Chrome signs in", "Session saved"];

const GH_FONT: &str = "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI','Noto Sans',Helvetica,Arial,sans-serif;";
const GH_FIELD: &str = "flex h-9 items-center rounded-md border border-[#d0d7de] px-3 text-[14px]";
const GH_FIELD_ON: &str =
    "flex h-9 items-center rounded-md border border-[#0969da] bg-[#e8f0fe] px-3 text-[14px] ring-2 ring-[#0969da]/25";

const BUBBLE_ME: &str = "max-w-[88%] self-end rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg";
const BUBBLE_FI: &str = "max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text";

fn octocat(class: &'static str) -> impl IntoView {
    view! {
        <svg viewBox="0 0 24 24" class=class fill="currentColor" aria-hidden="true">
            <path d="M12 .3a12 12 0 0 0-3.8 23.4c.6.1.8-.3.8-.6v-2c-3.3.7-4-1.6-4-1.6-.5-1.4-1.3-1.8-1.3-1.8-1.1-.7.1-.7.1-.7 1.2.1 1.8 1.2 1.8 1.2 1 1.8 2.8 1.3 3.5 1 .1-.8.4-1.3.7-1.6-2.7-.3-5.5-1.3-5.5-6 0-1.3.5-2.4 1.2-3.2-.1-.3-.5-1.5.1-3.2 0 0 1-.3 3.3 1.2a11.5 11.5 0 0 1 6 0C17 4.7 18 5 18 5c.6 1.7.2 2.9.1 3.2.8.8 1.2 1.9 1.2 3.2 0 4.6-2.8 5.6-5.5 5.9.4.4.8 1.1.8 2.2v3.3c0 .3.2.7.8.6A12 12 0 0 0 12 .3"></path>
        </svg>
    }
}

/// A GitHub form field: empty, or what's been typed so far with a caret while it's typing.
fn gh_field(text: RwSignal<String>, full: usize, mono: bool) -> impl IntoView {
    view! {
        <div class=move || if text.with(|t| t.is_empty() || t.chars().count() == full) { GH_FIELD } else { GH_FIELD_ON }>
            <span class=if mono { "font-mono tracking-[0.3em]" } else { "" }>{move || text.get()}</span>
            <Show when=move || text.with(|t| !t.is_empty() && t.chars().count() < full)>
                <span class="ml-px inline-block h-4 w-px bg-[#1f2328]"></span>
            </Show>
        </div>
    }
}

/// GitHub's page in Chrome on Finance's computer.
fn github(page: RwSignal<u8>, user: RwSignal<String>, pass: RwSignal<String>, otp: RwSignal<String>) -> impl IntoView {
    let login = move || {
        view! {
            <div class="flex h-full flex-col items-center justify-center gap-3 px-4">
                {octocat("h-12 w-12 text-[#1f2328]")}
                <div class="text-[22px] font-light tracking-[-0.3px]">"Sign in to GitHub"</div>
                <div class="flex w-[min(320px,100%)] flex-col gap-2 rounded-md border border-[#d0d7de] bg-white p-4">
                    <span class="text-[13px] font-semibold">"Username or email address"</span>
                    {gh_field(user, USER.chars().count(), false)}
                    <span class="mt-1 flex justify-between text-[13px] font-semibold">
                        "Password"
                        <span class="font-normal text-[#0969da]">"Forgot password?"</span>
                    </span>
                    {gh_field(pass, DOTS.chars().count(), false)}
                    <span class="mt-2 flex h-9 items-center justify-center rounded-md bg-[#1f883d] text-[14px] font-semibold text-white">"Sign in"</span>
                </div>
            </div>
        }
    };
    let two_factor = move || {
        view! {
            <div class="flex h-full flex-col items-center justify-center gap-3 px-4">
                {octocat("h-12 w-12 text-[#1f2328]")}
                <div class="text-[22px] font-light tracking-[-0.3px]">"Two-factor authentication"</div>
                <div class="flex w-[min(320px,100%)] flex-col gap-2 rounded-md border border-[#d0d7de] bg-white p-4">
                    <span class="text-[13px] font-semibold">"Authentication code"</span>
                    {gh_field(otp, OTP.chars().count(), true)}
                    <span class="mt-2 flex h-9 items-center justify-center rounded-md bg-[#1f883d] text-[14px] font-semibold text-white">"Verify"</span>
                    <span class="text-[12px] text-[#57606a]">"Touch ID fills in its code."</span>
                </div>
            </div>
        }
    };
    let signed_in = move || {
        view! {
            <div class="flex h-12 items-center gap-3 bg-[#f6f8fa] px-4 shadow-[0_1px_0_#d0d7de]">
                {octocat("h-8 w-8 text-[#1f2328]")}
                <span class="text-[14px] font-semibold">"Dashboard"</span>
                <span class="ml-auto grid h-7 w-7 place-items-center rounded-full bg-[#f0a04b] text-[10px] font-bold text-[#1a1a1a]">"Fi"</span>
            </div>
            <div class="grid h-[calc(100%-48px)] place-items-center px-4">
                <div class="gc-card-in flex flex-col items-center gap-2 rounded-2xl border-2 border-[#1a7f37]/40 bg-[#dafbe1] px-10 py-7 text-center text-[#1a7f37]">
                    <svg viewBox="0 0 24 24" class="h-14 w-14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"></circle><path d="M7.5 12.5l3 3 6-6.5"></path></svg>
                    <span class="text-[34px] font-bold leading-tight">"Signed in"</span>
                    <span class="text-[22px] font-semibold">"session saved"</span>
                    <span class="text-[14px] text-[#1f2328]">{format!("{USER} · github.com")}</span>
                </div>
            </div>
        }
    };
    chrome(
        octocat("h-3.5 w-3.5 shrink-0 text-[#1f2328]"),
        move || match page.get() {
            GH_LOGIN => "Sign in to GitHub",
            GH_OTP => "Two-factor authentication",
            _ => "GitHub",
        },
        "github.com",
        move || match page.get() {
            GH_LOGIN => "/login",
            GH_OTP => "/sessions/two-factor",
            _ => "/",
        },
        "bg-[#f6f8fa] text-[#1f2328]",
        GH_FONT,
        move || match page.get() {
            GH_LOGIN => login().into_any(),
            GH_OTP => two_factor().into_any(),
            _ => signed_in().into_any(),
        },
    )
}

#[component]
pub fn SecureVault() -> impl IntoView {
    let app = NodeRef::<html::Div>::new();
    let user_box = NodeRef::<html::Div>::new();
    let account = NodeRef::<html::Div>::new();
    let finger = NodeRef::<html::Div>::new();
    let cont = NodeRef::<html::Div>::new();

    let phase = RwSignal::new(IDLE);
    let fp_ok = RwSignal::new(false);
    let page = RwSignal::new(GH_LOGIN);
    let gh_user = RwSignal::new(String::new());
    let gh_pass = RwSignal::new(String::new());
    let otp = RwSignal::new(String::new());
    let screen = AgentScreen::new();
    let tl = Timeline::new();
    let cur = DemoCursor::new(tl, app, (0.38, 0.88));

    let type_into = |s: &mut Script, start: u32, sig: RwSignal<String>, text: &'static str| {
        for n in 1..=text.chars().count() {
            let typed: String = text.chars().take(n).collect();
            s.go(start + n as u32 * 55).then(move || sig.set(typed));
        }
    };

    tl.script(move |s| {
        phase.set(IDLE);
        fp_ok.set(false);
        page.set(GH_LOGIN);
        gh_user.set(String::new());
        gh_pass.set(String::new());
        otp.set(String::new());
        screen.reset();
        cur.reset();

        s.go(600).move_to(cur, user_box);
        s.go(1400).press(cur).then(move || phase.set(PICKING));
        s.go(1900).move_to(cur, account);
        s.go(2700).press(cur).then(move || phase.set(TOUCH));
        s.go(3200).move_to(cur, finger);
        s.go(4000).press(cur).then(move || fp_ok.set(true));
        s.go(4500).then(move || phase.set(FILLED));
        s.go(5000).move_to(cur, cont);
        s.go(5800).press(cur).then(move || phase.set(SUBMITTING));
        // Finance's screen grows out of the pane while Chrome signs in.
        s.go(6100).then(move || screen.expand()).move_home(cur);
        type_into(s, 6700, gh_user, USER);
        type_into(s, 7700, gh_pass, DOTS);
        s.go(8700).then(move || page.set(GH_OTP));
        type_into(s, 9000, otp, OTP);
        s.go(9800).then(move || page.set(GH_IN));
        // Back into the pane, still showing it's signed in.
        s.go(11400).then(move || screen.shrink());
        s.go(11800).then(move || phase.set(DONE));
    });

    // Play once, the first time the window scrolls into view.
    autoplay_when_visible(tl, app);

    let step_on = move || match phase.get() {
        IDLE => 0,
        PICKING => 1,
        TOUCH => 2,
        FILLED => 3,
        SUBMITTING => 4,
        _ => 5,
    };

    let desktop = move || box_desktop("Buwiz Bot Box · Finance", (), github(page, gh_user, gh_pass, otp), dock_apps());

    let overlay = move || {
        view! {
            // Touch ID sheet
            <Show when=move || phase.get() == TOUCH>
                <div class="absolute inset-0 z-20 grid place-items-center bg-black/35">
                    <div class="w-[min(290px,86%)] rounded-2xl bg-[#f5f5f7] p-5 text-center text-[#16161c] shadow-2xl">
                        <div class="mx-auto mb-2 w-11 h-11 rounded-xl bg-[#16161c] text-white flex items-center justify-center text-[12px] font-bold">"NC"</div>
                        <div class="text-[13px] font-bold">"NativeChat wants to use a saved password."</div>
                        <div class="mt-1 text-[11px] text-[#5e6278]">{format!("Touch ID to fill {USER} for github.com.")}</div>
                        <div node_ref=finger class="mx-auto my-3 w-14 h-14">
                            <svg viewBox="0 0 64 64" fill="none" stroke-width="2.6" stroke-linecap="round" class=move || if fp_ok.get() { "w-14 h-14 stroke-[#059669] transition-colors" } else { "w-14 h-14 stroke-[#e5484d] transition-colors" }>
                                <path d="M20 26a12 12 0 0 1 24 0v6" />
                                <path d="M14 30a18 18 0 0 1 36-4" />
                                <path d="M26 28a6 6 0 0 1 12 0v8c0 6-2 10-5 14" />
                                <path d="M32 30v8c0 5-2 9-5 12" />
                                <path d="M20 34c0 7-1 11-3 14" />
                                <path d="M44 36c0 6-1 10-3 14" />
                            </svg>
                        </div>
                        <div class="text-[11px] text-[#5e6278]">{move || if fp_ok.get() { "Done" } else { "Touch ID" }}</div>
                    </div>
                </div>
            </Show>
            <DemoPointer cursor=cur />
        }
    };

    view! {
        <section id="vault" class="relative py-24 md:py-32 border-t border-gc-border">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Log in once. The model never sees it."
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[52ch]">
                        "Your coworker asks for a sign-in right in the chat. Apple Passwords fills it with Touch ID, Chrome on its computer signs in, and the session is saved. The password never lands in the transcript."
                    </p>
                </div>

                <DemoBar steps=STEPS current=Signal::derive(move || Some(step_on())) timeline=tl />

                <AgentWindow win=app screen who=FINANCE rail=FINANCE_RAIL desktop overlay>
                    <div class=BUBBLE_ME>"Clone the invoices repo and run the close"</div>
                    <div class=BUBBLE_FI>
                        "GitHub wants a sign-in before I can clone. Fill it here. I only get the session, never the password."
                    </div>

                    {move || if phase.get() < SUBMITTING {
                        view! {
                            <div class="relative flex w-full max-w-[440px] flex-col gap-2.5 self-start rounded-2xl border border-gc-gold/50 bg-gc-gold/5 p-3.5">
                                <div class="text-[11px] font-semibold tracking-wide uppercase text-gc-gold">"● Action needed · github.com"</div>
                                <div class="relative">
                                    <div class="text-[11px] text-gc-text-muted mb-1">"Username or email"</div>
                                    <div
                                        node_ref=user_box
                                        class=move || if phase.get() == PICKING {
                                            "h-9 rounded-lg border border-gc-brand bg-gc-bg px-3 flex items-center text-[13px] text-gc-text ring-2 ring-gc-brand/20"
                                        } else {
                                            "h-9 rounded-lg border border-gc-border bg-gc-bg px-3 flex items-center text-[13px] text-gc-text"
                                        }
                                    >
                                        {move || if phase.get() >= TOUCH { view! { <span>{USER}</span> }.into_any() } else { view! { <span class="text-gc-text-faint">"Choose a saved login"</span> }.into_any() }}
                                    </div>
                                    <Show when=move || phase.get() == PICKING>
                                        <DemoMenu class="left-0 right-0 mt-1.5">
                                            <div class="px-2 py-1 text-[11px] text-gc-text-faint">"From your saved logins"</div>
                                            <div node_ref=account class="flex items-center gap-2.5 rounded-lg bg-gc-bg-hover px-2 py-1.5">
                                                <span class="w-7 h-7 rounded-md bg-gc-bg-muted flex items-center justify-center material-symbols-outlined text-[16px] text-gc-text-muted">"key"</span>
                                                <span class="text-[13px] leading-tight"><span class="block font-semibold text-gc-text">{USER}</span><span class="text-[11px] text-gc-text-faint">"github.com"</span></span>
                                            </div>
                                            <div class="flex items-center gap-2.5 px-2 py-1.5">
                                                <span class="w-7 h-7 rounded-md bg-gc-bg-muted flex items-center justify-center material-symbols-outlined text-[16px] text-gc-text-muted">"key"</span>
                                                <span class="text-[13px] leading-tight"><span class="block font-semibold text-gc-text">"goldcoders-bot"</span><span class="text-[11px] text-gc-text-faint">"github.com"</span></span>
                                            </div>
                                            <div class="mt-1 border-t border-gc-border px-2 pt-1.5 text-[11px] text-gc-text-faint">"Choose an account. Touch ID fills in its password."</div>
                                        </DemoMenu>
                                    </Show>
                                </div>
                                <div>
                                    <div class="text-[11px] text-gc-text-muted mb-1">"Password"</div>
                                    <div class="h-9 rounded-lg border border-gc-border bg-gc-bg px-3 flex items-center text-[13px] text-gc-text tracking-widest">
                                        {move || if phase.get() >= FILLED { view! { <span>{DOTS}</span> }.into_any() } else { view! { <span class="tracking-normal text-gc-text-faint">"Filled by Touch ID"</span> }.into_any() }}
                                    </div>
                                </div>
                                <div class="flex flex-wrap gap-2 pt-0.5">
                                    <div
                                        node_ref=cont
                                        class=move || {
                                            if phase.get() >= FILLED {
                                                "rounded-lg bg-gc-text text-gc-bg px-3.5 py-1.5 text-[12px] font-semibold"
                                            } else {
                                                "rounded-lg bg-gc-text/30 text-gc-bg px-3.5 py-1.5 text-[12px] font-semibold"
                                            }
                                        }
                                    >
                                        "Continue"
                                    </div>
                                    <div class="rounded-lg border border-gc-border px-3.5 py-1.5 text-[12px] font-semibold text-gc-text">"Open the screen"</div>
                                    <div class="rounded-lg border border-gc-border px-3.5 py-1.5 text-[12px] font-semibold text-gc-text">"Dismiss"</div>
                                </div>
                            </div>
                        }.into_any()
                    } else if phase.get() == SUBMITTING {
                        view! {
                            <div class="gc-card-in self-start inline-flex items-center gap-2 rounded-xl border border-gc-border px-3 py-2 text-[12px] text-gc-text-muted">
                                <span class="w-3.5 h-3.5 rounded-full border-2 border-gc-border border-t-gc-text animate-spin"></span>
                                "Submitting to github.com…"
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="gc-card-in self-start inline-flex items-center gap-2 rounded-xl border border-gc-border px-3 py-2 text-[12px] text-gc-text-muted">
                                <span class="text-gc-accent">"✓"</span>
                                {format!("Submitted · github.com · {USER}")}
                            </div>
                            <div class=format!("gc-card-in {BUBBLE_FI}")>
                                "Authenticated. Session saved. Cloning goldcoders/invoices now."
                            </div>
                        }.into_any()
                    }}
                </AgentWindow>
            </div>
        </section>
    }
}
