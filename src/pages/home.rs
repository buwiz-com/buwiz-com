//! The public home page, ported from the old server's marketing site
//! (`src/app/marketing` in buwiz-server). It needs no JavaScript: the app
//! tour switches screens with radio buttons and the FAQ uses `<details>`.
//! Styles live in `src/home.css`.

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

/// (tool, title, what it reads)
const AGENT_TOOLS: [(&str, &str, &str); 4] = [
    ("list_tax_profiles", "Active TINs", "Your active TINs and what each allows in the Buwiz app."),
    ("list_tin_claims", "TIN claims", "Every TIN you asked to activate, its review status and the reviewer's note."),
    (
        "list_submissions",
        "Returns sent",
        "The newest returns sent to BIR through Buwiz: form, period, status and when.",
    ),
    ("get_submission", "One return", "A single return you sent, with any error and its signed receipt."),
];

/// (icon, title, body)
const AGENT_PROMISES: [(&str, &str, &str); 3] = [
    ("visibility", "Read-only, always", "Agent access can look things up but never change your account."),
    ("key", "You approve each agent", "An agent signs in only after you allow it on a Buwiz consent page."),
    ("approval", "Filing stays with you", "Returns are sent from the Buwiz app on your computer, never by an agent."),
];

/// (question, answer)
const FAQS: [(&str, &str); 5] = [
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
        "Can an AI agent file for me?",
        "No. An agent you approve can read your active TINs, your TIN claims and the returns you sent, with their receipts. Its access is read-only; filing stays in the Buwiz app, with you.",
    ),
];

/// (icon, label)
const TRUST: [(&str, &str); 5] = [
    ("devices", "Windows · macOS · Linux"),
    ("description", "All 43 BIR forms"),
    ("cloud_off", "Works offline after sign-in"),
    ("account_tree", "Every branch under one TIN"),
    ("approval", "You approve every filing"),
];

#[page("/")]
pub async fn home_page(cx: &Cx) -> Result<impl View> {
    let signed_in = current(cx).await?.is_some();
    let mcp_url = format!("{}/mcp", state(cx).config.account_origin);
    Ok(view! {
        <link rel="preconnect" href="https://fonts.googleapis.com">
        <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Manrope:wght@400;500;600;700;800&display=swap">
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:wght,FILL@100..700,0..1&display=block">
        <link rel="stylesheet" href="/assets/home.css">
        <main class="home">
            hero(signed_in: signed_in)
            tour()
            account_section(signed_in: signed_in)
            agents_section(mcp_url: mcp_url)
            faq()
            footer()
        </main>
    })
}

#[component]
async fn icon(name: &'static str) -> Result<impl View> {
    Ok(view! { <span class="icon" aria-hidden="true">(name)</span> })
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
            <div class="hero-copy">
                <span class="pill rise">icon(name: "volunteer_activism") "Ease of Paying Taxes"</span>
                <h1 class="rise d1">"Filing your taxes, "<span class="accent">"finally easy."</span></h1>
                <p class="hero-lead rise d2">
                    "Buwiz fills in your BIR returns, checks each one against the official rules, reminds you before every deadline, then files it with BIR from your own connection. One TIN, every branch under it, all 43 forms."
                </p>
                <div class="hero-actions rise d3">account_actions(signed_in: signed_in)</div>
                <p class="hero-note rise d3">"Free account · the Buwiz app runs on Windows, macOS and Linux"</p>
            </div>
            <div class="journey-wrap rise d3">
                journey()
            </div>
            <div class="trust">
                for (name, label) in TRUST {
                    <span>icon(name: name) (label)</span>
                }
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

#[component]
async fn agents_section(mcp_url: String) -> Result<impl View> {
    Ok(view! {
        <section id="agents" class="section">
            <div class="section-head">
                <span class="eyebrow">"AI AGENTS"</span>
                <h2>"Ask your AI agent about your filings."</h2>
                <p class="section-lead">
                    "Connect an AI agent that speaks MCP to your Buwiz account. Approve it once and it can look up your active TINs, your TIN claims, the returns you sent and BIR's receipt for each. It only reads: filing stays in the Buwiz app, with you."
                </p>
            </div>
            <div class="agent-card panel">
                <div class="agent-url">
                    <span class="agent-url-label">"MCP server"</span>
                    <code>(mcp_url)</code>
                </div>
                <ul class="agent-tools">
                    for (tool, title, body) in AGENT_TOOLS {
                        <li>
                            <div class="agent-tool-head">
                                <span class="agent-tool-title">(title)</span>
                                <code>(tool)</code>
                            </div>
                            <p>(body)</p>
                        </li>
                    }
                </ul>
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
                    <p class="home-footer-note">"© 2026 Buwiz by Goldcoders. Philippine tax filing, made modern."</p>
                </div>
                <nav class="home-footer-links" aria-label="Footer">
                    <a href="https://goldcoders.dev/legalities">"Legalities"</a>
                    <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                    <a href="https://github.com/hexuria/buwiz-forms" target="_blank" rel="noopener noreferrer">"GitHub"</a>
                </nav>
            </div>
        </footer>
    })
}

#[cfg(test)]
mod tests {
    use super::FEATURES;
    use crate::web::SCREENSHOTS;

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
