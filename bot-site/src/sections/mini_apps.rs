use leptos::{ev, html, prelude::*};

use crate::components::agent_window::{AgentScreen, AgentWindow, FINANCE, FINANCE_RAIL};
use crate::components::demo_player::{
    DemoControls, DemoCursor, DemoPointer, PlayState, Script, SpeedChips, Timeline, autoplay_when_visible,
};

/// A Buwiz mini app: a signed plugin that runs inside Buwiz Bot, in a chat or full screen.
pub struct MiniApp {
    pub name: &'static str,
    /// The plugin id.
    pub id: &'static str,
    /// One line on what it does.
    pub line: &'static str,
    /// A Material Symbols name.
    pub icon: &'static str,
    /// Icon tile background, a Tailwind class.
    pub tint: &'static str,
}

pub const APPS: [MiniApp; 4] = [
    MiniApp {
        name: "eBIRForms",
        id: "app.buwiz.ebirforms",
        line: "Fill, check and print BIR returns.",
        icon: "assignment",
        tint: "bg-[#0f766e]",
    },
    MiniApp {
        name: "Books",
        id: "app.buwiz.books",
        line: "Bookkeeping and books of accounts.",
        icon: "menu_book",
        tint: "bg-[#b45309]",
    },
    MiniApp {
        name: "Word",
        id: "app.buwiz.word",
        line: "Native, offline .docx editing.",
        icon: "article",
        tint: "bg-[#3b5bdb]",
    },
    MiniApp {
        name: "Spreadsheet",
        id: "app.buwiz.spreadsheet",
        line: "Native, offline .xlsx editing.",
        icon: "table_chart",
        tint: "bg-[#2f9e44]",
    },
];

// The Mini apps demo, played in a chat with Finance. Each tab is one app: the visitor asks in
// plain words, Finance answers with the app as a card in the thread, then opens it full screen
// (the screen grows out of the pane to fill the window) and closes it again. A cursor drives it;
// a click on a tab plays from that app, a click on a card's button takes over.
//
// Every control is a drawn box, never a real input.
const FORMS: usize = 0;
const BOOKS: usize = 1;
const WORD: usize = 2;

const ASKS: [&str; 4] = [
    "File my 1601-C for October",
    "Record this receipt",
    "Draft a payment reminder letter for Dela Cruz Trading",
    "Sum October sales by client",
];
const REPLIES: [&str; 4] = [
    "Filled from your October payroll and checked against the eBIRForms rules. 0 errors.",
    "Recorded in your cash disbursements journal.",
    "Here's a draft. Edit it here, or open it full screen.",
    "Summed by client, with a total row.",
];
const CAPTIONS: [&str; 4] = [
    "eBIRForms fills the return from your own data, checks it, then queues or prints it.",
    "Books keeps your books of accounts, the same books your BIR forms are filled from.",
    "Word edits .docx natively, offline. Open it full screen and keep typing.",
    "Spreadsheet edits .xlsx natively, offline.",
];
const TITLES: [&str; 4] =
    ["BIR Form 1601-C · October 2026", "Cash disbursements journal", "Payment reminder.docx", "October sales.xlsx"];
const RECEIPT: &str = "receipt-oct28.jpg";
/// What the cursor types into the letter, full screen.
const ADDED: &str = "Thank you for your business.";
const YOUR_TURN: &str = "Your turn: tap a tab, or open the app in the chat. Or Replay.";

/// How long an app stays full screen.
const FULL_HOLD: u32 = 2400;

const BTN: &str = "cursor-pointer rounded-full bg-gc-text px-3.5 py-1.5 text-[13px] font-semibold text-gc-bg outline-none focus-visible:ring-2 focus-visible:ring-gc-brand focus-visible:ring-offset-2 focus-visible:ring-offset-gc-bg-card";
const BTN_QUIET: &str = "cursor-pointer rounded-full border border-gc-border px-3.5 py-1.5 text-[13px] font-semibold text-gc-text outline-none hover:border-gc-brand/40 focus-visible:ring-2 focus-visible:ring-gc-brand";
const CARD: &str = "gc-card-in grid gap-3 rounded-2xl border border-gc-border bg-gc-bg-card p-3.5";
const BADGE: &str =
    "rounded-[5px] border border-gc-accent/50 px-1.5 font-mono text-[10px] font-semibold text-gc-accent";

const TAB_ON: &str = "inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-gc-text bg-gc-text px-3 py-1 text-[12px] font-semibold text-gc-bg";
const TAB_OFF: &str = "inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-gc-border px-3 py-1 text-[12px] text-gc-text-muted hover:border-gc-brand/40 hover:text-gc-text";

/// Sample books: (date, payee, reference, account, amount). The last is the receipt recorded.
const JOURNAL: [(&str, &str, &str, &str, &str); 4] = [
    ("Oct 03", "Lungsod Realty", "CV-0412", "Rent expense", "25,000.00"),
    ("Oct 10", "Bayani Internet", "CV-0413", "Utilities", "2,499.00"),
    ("Oct 21", "Juan Santos", "CV-0414", "Professional fees", "8,000.00"),
    ("Oct 28", "Mabuhay Office Supply", "OR 004821", "Office supplies", "1,850.00"),
];
const JOURNAL_TOTAL: &str = "37,349.00";

/// Sample sales by client: (client, invoices, amount).
const SALES: [(&str, &str, &str); 4] = [
    ("Dela Cruz Trading", "3", "84,000.00"),
    ("Santos Bakery", "2", "36,500.00"),
    ("Reyes Logistics", "4", "128,250.00"),
    ("Lim Hardware", "1", "18,900.00"),
];
const SALES_TOTAL: (&str, &str) = ("10", "267,650.00");

/// Sample 1601-C lines: (label, amount).
const RETURN: [(&str, &str); 4] = [
    ("Total compensation", "412,500.00"),
    ("Non-taxable compensation", "96,000.00"),
    ("Taxable compensation", "316,500.00"),
    ("Tax withheld", "38,215.40"),
];

const LETTER_TO: [&str; 3] = ["Dela Cruz Trading", "Attn: Accounts Payable", "Re: Invoice INV-1043"];
const LETTER_BODY: &str = "This is a friendly reminder that Invoice INV-1043 for ₱28,000.00, due on October 15, 2026, is still open. Please settle it by November 7, 2026, or let us know if you have already paid.";

#[derive(Clone)]
struct Msg {
    id: u32,
    body: Body,
}

#[derive(Clone)]
enum Body {
    /// The ask, and a file sent with it.
    Me(&'static str, Option<&'static str>),
    Finance(&'static str),
    /// The app, as a card in the thread.
    App(usize),
}

#[derive(Clone, Copy)]
struct Demo {
    tl: Timeline,
    cur: DemoCursor,
    msgs: RwSignal<Vec<Msg>>,
    next: StoredValue<u32>,
    /// The app on show.
    app: RwSignal<usize>,
    caption: RwSignal<&'static str>,
    /// The return is queued to send.
    queued: RwSignal<bool>,
    /// What's been typed into the letter.
    typed: RwSignal<&'static str>,
    /// The app the next play starts from (a tab click).
    from: StoredValue<usize>,
    screen: AgentScreen,
    scroller: NodeRef<html::Div>,
    // Where the cursor goes.
    open_btn: NodeRef<html::Div>,
    queue_btn: NodeRef<html::Div>,
}

impl Demo {
    fn reset(self) {
        self.msgs.set(Vec::new());
        self.queued.set(false);
        self.typed.set("");
        self.screen.reset();
        self.cur.reset();
    }

    fn push(self, body: Body) {
        let id = self
            .next
            .try_update_value(|n| {
                *n += 1;
                *n
            })
            .unwrap_or_default();
        self.msgs.update(|v| v.push(Msg { id, body }));
        let scroller = self.scroller;
        request_animation_frame(move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    /// A fresh chat for `app`.
    fn show(self, app: usize) {
        self.reset();
        self.app.set(app);
        self.caption.set(CAPTIONS[app]);
    }

    fn ask(self, app: usize) {
        self.push(Body::Me(ASKS[app], (app == BOOKS).then_some(RECEIPT)));
    }

    fn answer(self, app: usize) {
        self.push(Body::Finance(REPLIES[app]));
        self.push(Body::App(app));
    }

    /// A visitor's click. Mid-run it ends the autoplay and hands the chat over.
    fn by_hand(self) -> bool {
        match self.tl.state.get_untracked() {
            PlayState::Idle => false,
            PlayState::Done => true,
            PlayState::Playing | PlayState::Paused => {
                self.tl.stop();
                self.cur.reset();
                self.screen.reset();
                self.caption.set(YOUR_TURN);
                true
            }
        }
    }

    /// Plays from `app` on.
    fn play_from(self, app: usize) {
        self.from.set_value(app);
        self.tl.play();
    }
}

/// Enter or Space on a drawn button.
fn activates(e: &ev::KeyboardEvent) -> bool {
    let hit = matches!(e.key().as_str(), "Enter" | " ");
    if hit {
        e.prevent_default();
    }
    hit
}

pub fn app_icon(app: &MiniApp, size: &'static str, glyph: &'static str) -> impl IntoView {
    view! {
        <span class=format!("grid shrink-0 place-items-center rounded-lg text-white {size} {}", app.tint)>
            <span class=format!("material-symbols-outlined {glyph}")>{app.icon}</span>
        </span>
    }
}

/// A drawn button: `f` on click, Enter or Space.
fn button(
    class: &'static str,
    aim: NodeRef<html::Div>,
    label: &'static str,
    f: impl Fn() + Clone + 'static,
) -> impl IntoView {
    let f2 = f.clone();
    view! {
        <div
            node_ref=aim
            role="button"
            tabindex="0"
            class=class
            on:click=move |_| f()
            on:keydown=move |e| { if activates(&e) { f2() } }
        >
            {label}
        </div>
    }
}

/// One row of a small table: the first `text` cells words, the rest figures.
fn row(cols: &'static str, text: usize, cells: [&'static str; 3], class: &'static str) -> impl IntoView {
    view! {
        <div class=format!("grid {cols} gap-2 px-2.5 py-1.5 {class}")>
            {cells.into_iter().enumerate().map(|(i, c)| view! {
                <span class=if i < text { "truncate" } else { "truncate text-right font-mono tabular-nums" }>{c}</span>
            }).collect_view()}
        </div>
    }
}

/// What the app shows in its card, in the thread.
fn preview(app: usize) -> AnyView {
    match app {
        FORMS => view! {
            <div class="grid gap-1.5 rounded-xl border border-gc-border bg-gc-bg-elevated p-3 text-[12.5px]">
                <div class="flex flex-wrap items-baseline gap-x-2 text-gc-text">
                    <span class="font-semibold">"BIR Form 1601-C"</span>
                    <span class="text-gc-text-muted">"October 2026"</span>
                </div>
                {RETURN[2..].iter().map(|(label, amount)| view! {
                    <div class="flex justify-between gap-3 text-gc-text-muted">
                        <span>{*label}</span>
                        <span class="font-mono tabular-nums text-gc-text">"₱"{*amount}</span>
                    </div>
                }).collect_view()}
            </div>
        }
        .into_any(),
        BOOKS => {
            let (date, payee, _, _, amount) = JOURNAL[3];
            view! {
                <div class="overflow-hidden rounded-xl border border-gc-border bg-gc-bg-elevated text-[12.5px]">
                    <div class="border-b border-gc-border px-2.5 py-1.5 font-semibold text-gc-text">"Cash disbursements journal · October 2026"</div>
                    {JOURNAL[1..3].iter().map(|(date, payee, _, _, amount)| row("grid-cols-[52px_1fr_auto]", 2, [date, payee, amount], "text-gc-text-muted")).collect_view()}
                    {row("grid-cols-[52px_1fr_auto]", 2, [date, payee, amount], "gc-card-in bg-gc-accent/10 font-semibold text-gc-text")}
                </div>
            }
            .into_any()
        }
        WORD => view! {
            <div class="rounded-xl border border-gc-border bg-gc-bg-elevated p-3">
                <div class="mx-auto max-h-[112px] max-w-[360px] overflow-hidden rounded-[4px] bg-white px-5 py-4 text-[10.5px] leading-[1.5] text-[#1f2328] shadow-sm">
                    <p>"October 31, 2026"</p>
                    <p class="mt-1.5">{LETTER_TO[0]}<br />{LETTER_TO[1]}</p>
                    <p class="mt-1.5 font-semibold">{LETTER_TO[2]}</p>
                    <p class="mt-1.5">"Dear Ms. Dela Cruz,"</p>
                    <p>{LETTER_BODY}</p>
                </div>
            </div>
        }
        .into_any(),
        _ => view! {
            <div class="overflow-hidden rounded-xl border border-gc-border bg-gc-bg-elevated text-[12.5px]">
                <div class="border-b border-gc-border px-2.5 py-1.5 font-semibold text-gc-text">"October sales.xlsx"</div>
                {SALES.iter().map(|(client, n, amount)| row("grid-cols-[1fr_40px_auto]", 1, [client, n, amount], "text-gc-text-muted")).collect_view()}
                {row("grid-cols-[1fr_40px_auto]", 1, ["Total", SALES_TOTAL.0, SALES_TOTAL.1], "gc-card-in border-t border-gc-border bg-gc-accent/10 font-semibold text-gc-text")}
            </div>
        }
        .into_any(),
    }
}

/// The app as a card in the thread: its name and id, what it made, and its buttons.
fn app_card(d: Demo, app: usize) -> impl IntoView {
    let a = &APPS[app];
    let open = move || {
        if d.by_hand() {
            d.app.set(app);
            d.screen.expand();
        }
    };
    let queue = move || {
        if !d.queued.get_untracked() && d.by_hand() {
            d.queued.set(true);
        }
    };
    let print = move || {
        d.by_hand();
    };
    view! {
        <div class=CARD>
            <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                {app_icon(a, "h-7 w-7", "text-[17px]")}
                <span class="text-[13.5px] font-semibold text-gc-text">{a.name}</span>
                <span class=BADGE>"signed"</span>
                <span class="ml-auto font-mono text-[11px] text-gc-text-faint">{a.id}</span>
            </div>
            {preview(app)}
            <div class="flex flex-wrap items-center gap-2">
                {if app == FORMS {
                    view! {
                        <span class="mr-auto inline-flex items-center gap-1.5 text-[12.5px] font-semibold text-gc-accent">
                            <span class="material-symbols-outlined text-[16px]">"check_circle"</span>
                            "0 errors"
                        </span>
                    }.into_any()
                } else {
                    view! { <span class="mr-auto"></span> }.into_any()
                }}
                {button(BTN_QUIET, d.open_btn, "Open full screen", open)}
                {(app == FORMS).then(|| view! {
                    {button(BTN_QUIET, NodeRef::new(), "Print", print)}
                    {move || if d.queued.get() {
                        view! { <span class="px-2 text-[13px] font-semibold text-gc-accent">"✓ Queued to send"</span> }.into_any()
                    } else {
                        button(BTN, d.queue_btn, "Queue to send", queue).into_any()
                    }}
                })}
            </div>
        </div>
    }
}

fn message(d: Demo, m: Msg) -> AnyView {
    match m.body {
        Body::Me(text, file) => view! {
            <div class="gc-card-in flex max-w-[88%] flex-col items-end gap-1.5 self-end">
                {file.map(|f| view! {
                    <span class="inline-flex items-center gap-1.5 rounded-xl border border-gc-border bg-gc-bg-elevated px-2.5 py-1.5 text-[12.5px] text-gc-text">
                        <span class="material-symbols-outlined text-[16px] text-gc-text-muted">"receipt_long"</span>
                        {f}
                    </span>
                })}
                <div class="rounded-[20px] bg-gc-text px-4 py-2.5 text-gc-bg">{text}</div>
            </div>
        }
        .into_any(),
        Body::Finance(text) => view! {
            <div class="gc-card-in max-w-[88%] self-start rounded-2xl bg-gc-bg-muted px-3.5 py-2.5 text-gc-text">{text}</div>
        }
        .into_any(),
        Body::App(app) => app_card(d, app).into_any(),
    }
}

/// The app full screen, laid out at the screen's real size. Light, like a document.
fn full_screen(d: Demo) -> impl IntoView {
    let close = move || {
        if d.screen.is_open() && d.by_hand() {
            d.screen.shrink();
        }
    };
    move || {
        let app = d.app.get();
        let a = &APPS[app];
        view! {
            <div class="flex h-full w-full flex-col overflow-hidden bg-[#f6f7f9] text-left text-[13px] text-[#1f2328]">
                <div class="flex h-11 shrink-0 items-center gap-2.5 border-b border-[#e3e5e8] bg-white px-4">
                    {app_icon(a, "h-7 w-7", "text-[17px]")}
                    <span class="font-semibold">{a.name}</span>
                    <span class="truncate text-[#6b7280]">{TITLES[app]}</span>
                    <span class="ml-auto hidden shrink-0 font-mono text-[11px] text-[#9aa0a6] sm:inline">{a.id}" · signed"</span>
                    <div
                        role="button"
                        tabindex="0"
                        aria-label="Close full screen"
                        class="ml-auto grid h-7 w-7 shrink-0 cursor-pointer place-items-center rounded-md sm:ml-0 text-[#5f6368] hover:bg-[#eef0f2]"
                        on:click=move |_| close()
                        on:keydown=move |e| { if activates(&e) { close() } }
                    >
                        <span class="material-symbols-outlined text-[18px]">"close_fullscreen"</span>
                    </div>
                </div>
                <div class="relative min-h-0 flex-1">{full_body(d, app)}</div>
            </div>
        }
    }
}

fn side_list(items: &'static [&'static str], on: usize) -> impl IntoView {
    view! {
        <div class="hidden w-[190px] shrink-0 flex-col gap-0.5 border-r border-[#e3e5e8] bg-white p-3 sm:flex">
            {items.iter().enumerate().map(|(i, s)| view! {
                <span class=if i == on {
                    "rounded-md bg-[#e8f5f1] px-2.5 py-1.5 font-semibold text-[#0f5b46]"
                } else {
                    "rounded-md px-2.5 py-1.5 text-[#4b5563]"
                }>{*s}</span>
            }).collect_view()}
        </div>
    }
}

const JOURNAL_COLS: &str =
    "grid grid-cols-[52px_1fr_84px] gap-3 px-3 py-2 sm:grid-cols-[64px_1.4fr_96px_1.1fr_110px] sm:px-4";
/// Columns the narrow journal leaves out.
const WIDE_ONLY: &str = "hidden sm:block";
const SHEET_COLS: &str = "grid grid-cols-[32px_1.6fr_0.8fr_1.2fr] sm:grid-cols-[40px_1.6fr_0.8fr_1.1fr_0.7fr_0.7fr]";
const SHEET_CELL: &str = "truncate border-b border-r border-[#e3e5e8] px-2 py-1.5";
const SHEET_HEAD: &str =
    "border-b border-r border-[#e3e5e8] bg-[#f1f3f4] px-2 py-1 text-center text-[11.5px] text-[#6b7280]";

fn offline_saved() -> impl IntoView {
    view! {
        <span class="ml-auto inline-flex items-center gap-1 font-sans text-[12px] text-[#6b7280]">
            <span class="material-symbols-outlined text-[15px]">"cloud_off"</span>
            "Offline · saved"
        </span>
    }
}

fn full_body(d: Demo, app: usize) -> AnyView {
    match app {
        FORMS => view! {
            <div class="flex h-full">
                {side_list(&["1701Q", "2550Q", "1601-C", "1601-EQ", "0619-E"], 2)}
                <div class="flex min-w-0 flex-1 flex-col gap-4 p-3 sm:p-5">
                    <div class="rounded-lg border border-[#e3e5e8] bg-white">
                        <div class="border-b border-[#e3e5e8] px-4 py-2.5">
                            <div class="font-bold">"BIR Form 1601-C"</div>
                            <div class="text-[12px] text-[#6b7280]">"Monthly Remittance Return of Income Taxes Withheld on Compensation"</div>
                        </div>
                        <div class="grid grid-cols-1 gap-3 border-b border-[#e3e5e8] px-4 py-3 text-[12px] sm:grid-cols-3">
                            {[("For the month", "10 / 2026"), ("TIN", "123-456-789-000"), ("Withholding agent", "Mabini Design Studio")].into_iter().map(|(k, v)| view! {
                                <div class="min-w-0">
                                    <div class="text-[#6b7280]">{k}</div>
                                    <div class="mt-0.5 truncate rounded border border-[#d0d4d9] bg-[#fafbfc] px-2 py-1 font-mono">{v}</div>
                                </div>
                            }).collect_view()}
                        </div>
                        <div class="px-4 py-2">
                            {RETURN.iter().enumerate().map(|(i, (label, amount))| view! {
                                <div class=if i == RETURN.len() - 1 { "flex justify-between py-1.5 font-semibold" } else { "flex justify-between border-b border-[#f0f1f3] py-1.5" }>
                                    <span>{*label}</span>
                                    <span class="font-mono tabular-nums">"₱ "{*amount}</span>
                                </div>
                            }).collect_view()}
                        </div>
                    </div>
                    <div class="mt-auto flex flex-wrap items-center gap-2 rounded-lg border border-[#cfe9df] bg-[#effaf5] px-4 py-2.5">
                        <span class="material-symbols-outlined text-[18px] text-[#0f7b5a]">"check_circle"</span>
                        <span class="font-semibold text-[#0f7b5a]">"0 errors"</span>
                        <span class="hidden text-[#4b5563] sm:inline">"Checked against the eBIRForms rules"</span>
                        <span class="ml-auto rounded-full border border-[#d0d4d9] bg-white px-3 py-1 font-semibold">"Print"</span>
                        <span class="rounded-full bg-[#1f2328] px-3 py-1 font-semibold text-white">
                            {move || if d.queued.get() { "✓ Queued" } else { "Queue to send" }}
                        </span>
                    </div>
                </div>
            </div>
        }
        .into_any(),
        BOOKS => view! {
            <div class="flex h-full">
                {side_list(&["Cash receipts", "Cash disbursements", "Sales journal", "General journal", "General ledger"], 1)}
                <div class="min-w-0 flex-1 p-3 sm:p-5">
                    <div class="mb-3 font-bold">"Cash disbursements journal · October 2026"</div>
                    <div class="overflow-hidden rounded-lg border border-[#e3e5e8] bg-white">
                        <div class=format!("{JOURNAL_COLS} border-b border-[#e3e5e8] bg-[#fafbfc] text-[12px] font-semibold text-[#6b7280]")>
                            <span>"Date"</span><span>"Payee"</span><span class=WIDE_ONLY>"Ref"</span><span class=WIDE_ONLY>"Account"</span><span class="text-right">"Cash (Cr)"</span>
                        </div>
                        {JOURNAL.iter().enumerate().map(|(i, (date, payee, r, account, amount))| view! {
                            <div class=if i == JOURNAL.len() - 1 {
                                format!("{JOURNAL_COLS} border-b border-[#e3e5e8] bg-[#effaf5] font-semibold")
                            } else {
                                format!("{JOURNAL_COLS} border-b border-[#f0f1f3]")
                            }>
                                <span>{*date}</span>
                                <span class="truncate">{*payee}</span>
                                <span class="hidden truncate font-mono text-[12px] sm:block">{*r}</span>
                                <span class="hidden truncate sm:block">{*account}</span>
                                <span class="text-right font-mono tabular-nums">{*amount}</span>
                            </div>
                        }).collect_view()}
                        <div class=format!("{JOURNAL_COLS} font-bold")>
                            <span></span><span>"Total"</span><span class=WIDE_ONLY></span><span class=WIDE_ONLY></span>
                            <span class="text-right font-mono tabular-nums">{JOURNAL_TOTAL}</span>
                        </div>
                    </div>
                </div>
            </div>
        }
        .into_any(),
        WORD => view! {
            <div class="flex h-full flex-col">
                <div class="flex h-9 shrink-0 items-center gap-3 border-b border-[#e3e5e8] bg-white px-4 text-[12.5px] text-[#4b5563]">
                    <span class="rounded border border-[#d0d4d9] px-2 py-0.5">"Normal"</span>
                    <span class="rounded border border-[#d0d4d9] px-2 py-0.5">"Serif · 11"</span>
                    <span class="font-bold">"B"</span>
                    <span class="italic">"I"</span>
                    <span class="underline">"U"</span>
                    {offline_saved()}
                </div>
                <div class="min-h-0 flex-1 overflow-hidden px-3 pt-4 sm:px-6 sm:pt-5">
                    <div class="mx-auto h-full max-w-[600px] bg-white px-6 py-6 font-serif sm:px-12 sm:py-8 text-[13.5px] leading-[1.55] shadow-[0_2px_10px_rgba(0,0,0,.08)]">
                        <p>"October 31, 2026"</p>
                        <p class="mt-3">{LETTER_TO[0]}<br />{LETTER_TO[1]}</p>
                        <p class="mt-3 font-semibold">{LETTER_TO[2]}</p>
                        <p class="mt-3">"Dear Ms. Dela Cruz,"</p>
                        <p class="mt-1">{LETTER_BODY}</p>
                        <p class="mt-3 min-h-[1.55em]">{move || d.typed.get()}<span class="gc-caret !bg-[#1f2328]"></span></p>
                        <p class="mt-3">"Sincerely,"<br />"Mabini Design Studio"</p>
                    </div>
                </div>
            </div>
        }
        .into_any(),
        _ => view! {
            <div class="flex h-full flex-col bg-white">
                <div class="flex h-9 shrink-0 items-center gap-2 border-b border-[#e3e5e8] px-3 font-mono text-[12.5px]">
                    <span class="rounded border border-[#d0d4d9] px-2 py-0.5 text-[#4b5563]">"C6"</span>
                    <span class="italic text-[#9aa0a6]">"fx"</span>
                    <span>"=SUM(C2:C5)"</span>
                    {offline_saved()}
                </div>
                <div class="min-h-0 flex-1 overflow-hidden">
                    <div class=SHEET_COLS>
                        {["", "A", "B", "C"].into_iter().map(|h| view! { <span class=SHEET_HEAD>{h}</span> }).collect_view()}
                        {["D", "E"].into_iter().map(|h| view! { <span class=format!("{SHEET_HEAD} {WIDE_ONLY}")>{h}</span> }).collect_view()}
                    </div>
                    <div class=SHEET_COLS>
                        <span class=SHEET_HEAD>"1"</span>
                        {["Client", "Invoices", "Amount"].into_iter().map(|h| view! { <span class=format!("{SHEET_CELL} font-semibold")>{h}</span> }).collect_view()}
                        <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                        <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                    </div>
                    {SALES.iter().enumerate().map(|(i, (client, n, amount))| view! {
                        <div class=SHEET_COLS>
                            <span class=SHEET_HEAD>{format!("{}", i + 2)}</span>
                            <span class=SHEET_CELL>{*client}</span>
                            <span class=format!("{SHEET_CELL} text-right font-mono tabular-nums")>{*n}</span>
                            <span class=format!("{SHEET_CELL} text-right font-mono tabular-nums")>{*amount}</span>
                            <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                            <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                        </div>
                    }).collect_view()}
                    <div class=SHEET_COLS>
                        <span class=SHEET_HEAD>"6"</span>
                        <span class=format!("{SHEET_CELL} font-bold")>"Total"</span>
                        <span class=format!("{SHEET_CELL} text-right font-mono font-bold tabular-nums")>{SALES_TOTAL.0}</span>
                        <span class=format!("{SHEET_CELL} bg-[#effaf5] text-right font-mono font-bold tabular-nums ring-2 ring-inset ring-[#2f9e44]")>{SALES_TOTAL.1}</span>
                        <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                        <span class=format!("{SHEET_CELL} {WIDE_ONLY}")></span>
                    </div>
                    {(7..=14).map(|n| view! {
                        <div class=SHEET_COLS>
                            <span class=SHEET_HEAD>{format!("{n}")}</span>
                            {(0..5).map(|i| view! { <span class=format!("{SHEET_CELL} h-[30px] {}", if i > 2 { WIDE_ONLY } else { "" })></span> }).collect_view()}
                        </div>
                    }).collect_view()}
                </div>
            </div>
        }
        .into_any(),
    }
}

/// One app's run: ask, the card, full screen and back.
fn scene(s: &mut Script, d: Demo, app: usize, composer: NodeRef<html::Div>, draft: RwSignal<&'static str>) {
    let cur = d.cur;
    s.then(move || {
        d.show(app);
        draft.set("");
    })
    .wait(350)
    .move_to(cur, composer)
    .typing(ASKS[app], 24, move |t| draft.set(t))
    .wait(250)
    .then(move || {
        draft.set("");
        d.ask(app);
    })
    .wait(650)
    .then(move || d.answer(app))
    .wait(1300)
    .click(cur, d.open_btn, move || d.screen.expand())
    .wait(450);
    if app == WORD {
        s.move_home(cur).typing(ADDED, 34, move |t| d.typed.set(t)).wait(900);
    } else {
        s.wait(FULL_HOLD);
    }
    s.then(move || d.screen.shrink()).wait(650);
    if app == FORMS {
        s.click(cur, d.queue_btn, move || d.queued.set(true)).wait(1100);
    } else {
        s.wait(700);
    }
}

fn point(title: &'static str, text: &'static str) -> impl IntoView {
    view! {
        <div class="grid content-start gap-1 rounded-2xl border border-gc-border bg-gc-bg p-4 text-[13.5px]">
            <span class="text-[15px] font-bold text-gc-text">{title}</span>
            <span class="text-gc-text-muted">{text}</span>
        </div>
    }
}

/// The Buwiz mini apps, opened by a coworker in the chat and full screen.
#[component]
pub fn MiniApps() -> impl IntoView {
    let win = NodeRef::<html::Div>::new();
    let composer = NodeRef::<html::Div>::new();
    let draft = RwSignal::new("");

    let tl = Timeline::new();
    let cur = DemoCursor::new(tl, win, (0.72, 0.8));
    let d = Demo {
        tl,
        cur,
        msgs: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
        app: RwSignal::new(FORMS),
        caption: RwSignal::new(CAPTIONS[FORMS]),
        queued: RwSignal::new(false),
        typed: RwSignal::new(""),
        from: StoredValue::new(FORMS),
        screen: AgentScreen::new(),
        scroller: NodeRef::new(),
        open_btn: NodeRef::new(),
        queue_btn: NodeRef::new(),
    };

    tl.script(move |s| {
        let from = d.from.get_value();
        d.from.set_value(FORMS);
        for app in from..APPS.len() {
            scene(s, d, app, composer, draft);
        }
        s.then(move || d.caption.set(YOUR_TURN)).move_home(cur);
    });

    autoplay_when_visible(tl, win);

    view! {
        <section id="mini-apps" class="relative py-24 md:py-32 border-t border-gc-border bg-gc-bg-card/40">
            <div class="max-w-[1128px] mx-auto px-6">
                <div class="max-w-2xl mb-10 md:mb-12">
                    <h2 class="text-3xl md:text-5xl font-black tracking-tighter text-gc-text leading-[1.08] mb-4">
                        "Mini apps"
                    </h2>
                    <p class="text-lg text-gc-text-muted max-w-[60ch]">
                        "Apps your coworker opens for you, right in the chat. Signed, local, yours."
                    </p>
                </div>

                <div class="flex flex-wrap items-center gap-2 mb-5">
                    <div class="flex flex-wrap gap-2" role="tablist" aria-label="Mini apps">
                        {APPS.iter().enumerate().map(|(i, a)| {
                            let on = move || d.app.get() == i;
                            view! {
                                <button
                                    type="button"
                                    role="tab"
                                    aria-selected=move || on().to_string()
                                    class=move || if on() { TAB_ON } else { TAB_OFF }
                                    on:click=move |_| d.play_from(i)
                                >
                                    <span class="material-symbols-outlined text-[15px]">{a.icon}</span>
                                    {a.name}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    <div class="ml-auto flex flex-wrap items-center gap-2">
                        <SpeedChips timeline=tl />
                        <DemoControls timeline=tl />
                    </div>
                    <p class="basis-full min-h-5 pt-1 text-[13px] text-gc-text-muted" aria-live="polite">{move || d.caption.get()}</p>
                </div>

                <AgentWindow
                    win
                    screen=d.screen
                    who=FINANCE
                    rail=FINANCE_RAIL
                    scroller=d.scroller
                    composer
                    draft
                    desktop=move || full_screen(d)
                    routine=("Month-end close", "Monthly")
                    overlay=move || view! { <DemoPointer cursor=cur /> }
                >
                    <For each=move || d.msgs.get() key=|m| m.id children=move |m| message(d, m) />
                </AgentWindow>

                <div class="mt-6 grid grid-cols-1 gap-3 md:grid-cols-3">
                    {point("In the chat", "Ask in plain words. The app opens as a card in the thread, already filled in.")}
                    {point("Or full screen", "Open it full screen when you need room. Close it and you're back in the chat.")}
                    {point("Signed plugins", "Each mini app is a signed plugin with its own id, and runs inside Buwiz Bot.")}
                </div>

                <a href="/mini-apps" class="mt-8 inline-flex items-center gap-2 text-sm font-semibold text-gc-text hover:text-gc-brand transition-colors">
                    "See all mini apps"
                    <span class="material-symbols-outlined text-[16px]">"arrow_forward"</span>
                </a>
            </div>
        </section>
    }
}
