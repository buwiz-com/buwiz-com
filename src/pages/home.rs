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

/// Where the Buwiz app will run: (name, paths of its logo in a 24 × 24
/// box, each with a transform). Logos only, so the row fits one line on a
/// phone. Apple, Linux (Tux) and Android are from Simple Icons
/// (simpleicons.org, CC0); Windows is four panes, and iOS a phone with the
/// Apple logo, so it reads apart from macOS.
const PLATFORMS: [(&str, &[(&str, &str)]); 5] = [
    ("Windows", &[("M3 3h8.5v8.5H3zM12.5 3H21v8.5h-8.5zM3 12.5h8.5V21H3zM12.5 12.5H21V21h-8.5z", "")]),
    ("macOS", &[(APPLE, "")]),
    (
        "Linux",
        &[(
            "M12.504 0c-.155 0-.315.008-.48.021-4.226.333-3.105 4.807-3.17 6.298-.076 1.092-.3 1.953-1.05 3.02-.885 1.051-2.127 2.75-2.716 4.521-.278.832-.41 1.684-.287 2.489a.424.424 0 00-.11.135c-.26.268-.45.6-.663.839-.199.199-.485.267-.797.4-.313.136-.658.269-.864.68-.09.189-.136.394-.132.602 0 .199.027.4.055.536.058.399.116.728.04.97-.249.68-.28 1.145-.106 1.484.174.334.535.47.94.601.81.2 1.91.135 2.774.6.926.466 1.866.67 2.616.47.526-.116.97-.464 1.208-.946.587-.003 1.23-.269 2.26-.334.699-.058 1.574.267 2.577.2.025.134.063.198.114.333l.003.003c.391.778 1.113 1.132 1.884 1.071.771-.06 1.592-.536 2.257-1.306.631-.765 1.683-1.084 2.378-1.503.348-.199.629-.469.649-.853.023-.4-.2-.811-.714-1.376v-.097l-.003-.003c-.17-.2-.25-.535-.338-.926-.085-.401-.182-.786-.492-1.046h-.003c-.059-.054-.123-.067-.188-.135a.357.357 0 00-.19-.064c.431-1.278.264-2.55-.173-3.694-.533-1.41-1.465-2.638-2.175-3.483-.796-1.005-1.576-1.957-1.56-3.368.026-2.152.236-6.133-3.544-6.139zm.529 3.405h.013c.213 0 .396.062.584.198.19.135.33.332.438.533.105.259.158.459.166.724 0-.02.006-.04.006-.06v.105a.086.086 0 01-.004-.021l-.004-.024a1.807 1.807 0 01-.15.706.953.953 0 01-.213.335.71.71 0 00-.088-.042c-.104-.045-.198-.064-.284-.133a1.312 1.312 0 00-.22-.066c.05-.06.146-.133.183-.198.053-.128.082-.264.088-.402v-.02a1.21 1.21 0 00-.061-.4c-.045-.134-.101-.2-.183-.333-.084-.066-.167-.132-.267-.132h-.016c-.093 0-.176.03-.262.132a.8.8 0 00-.205.334 1.18 1.18 0 00-.09.4v.019c.002.089.008.179.02.267-.193-.067-.438-.135-.607-.202a1.635 1.635 0 01-.018-.2v-.02a1.772 1.772 0 01.15-.768c.082-.22.232-.406.43-.533a.985.985 0 01.594-.2zm-2.962.059h.036c.142 0 .27.048.399.135.146.129.264.288.344.465.09.199.14.4.153.667v.004c.007.134.006.2-.002.266v.08c-.03.007-.056.018-.083.024-.152.055-.274.135-.393.2.012-.09.013-.18.003-.267v-.015c-.012-.133-.04-.2-.082-.333a.613.613 0 00-.166-.267.248.248 0 00-.183-.064h-.021c-.071.006-.13.04-.186.132a.552.552 0 00-.12.27.944.944 0 00-.023.33v.015c.012.135.037.2.08.334.046.134.098.2.166.268.01.009.02.018.034.024-.07.057-.117.07-.176.136a.304.304 0 01-.131.068 2.62 2.62 0 01-.275-.402 1.772 1.772 0 01-.155-.667 1.759 1.759 0 01.08-.668 1.43 1.43 0 01.283-.535c.128-.133.26-.2.418-.2zm1.37 1.706c.332 0 .733.065 1.216.399.293.2.523.269 1.052.468h.003c.255.136.405.266.478.399v-.131a.571.571 0 01.016.47c-.123.31-.516.643-1.063.842v.002c-.268.135-.501.333-.775.465-.276.135-.588.292-1.012.267a1.139 1.139 0 01-.448-.067 3.566 3.566 0 01-.322-.198c-.195-.135-.363-.332-.612-.465v-.005h-.005c-.4-.246-.616-.512-.686-.71-.07-.268-.005-.47.193-.6.224-.135.38-.271.483-.336.104-.074.143-.102.176-.131h.002v-.003c.169-.202.436-.47.839-.601.139-.036.294-.065.466-.065zm2.8 2.142c.358 1.417 1.196 3.475 1.735 4.473.286.534.855 1.659 1.102 3.024.156-.005.33.018.513.064.646-1.671-.546-3.467-1.089-3.966-.22-.2-.232-.335-.123-.335.59.534 1.365 1.572 1.646 2.757.13.535.16 1.104.021 1.67.067.028.135.06.205.067 1.032.534 1.413.938 1.23 1.537v-.043c-.06-.003-.12 0-.18 0h-.016c.151-.467-.182-.825-1.065-1.224-.915-.4-1.646-.336-1.77.465-.008.043-.013.066-.018.135-.068.023-.139.053-.209.064-.43.268-.662.669-.793 1.187-.13.533-.17 1.156-.205 1.869v.003c-.02.334-.17.838-.319 1.35-1.5 1.072-3.58 1.538-5.348.334a2.645 2.645 0 00-.402-.533 1.45 1.45 0 00-.275-.333c.182 0 .338-.03.465-.067a.615.615 0 00.314-.334c.108-.267 0-.697-.345-1.163-.345-.467-.931-.995-1.788-1.521-.63-.4-.986-.87-1.15-1.396-.165-.534-.143-1.085-.015-1.645.245-1.07.873-2.11 1.274-2.763.107-.065.037.135-.408.974-.396.751-1.14 2.497-.122 3.854a8.123 8.123 0 01.647-2.876c.564-1.278 1.743-3.504 1.836-5.268.048.036.217.135.289.202.218.133.38.333.59.465.21.201.477.335.876.335.039.003.075.006.11.006.412 0 .73-.134.997-.268.29-.134.52-.334.74-.4h.005c.467-.135.835-.402 1.044-.7zm2.185 8.958c.037.6.343 1.245.882 1.377.588.134 1.434-.333 1.791-.765l.211-.01c.315-.007.577.01.847.268l.003.003c.208.199.305.53.391.876.085.4.154.78.409 1.066.486.527.645.906.636 1.14l.003-.007v.018l-.003-.012c-.015.262-.185.396-.498.595-.63.401-1.746.712-2.457 1.57-.618.737-1.37 1.14-2.036 1.191-.664.053-1.237-.2-1.574-.898l-.005-.003c-.21-.4-.12-1.025.056-1.69.176-.668.428-1.344.463-1.897.037-.714.076-1.335.195-1.814.12-.465.308-.797.641-.984l.045-.022zm-10.814.049h.01c.053 0 .105.005.157.014.376.055.706.333 1.023.752l.91 1.664.003.003c.243.533.754 1.064 1.189 1.637.434.598.77 1.131.729 1.57v.006c-.057.744-.48 1.148-1.125 1.294-.645.135-1.52.002-2.395-.464-.968-.536-2.118-.469-2.857-.602-.369-.066-.61-.2-.723-.4-.11-.2-.113-.602.123-1.23v-.004l.002-.003c.117-.334.03-.752-.027-1.118-.055-.401-.083-.71.043-.94.16-.334.396-.4.69-.533.294-.135.64-.202.915-.47h.002v-.002c.256-.268.445-.601.668-.838.19-.201.38-.336.663-.336zm7.159-9.074c-.435.201-.945.535-1.488.535-.542 0-.97-.267-1.28-.466-.154-.134-.28-.268-.373-.335-.164-.134-.144-.333-.074-.333.109.016.129.134.199.2.096.066.215.2.36.333.292.2.68.467 1.167.467.485 0 1.053-.267 1.398-.466.195-.135.445-.334.648-.467.156-.136.149-.267.279-.267.128.016.034.134-.147.332a8.097 8.097 0 01-.69.468zm-1.082-1.583V5.64c-.006-.02.013-.042.029-.05.074-.043.18-.027.26.004.063 0 .16.067.15.135-.006.049-.085.066-.135.066-.055 0-.092-.043-.141-.068-.052-.018-.146-.008-.163-.065zm-.551 0c-.02.058-.113.049-.166.066-.047.025-.086.068-.14.068-.05 0-.13-.02-.136-.068-.01-.066.088-.133.15-.133.08-.031.184-.047.259-.005.019.009.036.03.03.05v.02h.003z",
            "",
        )],
    ),
    (
        "Android",
        &[(
            "M18.4395 5.5586c-.675 1.1664-1.352 2.3318-2.0274 3.498-.0366-.0155-.0742-.0286-.1113-.043-1.8249-.6957-3.484-.8-4.42-.787-1.8551.0185-3.3544.4643-4.2597.8203-.084-.1494-1.7526-3.021-2.0215-3.4864a1.1451 1.1451 0 0 0-.1406-.1914c-.3312-.364-.9054-.4859-1.379-.203-.475.282-.7136.9361-.3886 1.5019 1.9466 3.3696-.0966-.2158 1.9473 3.3593.0172.031-.4946.2642-1.3926 1.0177C2.8987 12.176.452 14.772 0 18.9902h24c-.119-1.1108-.3686-2.099-.7461-3.0683-.7438-1.9118-1.8435-3.2928-2.7402-4.1836a12.1048 12.1048 0 0 0-2.1309-1.6875c.6594-1.122 1.312-2.2559 1.9649-3.3848.2077-.3615.1886-.7956-.0079-1.1191a1.1001 1.1001 0 0 0-.8515-.5332c-.5225-.0536-.9392.3128-1.0488.5449zm-.0391 8.461c.3944.5926.324 1.3306-.1563 1.6503-.4799.3197-1.188.0985-1.582-.4941-.3944-.5927-.324-1.3307.1563-1.6504.4727-.315 1.1812-.1086 1.582.4941zM7.207 13.5273c.4803.3197.5506 1.0577.1563 1.6504-.394.5926-1.1038.8138-1.584.4941-.48-.3197-.5503-1.0577-.1563-1.6504.4008-.6021 1.1087-.8106 1.584-.4941z",
            "",
        )],
    ),
    (
        "iOS",
        &[
            (
                "M8 1h8a3 3 0 0 1 3 3v16a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3V4a3 3 0 0 1 3-3zm0 1.6A1.4 1.4 0 0 0 6.6 4v16A1.4 1.4 0 0 0 8 21.4h8a1.4 1.4 0 0 0 1.4-1.4V4A1.4 1.4 0 0 0 16 2.6z",
                "",
            ),
            (APPLE, "translate(8.2 7.6) scale(0.32)"),
        ],
    ),
];

/// The Apple logo (Simple Icons, CC0).
const APPLE: &str = "M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701";

/// (figure, what it counts)
const HERO_STATS: [(&str, &str); 4] = [
    ("43", "BIR forms, every one"),
    ("2,007", "official validation rules"),
    ("30 days", "of deadlines in view"),
    ("Offline", "after one sign-in"),
];

pub(super) const TITLE: &str = "Buwiz: Philippine tax filing, made modern";
pub(super) const DESCRIPTION: &str = "Prepare and file your BIR returns from your computer, with your own data. Buwiz handles 1701Q, 2550Q and more, and lets you check every return before it's sent.";

/// Who runs the site and what it is, for search engines (schema.org).
/// Only what we can stand behind: no ratings, prices or social profiles.
const JSON_LD: &str = r#"[{"@context":"https://schema.org","@type":"Organization","name":"Buwiz","url":"https://buwiz.com","logo":"https://buwiz.com/assets/brand/icon-512.png"},{"@context":"https://schema.org","@type":"WebSite","name":"Buwiz","url":"https://buwiz.com"}]"#;

/// The home page's share card, canonical address and JSON-LD, in the
/// layout's `<head>`. Always the public address, whichever host served it.
#[component]
pub(super) async fn home_head() -> Result<impl View> {
    Ok(view! {
        <link rel="canonical" href="https://buwiz.com/">
        <meta property="og:type" content="website">
        <meta property="og:site_name" content="Buwiz">
        <meta property="og:locale" content="en_PH">
        <meta property="og:url" content="https://buwiz.com/">
        <meta property="og:title" content=(TITLE)>
        <meta property="og:description" content=(DESCRIPTION)>
        <meta property="og:image" content="https://buwiz.com/assets/brand/og-image.png">
        <meta property="og:image:width" content="1200">
        <meta property="og:image:height" content="630">
        <meta property="og:image:alt" content="Buwiz: Philippine tax filing, made modern">
        <meta name="twitter:card" content="summary_large_image">
        <meta name="twitter:title" content=(TITLE)>
        <meta name="twitter:description" content=(DESCRIPTION)>
        <meta name="twitter:image" content="https://buwiz.com/assets/brand/og-image.png">
        // Text escaping leaves this JSON as it is: it has no `&`, `<` or `>`.
        <script type="application/ld+json">(JSON_LD)</script>
    })
}

#[page("/")]
pub async fn home_page(cx: &Cx) -> Result<impl View> {
    let signed_in = current(cx).await?.is_some();
    Ok(view! {
        <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@500;700&display=swap">
        <link rel="stylesheet" href="/assets/demo.css">
        <main class="home landing">
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
            <a class="btn btn-solid" href="/tins">icon(name: "badge") "Open My TINs"</a>
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
                    <div class="platforms rise d3">
                        <span class="soon-tag">"Coming soon"</span>
                        <ul aria-label="Where Buwiz will run">
                            for (name, paths) in PLATFORMS {
                                <li class="platform" title=(format!("{name}, coming soon"))>
                                    <svg viewBox="0 0 24 24" fill="currentColor" role="img" aria-label=(format!("{name}, coming soon"))>
                                        for (d, transform) in paths {
                                            <path d=(*d) transform=(*transform)></path>
                                        }
                                    </svg>
                                </li>
                            }
                        </ul>
                    </div>
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
