//! Drawn controls for the demos: a toggle, a select pill, and a dropdown menu. None of them is a
//! real form control, so a visitor's password manager or autofill has nothing to wake up for.

use leptos::{html, prelude::*};

/// An on/off switch.
#[component]
pub fn Toggle(#[prop(into)] on: Signal<bool>, #[prop(optional)] aim: NodeRef<html::Div>) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || if on.get() {
                "relative shrink-0 w-[34px] h-5 rounded-full bg-gc-text transition-colors duration-200"
            } else {
                "relative shrink-0 w-[34px] h-5 rounded-full bg-gc-border-strong transition-colors duration-200"
            }
        >
            <span class=move || if on.get() {
                "absolute top-0.5 left-4 w-4 h-4 rounded-full bg-gc-bg-elevated shadow transition-[left] duration-200"
            } else {
                "absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white shadow transition-[left] duration-200"
            }></span>
        </div>
    }
}

/// A closed select showing its value, in gold when the value needs attention. Put a
/// `DemoMenu` in its children to show it open.
#[component]
pub fn SelectPill(
    #[prop(into)] label: Signal<&'static str>,
    #[prop(optional, into)] warn: Signal<bool>,
    #[prop(optional)] aim: NodeRef<html::Div>,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || if warn.get() {
                "relative shrink-0 min-w-[132px] sm:min-w-[150px] rounded-lg border border-gc-border bg-gc-bg-elevated py-1 pl-2.5 pr-7 text-[12.5px] font-medium text-gc-gold"
            } else {
                "relative shrink-0 min-w-[132px] sm:min-w-[150px] rounded-lg border border-gc-border bg-gc-bg-elevated py-1 pl-2.5 pr-7 text-[12.5px] text-gc-text"
            }
        >
            {move || label.get()}
            <span class="material-symbols-outlined absolute right-1.5 top-1/2 -translate-y-1/2 text-[16px] text-gc-text-muted">"expand_more"</span>
            {children()}
        </div>
    }
}

/// A dropdown panel hanging under its positioned parent. `class` places it.
#[component]
pub fn DemoMenu(#[prop(optional, into)] class: String, children: Children) -> impl IntoView {
    view! {
        <div class=format!("absolute top-full z-10 rounded-xl border border-gc-border bg-gc-bg-elevated p-1.5 shadow-2xl {class}")>
            {children()}
        </div>
    }
}

/// One choice in a `DemoMenu`: checked if it's the current value, lit if the cursor is on it.
#[component]
pub fn MenuItem(
    label: &'static str,
    #[prop(optional)] checked: bool,
    #[prop(optional, into)] lit: Signal<bool>,
    #[prop(optional)] aim: NodeRef<html::Div>,
) -> impl IntoView {
    view! {
        <div
            node_ref=aim
            class=move || if lit.get() {
                "flex items-center gap-1.5 whitespace-nowrap rounded-md bg-gc-bg-hover px-2 py-1.5 text-[12.5px] text-gc-text"
            } else {
                "flex items-center gap-1.5 whitespace-nowrap rounded-md px-2 py-1.5 text-[12.5px] text-gc-text"
            }
        >
            <span class=if checked { "material-symbols-outlined text-[14px]" } else { "material-symbols-outlined text-[14px] invisible" }>"check"</span>
            {label}
        </div>
    }
}
