//! The public home page, ported from the old server's marketing site
//! (`src/app/marketing` in buwiz-server). The app tour switches screens with
//! radio buttons and the FAQ uses `<details>`; only the AI Co-worker demos
//! need JavaScript (`src/demo.js`). Styles live in `src/home.css`, and the
//! demos' in `src/demo.css`.

use super::*;

/// One screen of the app tour. `screen` names a pair of screenshots served
/// at `/assets/screens/{screen}_light.webp` and `..._dark.webp`.
struct Feature {
    title: &'static str,
    badge: &'static str,
    description: &'static str,
    detail: &'static str,
    icon: &'static str,
    screen: &'static str,
}

const FEATURES: [Feature; 6] = [
    Feature {
        title: "Live Print Preview",
        badge: "Live",
        description: "Fill the on-screen form on the left and watch the BIR-format paper return update on the right as you type.",
        detail: "Print preview follows your open draft and redraws only the pages you changed, so you can check every comb box and schedule row before you print or file.",
        icon: "splitscreen",
        screen: "side_by_side",
    },
    Feature {
        title: "43 BIR Forms, Official Rules",
        badge: "43 Forms",
        description: "Every form is checked with rules built from the official eBIRForms package: 2,007 validations and 623 calculations across 9,592 fields.",
        detail: "Checks run as you type without freezing the window. Each message points at its numbered item, and the 'On this form' sidebar jumps to the next part that needs attention.",
        icon: "verified",
        screen: "form_2550q",
    },
    Feature {
        title: "Templates & Guided Mode",
        badge: "Tab",
        description: "Keep one template per form for each branch, then press Tab in Guided Mode to stop only at the fields that change each period.",
        detail: "Templates keep setup answers such as ATCs, tax elections, and categories, never last period's amounts. Turn any filed return into a template with 'Use as template'.",
        icon: "keyboard_tab",
        screen: "templates",
    },
    Feature {
        title: "Taxpayers & Branches",
        badge: "Branches",
        description: "Keep your head office (00000) and branches under one 9-digit TIN. Branches inherit address and RDO until you change them.",
        detail: "Switch taxpayer or branch from the title bar. Each tax year has its own form checklist: copy last year's forms in one click or start from a Non-VAT, VAT, Employer, or Professional starter pack.",
        icon: "account_tree",
        screen: "panel_profile",
    },
    Feature {
        title: "Deadlines Calendar",
        badge: "30 Days",
        description: "See every return due in the next 30 days across your taxpayers and branches, overdue items first.",
        detail: "Home lists overdue and upcoming deadlines with one-click Open, plus recently filed returns you can reuse as templates.",
        icon: "event_upcoming",
        screen: "calendar",
    },
    Feature {
        title: "Search Everything",
        badge: "Cmd+K",
        description: "Jump to any taxpayer, branch, recent return, or any of the 43 forms from the keyboard.",
        detail: "Press Cmd+K on macOS or Ctrl+K on Windows and Linux, then search by TIN, name, or form code such as 1601C, 2550Q, or 1701.",
        icon: "search",
        screen: "palette",
    },
];

/// (index, title, body)
const ACCOUNT_STEPS: [(&str, &str, &str); 3] = [
    (
        "01",
        "Create your account",
        "Sign up free with your email, here or from the desktop app the first time it opens. We email you a 6-digit code; there is no password to remember.",
    ),
    (
        "02",
        "Activate your TIN",
        "Upload your BIR Form 2303 and a government ID. A person reviews every claim, and each TIN has at most one owner. Your files are encrypted and deleted once the claim is decided.",
    ),
    (
        "03",
        "Sign in on your desktop",
        "Approve the Buwiz desktop or mini app once in your browser. It stays signed in until you sign out.",
    ),
];

/// One self-playing AI Co-worker demo; `film` names the one `/assets/demo.js`
/// draws into its box.
struct Demo {
    film: &'static str,
    tab: &'static str,
    eyebrow: &'static str,
    title: &'static str,
    lead: &'static str,
}

/// The demos in filing order.
const DEMOS: [Demo; 6] = [
    Demo {
        film: "connect",
        tab: "Connect",
        eyebrow: "CONNECT",
        title: "Install Buwiz from the chat. Log in with Touch ID.",
        lead: "Ask your co-worker to install the Buwiz plugin, then say “Log in with Buwiz”. A login card appears in the chat and your saved password fills it with Touch ID. You choose what the co-worker may do on your account.",
    },
    Demo {
        film: "profiles",
        tab: "Tax profiles",
        eyebrow: "TAX PROFILES",
        title: "One TIN, every branch, as live cards.",
        lead: "A salary, two bakeshops and an apartment: a mixed-income earner keeps them all under one TIN. Ask in plain words and get your branches, the tax year, which branch files which form and what falls due next. Keep any of them as a Mini App.",
    },
    Demo {
        film: "prepare",
        tab: "Prepare",
        eyebrow: "PREPARE",
        title: "One spreadsheet for every branch. A finished form for each.",
        lead: "Attach the payroll workbook your accountant sent for all your branches. The co-worker reads it, fills one 1601-C per branch, checks each against every official rule and opens them as BIR prints them, for you to review.",
    },
    Demo {
        film: "submit",
        tab: "Submit",
        eyebrow: "SUBMIT",
        title: "Say “submit”. Approve it. It is queued.",
        lead: "The co-worker checks the form again, asks for permission to publish if you never gave it, and shows you exactly what will be filed. Nothing leaves until you approve, and then it goes to BIR from the Buwiz app on your computer.",
    },
    Demo {
        film: "confirm",
        tab: "Confirmation",
        eyebrow: "CONFIRMATION",
        title: "A routine watches your inbox for BIR's confirmation.",
        lead: "Install Gmail and ask for a routine. Every minute it looks for BIR's confirmation email; when it arrives, the confirmation comes back as a PDF to print or keep.",
    },
    Demo {
        film: "pay",
        tab: "Pay",
        eyebrow: "PAY",
        title: "Paid online through LandBank, with your OK.",
        lead: "A second routine waits for the confirmation, then opens the bank's payment site on the co-worker's computer and fills in BIR, the form, the period, the ATC and the amount. You log in with Touch ID and approve the amount; the payment itself is made on the bank's page.",
    },
];

/// (icon, title, body)
const AGENT_PROMISES: [(&str, &str, &str); 3] = [
    (
        "approval",
        "You approve every filing and payment",
        "Buwiz does the work, then stops and asks. Nothing is filed or paid until you confirm.",
    ),
    (
        "verified",
        "Re-checked before it is sent",
        "Every return runs through BIR's rules again right before it leaves your computer.",
    ),
    (
        "visibility",
        "Masked TINs, read-only access",
        "Agents connected to your Buwiz account see masked TINs only, and can read it but never change it.",
    ),
];

/// (question, answer)
const FAQS: [(&str, &str); 6] = [
    (
        "Do I need a Buwiz account?",
        "Yes. The desktop app asks you to sign in or create a free account the first time it opens. Your account is how Buwiz knows which TINs are yours and keeps each TIN with a single owner.",
    ),
    (
        "How does activating a TIN work?",
        "On My TINs, upload your BIR Form 2303, a government ID and any supporting documents. A reviewer checks them by hand, and the TIN becomes active only if the claim is approved. Active TINs unlock filing in the Buwiz app.",
    ),
    (
        "How does Buwiz send my return to BIR?",
        "Through your own connection. When you file from the Buwiz app, the return travels to BIR from your computer, so BIR sees you, not us. The app connects only to BIR's own servers, and every filing ends with a signed receipt.",
    ),
    (
        "Does the app work offline?",
        "Yes, after that first sign-in, and it stays signed in until you sign out. You need a connection to file a return and to add a TIN.",
    ),
    (
        "Is there a mobile app?",
        "Yes, it's coming soon. The Buwiz app is coming to Windows, macOS, Linux, Android and iOS, all with the same Buwiz account.",
    ),
    (
        "Can an AI agent file for me?",
        "Buwiz, your AI co-worker, does the work and you approve it. Nothing is filed or paid until you confirm: returns are filed from the Buwiz app on your computer, and payments are made on your bank's own page. Agents connected to your Buwiz account can only read it, and see masked TINs.",
    ),
];

/// Where the Buwiz app runs: (icon, platform, coming soon).
const PLATFORMS: [(&str, &str, bool); 5] = [
    ("desktop_windows", "Windows", true),
    ("laptop_mac", "macOS", true),
    ("terminal", "Linux", true),
    ("android", "Android", true),
    ("phone_iphone", "iOS", true),
];

/// (figure, what it counts)
const HERO_STATS: [(&str, &str); 4] = [
    ("43", "BIR forms, every one"),
    ("2,007", "official validation rules"),
    ("30 days", "of deadlines in view"),
    ("Offline", "after one sign-in"),
];

#[page("/")]
pub async fn home_page(cx: &Cx) -> Result<impl View> {
    let signed_in = current(cx).await?.is_some();
    Ok(view! {
        brand_styles()
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,600&family=Figtree:wght@400;500;600;700&family=JetBrains+Mono:wght@500;700&display=swap">
        <link rel="stylesheet" href="/assets/demo.css">
        <main class="home">
            hero(signed_in: signed_in)
            month()
            tour()
            account_section(signed_in: signed_in)
            agents_section()
            faq()
            footer()
        </main>
        <script src="/assets/demo.js" defer=(true)></script>
    })
}

/// Create account and sign in, or the way into the account when signed in.
#[component]
async fn account_actions(signed_in: bool) -> Result<impl View> {
    Ok(view! {
        if signed_in {
            <a class="btn btn-solid" href="/account">icon(name: "badge") "Open My TINs"</a>
            <a class="btn btn-ghost" href="/tins/new">"Add a TIN"</a>
        } else {
            <a class="btn btn-solid" href="/signup">icon(name: "person_add") "Create account"</a>
            <a class="btn btn-ghost" href="/login">"Sign in"</a>
        }
    })
}

#[component]
async fn hero(signed_in: bool) -> Result<impl View> {
    Ok(view! {
        <section id="hero" class="hero-wrap">
            <div class="hero-glow" aria-hidden="true"></div>
            <div class="hero-grid">
                <div class="hero-copy">
                    <span class="pill rise">icon(name: "volunteer_activism") "Ease of Paying Taxes"</span>
                    <h1 class="rise d1" aria-label="BIR filing, finally easy.">
                        <span aria-hidden="true">"BIR filing,"</span>
                        <span class="hero-rotator" aria-hidden="true">
                            <span class="hero-words">
                                <span>"finally easy."</span>
                                <span>"on time."</span>
                                <span>"error-free."</span>
                                <span>"anywhere."</span>
                                <span>"finally easy."</span>
                            </span>
                        </span>
                    </h1>
                    <p class="hero-lead rise d2">
                        "Buwiz fills in your BIR returns, checks every one against the official eBIRForms rules, reminds you before each deadline, then files it with BIR from your own connection. One TIN, every branch under it, all 43 forms."
                    </p>
                    <div class="hero-actions rise d3">account_actions(signed_in: signed_in)</div>
                    <ul class="platforms rise d3" aria-label="Where Buwiz runs">
                        for (name, platform, soon) in PLATFORMS {
                            <li class=(if soon { "platform soon" } else { "platform" })>
                                icon(name: name)
                                (platform)
                                if soon {
                                    <span class="soon-tag">"Soon"</span>
                                }
                            </li>
                        }
                    </ul>
                </div>
                <div class="hero-visual rise d2">
                    <div class="device-desktop">
                        <div class="device-bar" aria-hidden="true">
                            <span class="device-dots"><i></i><i></i><i></i></span>
                            <span class="device-title">"Buwiz · 1601-C · October 2026"</span>
                        </div>
                        <picture>
                            <source srcset="/assets/screens/side_by_side_dark.webp" media="(prefers-color-scheme: dark)">
                            <img
                                src="/assets/screens/side_by_side_light.webp"
                                alt="The Buwiz desktop app: a BIR return beside its live print preview"
                                width="1440"
                                height="1129"
                            >
                        </picture>
                    </div>
                    <div class="device-phone">phone_screen()</div>
                    <div class="float-chip chip-check" aria-hidden="true">
                        icon(name: "verified")
                        <span><b>"0 errors"</b><small>"2,007 checks passed"</small></span>
                    </div>
                    <div class="float-chip chip-due" aria-hidden="true">
                        icon(name: "event_upcoming")
                        <span><b>"1601-C due Nov 10"</b><small>"Reminder · 17 days left"</small></span>
                    </div>
                    <div class="float-chip chip-filed" aria-hidden="true">
                        icon(name: "mark_email_read")
                        <span><b>"Received by BIR"</b><small>"3 Nov · 10:43 AM"</small></span>
                    </div>
                </div>
            </div>
            <dl class="hero-stats">
                for (figure, label) in HERO_STATS {
                    <div>
                        <dt>(figure)</dt>
                        <dd>(label)</dd>
                    </div>
                }
            </dl>
        </section>
    })
}

/// The phone in the hero: the Buwiz phone app's Home tab, drawn from its
/// approved mobile design (280 × 580 frame) in the app's theme colours:
/// the taxpayer pill, the greeting, the Due next card, a draft to pick up,
/// a return waiting to send, and the four tabs. Now and then the toast the
/// app shows after queueing a return slides in.
#[component]
async fn phone_screen() -> Result<impl View> {
    Ok(view! {
        <div class="ph" role="img" aria-label="The Buwiz mobile app, coming soon: Home, with the next return due, a draft to pick up and a return waiting to send">
            <div class="ph-screen">
                <div class="ph-sb">
                    <span>"9:41"</span>
                    <i class="ph-island"></i>
                    <span class="ph-sig"><i></i><i></i><i></i><b></b></span>
                </div>
                <div class="ph-top"><span class="ph-pill"><span class="ph-av">"JS"</span>"Juana"</span></div>
                <div class="ph-lt">"Good morning"</div>
                <div class="ph-hero">
                    <small>"Due next · in 17 days"</small>
                    <b>"1701Q · Q3 2026"</b>
                    <small>"Nov 15 · quarterly income tax"</small>
                    <span class="ph-btn">"Start return" phone_icon(d: "M4 10h12M11 5l5 5-5 5")</span>
                </div>
                <div class="ph-sec">"Pick up where you left off"</div>
                <div class="ph-row">
                    <span class="ph-code">"2551Q"</span>
                    <span class="ph-m"><b>"Percentage tax · Q3"</b><small>"Draft · 2 left to fill"</small></span>
                    phone_icon(d: "M8 5l5 5-5 5")
                </div>
                <div class="ph-sec">"Sending"</div>
                <div class="ph-row">
                    <span class="ph-code">"1701Q"</span>
                    <span class="ph-m"><b>"Income tax · Q2"</b><small>"Queued · sends when online"</small></span>
                    <span class="ph-tag">"Waiting"</span>
                </div>
                <div class="ph-fill"></div>
                <div class="ph-tabs">
                    for (i, (label, d)) in PHONE_TABS.iter().enumerate() {
                        <span class=(if i == 0 { "on" } else { "" })>phone_icon(d: d) (*label)</span>
                    }
                </div>
                <div class="ph-toast">"✓ Queued. It sends when you're online."</div>
            </div>
        </div>
    })
}

/// One of the phone app's 20 × 20 stroke icons.
#[component]
async fn phone_icon(d: &'static str) -> Result<impl View> {
    Ok(view! {
        <svg class="ph-i" viewBox="0 0 20 20" aria-hidden="true"><path d=(d)></path></svg>
    })
}

/// The phone app's four tabs: (label, icon path).
const PHONE_TABS: [(&str, &str); 4] = [
    ("Home", "M3 9l7-6 7 6v8H3z M8 17v-5h4v5"),
    ("Forms", "M5 2.5h7l3 3v12H5z M12 2.5v3h3 M7.5 10h5 M7.5 13h5"),
    ("Filings", "M3 5h14v11H3z M3 9h14 M7 2.5v3 M13 2.5v3"),
    ("Calendar", "M3 4.5h14v12H3z M3 8.5h14 M7 2.5v3 M13 2.5v3"),
];

/// One month with Buwiz, under the hero: the journey card.
#[component]
async fn month() -> Result<impl View> {
    Ok(view! {
        <section class="month">
            <p class="month-label">"One month with Buwiz, from the deadline to BIR's receipt"</p>
            <div class="journey-wrap">
                journey()
            </div>
        </section>
    })
}

/// One month of an employer's withholding return, from the deadline that
/// starts it to BIR's receipt.
#[component]
async fn journey() -> Result<impl View> {
    Ok(view! {
        <div class="journey">
            <div class="deadline">
                <span class="icon deadline-icon" aria-hidden="true">"event_upcoming"</span>
                <div class="deadline-text">
                    <div class="deadline-title">"1601-C for October is due Nov 10"</div>
                    <div class="deadline-sub">"JUAN DELA CRUZ · TIN 123-456-788 · 3 branches with staff"</div>
                </div>
                <span class="deadline-chip">"17 days left"</span>
            </div>
            <div class="journey-steps">
                <div class="jstep">
                    <div class="jstep-head"><span class="jnum">"1"</span><span>"Prepare"</span></div>
                    <p>"Templates hold each branch's setup. Type only what changed: one return per branch."</p>
                    <div class="jstep-foot">
                        <div class="jrow"><span class="mono">"00001 Makati"</span><span class="num">"₱5,598.60"</span></div>
                        <div class="jrow"><span class="mono">"00002 QC"</span><span class="num">"₱830.10"</span></div>
                        <div class="jrow"><span class="mono">"00003 Pasig"</span><span class="num">"₱0.00"</span></div>
                    </div>
                </div>
                <div class="jstep">
                    <div class="jstep-head"><span class="jnum">"2"</span><span>"Check"</span></div>
                    <p>"Every field runs through BIR's own eBIRForms rules before anything is filed."</p>
                    <div class="jstep-foot jcheck">
                        <div class="jcheck-title">icon(name: "verified") "0 errors"</div>
                        <div class="jcheck-sub">"2,007 validations · 623 calculations"</div>
                    </div>
                </div>
                <div class="jstep">
                    <div class="jstep-head"><span class="jnum">"3"</span><span>"File"</span></div>
                    <p>"Approve it once. Buwiz sends it to BIR from your own connection and keeps the signed receipt."</p>
                    <div class="jstep-foot jfile">
                        <div class="jfile-title">icon(name: "mark_email_read") "Received by BIR"</div>
                        <div class="mono truncate">"123456788001-1601Cv2018-102026.xml"</div>
                        <div class="faint">"3 November 2026 · 10:43 AM"</div>
                    </div>
                </div>
            </div>
        </div>
    })
}

#[component]
async fn screenshot(screen: &'static str, alt: &'static str) -> Result<impl View> {
    Ok(view! {
        <picture>
            <source srcset=(format!("/assets/screens/{screen}_dark.webp")) media="(prefers-color-scheme: dark)">
            <img
                src=(format!("/assets/screens/{screen}_light.webp"))
                alt=(alt)
                width="1440"
                height="1129"
                loading="lazy"
            >
        </picture>
    })
}

/// The app tour: a list of screens (chips on phones) and the picked one.
/// Each radio sits before the list and the stage, so CSS can show the
/// matching panel with `#tour-N:checked ~ ...`.
#[component]
async fn tour() -> Result<impl View> {
    let count = FEATURES.len();
    Ok(view! {
        <section id="features" class="section">
            <div class="section-head">
                <span class="eyebrow">icon(name: "photo_library") (format!("Tour the app · {count} screens"))</span>
                <h2>"Everything you file, in one calm window."</h2>
                <p class="section-lead">
                    "Built around how business owners, bookkeepers, and CPAs actually file: pick the taxpayer, open the form, fill only what changed."
                </p>
            </div>
            <div class="tour">
                for (i, _) in FEATURES.iter().enumerate() {
                    <input
                        class="tour-radio"
                        type="radio"
                        name="tour"
                        id=(format!("tour-{i}"))
                        aria-label=(FEATURES[i].title)
                        checked=(i == 0)
                    >
                }
                <div class="tour-list">
                    for (i, feature) in FEATURES.iter().enumerate() {
                        <label class=(format!("tour-item tour-item-{i}")) for=(format!("tour-{i}"))>
                            <span class="tour-icon">icon(name: feature.icon)</span>
                            <span class="tour-text">
                                <span class="tour-title-row">
                                    <span class="tour-title">(feature.title)</span>
                                    <span class="badge">(feature.badge)</span>
                                </span>
                                <span class="tour-desc">(feature.description)</span>
                            </span>
                        </label>
                    }
                </div>
                <div class="tour-stage">
                    for (i, feature) in FEATURES.iter().enumerate() {
                        <figure class=(format!("tour-panel tour-panel-{i}"))>
                            <div class="tour-bar">
                                <span class="tour-bar-title">icon(name: feature.icon) (feature.title)</span>
                                <span class="badge">(feature.badge)</span>
                            </div>
                            <div class="tour-shot">
                                screenshot(screen: feature.screen, alt: feature.title)
                                <span class="tour-count">(format!("{} / {count}", i + 1))</span>
                            </div>
                            <figcaption>
                                <p class="tour-panel-desc">(feature.description)</p>
                                <p>(feature.detail)</p>
                            </figcaption>
                        </figure>
                    }
                </div>
            </div>
        </section>
    })
}

#[component]
async fn account_section(signed_in: bool) -> Result<impl View> {
    Ok(view! {
        <section id="account" class="section">
            <div class="section-head">
                <h2>"One Buwiz account. Your TIN stays yours."</h2>
                <p class="section-lead">
                    "The same account signs you in on the web and in the Buwiz apps, and it is how Buwiz keeps every TIN with its rightful owner."
                </p>
            </div>
            <ol class="steps-grid">
                for (index, title, body) in ACCOUNT_STEPS {
                    <li class="panel">
                        <span class="step-index">(index)</span>
                        <h3>(title)</h3>
                        <p>(body)</p>
                    </li>
                }
            </ol>
            <div class="row-actions">account_actions(signed_in: signed_in)</div>
        </section>
    })
}

/// The AI Co-worker demos: a tab per demo (radios, like the tour), each with
/// its story and an empty box that `/assets/demo.js` draws the app into and
/// plays while it is on screen.
#[component]
async fn agents_section() -> Result<impl View> {
    Ok(view! {
        <section id="agents" class="section">
            <div class="section-head">
                <span class="eyebrow">icon(name: "smart_toy") "AI Co-worker"</span>
                <h2>"AI Native Tax Filing"</h2>
                <p class="section-lead">
                    "Give the paperwork to Buwiz, your AI co-worker. It works in your apps on your computer: it reads your tax profiles, turns your accountant's spreadsheet into a finished BIR form and checks it, watches your inbox for BIR's confirmation and fills in the bank's payment page. Then it stops and asks. Returns are filed from the Buwiz app and paid on your bank's own page, each only after you confirm."
                </p>
            </div>
            <div class="demos">
                for (i, demo) in DEMOS.iter().enumerate() {
                    <input
                        class="demo-radio"
                        type="radio"
                        name="demo"
                        id=(format!("demo-{i}"))
                        value=(demo.film)
                        aria-label=(demo.tab)
                        checked=(i == 0)
                    >
                }
                <div class="demo-tabs">
                    for (i, demo) in DEMOS.iter().enumerate() {
                        <label class=(format!("demo-tab demo-tab-{i}")) for=(format!("demo-{i}"))>
                            <span class="demo-tab-num">(format!("{:02}", i + 1))</span>
                            (demo.tab)
                        </label>
                    }
                </div>
                for (i, demo) in DEMOS.iter().enumerate() {
                    <div class=(format!("demo-panel demo-panel-{i}"))>
                        <div class="demo-copy">
                            <span class="eyebrow">(format!("{:02} · {}", i + 1, demo.eyebrow))</span>
                            <h3>(demo.title)</h3>
                            <p class="section-lead">(demo.lead)</p>
                        </div>
                        <div class="demo" data-film=(demo.film)></div>
                    </div>
                }
            </div>
            <div class="promises">
                for (name, title, body) in AGENT_PROMISES {
                    <div class="promise">
                        <span class="promise-icon">icon(name: name)</span>
                        <div>
                            <div class="promise-title">(title)</div>
                            <p>(body)</p>
                        </div>
                    </div>
                }
            </div>
        </section>
    })
}

#[component]
async fn faq() -> Result<impl View> {
    Ok(view! {
        <section id="faq" class="section">
            <div class="faq">
                <h2>"Questions"</h2>
                <div class="faq-list">
                    for (i, (question, answer)) in FAQS.iter().enumerate() {
                        <details open=(i == 0)>
                            <summary>
                                <span>(*question)</span>
                                <span class="icon faq-chevron" aria-hidden="true">"expand_more"</span>
                            </summary>
                            <p>(*answer)</p>
                        </details>
                    }
                </div>
            </div>
        </section>
    })
}

#[component]
async fn footer() -> Result<impl View> {
    Ok(view! {
        <footer class="home-footer">
            <div class="home-footer-inner">
                <div>
                    <p class="home-footer-name">"Buwiz"</p>
                    <p class="home-footer-note">"© 2026 Buwiz. Philippine tax filing, made modern."</p>
                </div>
                <nav class="home-footer-links" aria-label="Footer">
                    <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                </nav>
            </div>
        </footer>
    })
}

#[cfg(test)]
mod tests {
    use super::{DEMOS, FEATURES};
    use crate::web::{ASSETS, SCREENSHOTS};

    #[test]
    fn every_demo_is_drawn_and_its_documents_served() {
        let script = include_str!("../demo.js");
        for demo in &DEMOS {
            let film = format!("{}: {}Film,", demo.film, demo.film);
            assert!(script.contains(&film), "demo.js has no {film}");
        }
        let used: Vec<&str> = script.split("\"/assets/").skip(1).filter_map(|rest| rest.split('"').next()).collect();
        assert!(!used.is_empty());
        for path in used {
            assert!(ASSETS.iter().any(|(known, ..)| *known == path), "{path} is not served");
        }
    }

    #[test]
    fn every_tour_screen_is_served_in_both_themes() {
        for feature in &FEATURES {
            for theme in ["light", "dark"] {
                let name = format!("{}_{theme}.webp", feature.screen);
                let (_, bytes) = SCREENSHOTS.iter().find(|(known, _)| *known == name).expect(&name);
                assert!(bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP", "{name}");
            }
        }
        assert_eq!(SCREENSHOTS.len(), FEATURES.len() * 2);
    }
}
