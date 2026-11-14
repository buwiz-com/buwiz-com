//! The staff console's small parts: icons, pills, notices, the reason
//! dialogs. The look is staff.css's; docs/staff-design.md lists them.

use super::*;
use std::sync::LazyLock;
use topcoat::view::{Child, Unescaped};

/// An icon of staff-icons.svg. `name` is one of its symbols, a constant, so
/// the markup is ours. It points into the page's own copy of the icons
/// ([`sprite`]), so it loads no other file.
pub(crate) fn ic(name: &'static str) -> Unescaped<String> {
    icon_with(name, "ic")
}

/// [`ic`] with other classes (`ic sm`, `ic lg`).
pub(crate) fn icon_with(name: &'static str, class: &'static str) -> Unescaped<String> {
    Unescaped::new_unchecked(format!(
        r##"<svg class="{class}" aria-hidden="true" focusable="false"><use href="#{name}"></use></svg>"##
    ))
}

/// staff-icons.svg, inline once at the top of every staff page's body
/// (`shell::document`), so every [`ic`] finds its symbol in the same
/// document: an external `<use>` file is one more load the policy's
/// `default-src 'none'` may refuse (Firefox does). It takes no space and is
/// hidden from screen readers; the file itself has no style or script.
pub(crate) fn sprite() -> Unescaped<&'static str> {
    static SPRITE: LazyLock<String> = LazyLock::new(|| {
        include_str!("../../staff-icons.svg").replacen(
            r#"<svg xmlns="http://www.w3.org/2000/svg">"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" class="sx-sprite" width="0" height="0" aria-hidden="true" focusable="false">"#,
            1,
        )
    });
    Unescaped::new_unchecked(SPRITE.as_str())
}

/// The console's icon for one of the Material Symbols names the rest of
/// the code uses for flags, checks and history lines.
pub(crate) fn glyph(material: &str) -> &'static str {
    match material {
        "science" => "flask",
        "group" => "users",
        "person_check" | "how_to_reg" => "user-check",
        "bolt" => "bolt",
        "pause_circle" => "pause",
        "play_circle" => "play",
        "replay" => "replay",
        "swap_horiz" => "swap",
        "fact_check" | "task" | "add_task" | "upload_file" => "file-check",
        "rule" | "warning" => "warning",
        "report" | "error" => "alert",
        "face" | "person" => "person",
        "id_card" | "badge" => "badge",
        "gavel" => "gavel",
        "remove" | "remove_moderator" => "minus",
        "visibility" => "eye",
        "check_circle" | "verified" => "check-circle",
        "more_horiz" => "more",
        "person_add" => "user-plus",
        "login" => "log-in",
        "logout" => "log-out",
        "smart_toy" => "bot",
        "desktop_windows" => "monitor",
        "language" => "globe",
        "bookmark_add" => "bookmark",
        "assignment_return" | "undo" => "undo",
        "cancel" | "remove_circle" => "x-circle",
        "block" => "ban",
        "delete" => "trash",
        "lock_open" => "unlock",
        "shield_person" => "shield",
        "verified_user" => "shield-check",
        "schedule" | "hourglass_top" => "clock",
        "help" => "info",
        "storefront" => "store",
        "picture_as_pdf" | "document_scanner" | "draft" | "description" | "attach_file" => "file",
        "image" => "image",
        _ => "dot",
    }
}

/// The pill tone for a status look's tone ([`status_look`]).
pub(crate) fn status_tone(tone: &str) -> &'static str {
    match tone {
        "active" => "ok",
        "waiting" | "paused" => "warn",
        "fix" | "rejected" => "bad",
        _ => "neutral",
    }
}

/// The pill tone for a flag's tone (`good`, `warn`, `bad`, `info`).
pub(crate) fn flag_tone(tone: &str) -> &'static str {
    match tone {
        "good" => "ok",
        "warn" => "warn",
        "bad" => "bad",
        _ => "info",
    }
}

/// A rounded label; `tone` is one of staff.css's `.sx-pill` tones.
#[component]
pub(crate) async fn sx_pill(#[into] label: String, sym: &'static str, tone: &'static str) -> Result<impl View> {
    Ok(view! {
        <span class=(format!("sx-pill {tone}")) title=(label.clone())>
            if !sym.is_empty() {
                (icon_with(sym, "ic sm"))
            }
            <span>(label)</span>
        </span>
    })
}

/// A claim's status as a pill.
#[component]
pub(crate) async fn status_pill(status: String) -> Result<impl View> {
    let (label, sym, tone) = status_look(&status);
    Ok(view! { sx_pill(label: label, sym: glyph(sym), tone: status_tone(tone)) })
}

/// Personal or business, as the small square icon tables use.
#[component]
pub(crate) async fn kind_icon(kind: String) -> Result<impl View> {
    let business = kind != "personal";
    Ok(view! {
        <span class="sx-kind" title=(ClaimKind::label(&kind))>
            (ic(if business { "store" } else { "person" }))
            <span class="sx-vh">(ClaimKind::label(&kind))</span>
        </span>
    })
}

/// A flag as a pill.
#[component]
pub(crate) async fn flag_pill(item: Flag) -> Result<impl View> {
    let (tone, sym, text) = item;
    Ok(view! { sx_pill(label: text, sym: glyph(sym), tone: flag_tone(tone)) })
}

/// Two letters for an account, in a circle; `me` paints it in the accent.
#[component]
pub(crate) async fn avatar(email: String, #[default] size: &'static str, #[default] me: bool) -> Result<impl View> {
    let class = format!("sx-av {size}{}", if me { " me" } else { "" });
    Ok(view! { <span class=(class) aria-hidden="true">(initials(&email))</span> })
}

/// The notice a redirect brought (`?notice=`), with an icon for its tone.
/// Refusals read as errors.
#[component]
pub(crate) async fn sx_notice(notice: Option<Notice>) -> Result<impl View> {
    Ok(view! {
        if let Some(notice) = notice {
            <p class=(if notice.is_error() { "sx-notice error" } else { "sx-notice" }) role=(if notice.is_error() { "alert" } else { "status" })>
                (ic(if notice.is_error() { "alert" } else { "check-circle" }))
                <span>(notice.message())</span>
            </p>
        }
    })
}

/// The page head: a title, a line under it, and its actions.
#[component]
pub(crate) async fn page_head(
    #[into] title: String,
    #[default] sub: String,
    #[default] back: Option<String>,
    #[default] mono: bool,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <header class="sx-head">
            if let Some(back) = back {
                <a class="sx-ib sx-back" href=(back) aria-label="Back">(ic("arrow-left"))</a>
            }
            <div class="sx-head-t">
                <h1 class=(if mono { "sx-mono" } else { "" })>(title)</h1>
                if !sub.is_empty() {
                    <p>(sub)</p>
                }
            </div>
            <div class="sx-head-a">(child)</div>
        </header>
    })
}

/// An action that needs a reason: a button that opens its form. Without
/// script the form opens in place (`<details>`); staff.js moves it into a
/// modal dialog labelled by `question`. `id` names the dialog's heading.
/// The required reason field is the caller's (see [`reason_field`]), so the
/// form keeps its own fields and order.
#[component]
pub(crate) async fn act(
    id: String,
    label: &'static str,
    #[default] sym: &'static str,
    #[default] tone: &'static str,
    #[default] small: bool,
    #[into] question: String,
    #[default] detail: String,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let class = format!(
        "sx-btn{}{}",
        if tone.is_empty() { String::new() } else { format!(" {tone}") },
        if small { " sm" } else { "" }
    );
    Ok(view! {
        <details class="sx-act">
            <summary class=(class)>
                if !sym.is_empty() {
                    (ic(sym))
                }
                <span>(label)</span>
            </summary>
            <div class="sx-act-panel">
                <h3 id=(id)>(question)</h3>
                if !detail.is_empty() {
                    <p>(detail)</p>
                }
                (child)
            </div>
        </details>
    })
}

/// The buttons at the foot of an action's form: Cancel (with script, in the
/// dialog) and the action itself.
#[component]
pub(crate) async fn act_buttons(label: &'static str, #[default] danger: bool) -> Result<impl View> {
    Ok(view! {
        <div class="sx-form-a">
            <button class="sx-btn" type="button" data-close="">"Cancel"</button>
            <button class=(if danger { "sx-btn dan solid" } else { "sx-btn pri" }) type="submit">(label)</button>
        </div>
    })
}

/// The required reason of a staff form, kept in the account's history.
#[component]
pub(crate) async fn reason_field() -> Result<impl View> {
    Ok(view! {
        <label class="sx-field">
            <span>"Reason " <span class="sx-faint">"Required. Kept in the account's history."</span></span>
            <textarea name="reason" rows="3" maxlength="500" required=(true) placeholder="Why"></textarea>
        </label>
    })
}

/// A form that ends something with a required reason, as [`act`]: the
/// question, what happens, and the red (or `danger: false`) button.
#[component]
pub(crate) async fn reason_act(
    id: String,
    label: &'static str,
    #[default] sym: &'static str,
    #[default] tone: &'static str,
    #[default] small: bool,
    action: String,
    #[into] question: String,
    #[into] detail: String,
    confirm: &'static str,
    #[default(true)] danger: bool,
) -> Result<impl View> {
    Ok(view! {
        act(id: id, label: label, sym: sym, tone: tone, small: small, question: question, detail: detail,
            <form class="sx-form end-form" method="post" action=(action)>
                reason_field()
                act_buttons(label: confirm, danger: danger)
            </form>
        )
    })
}

/// `path?key=value&…` without the empty values.
pub(crate) fn href(path: &str, params: &[(&str, &str)]) -> String {
    let kept: Vec<(&str, &str)> = params.iter().filter(|(_, value)| !value.is_empty()).copied().collect();
    if kept.is_empty() {
        return path.to_owned();
    }
    let mut query = form_urlencoded::Serializer::new(String::new());
    for (key, value) in kept {
        query.append_pair(key, value);
    }
    format!("{path}?{}", query.finish())
}

/// A short span: `1 d 2 h`, `5 h`, `40 min`.
pub(crate) fn short_span(at: OffsetDateTime) -> String {
    let elapsed = OffsetDateTime::now_utc() - at;
    let (days, hours, minutes) = (elapsed.whole_days(), elapsed.whole_hours() % 24, elapsed.whole_minutes().max(1));
    if days >= 1 {
        if hours > 0 { format!("{days} d {hours} h") } else { format!("{days} d") }
    } else if elapsed.whole_hours() >= 1 {
        format!("{} h", elapsed.whole_hours())
    } else {
        format!("{minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_leave_out_empty_values() {
        assert_eq!(href("/staff", &[("status", "waiting"), ("q", "")]), "/staff?status=waiting");
        assert_eq!(href("/staff/users", &[("q", ""), ("filter", "")]), "/staff/users");
        assert_eq!(href("/staff/users", &[("q", "a b&c")]), "/staff/users?q=a+b%26c");
    }

    #[test]
    fn every_glyph_is_in_the_sprite() {
        let sprite = include_str!("../../staff-icons.svg");
        let names = [
            "science",
            "group",
            "person_check",
            "bolt",
            "pause_circle",
            "play_circle",
            "replay",
            "swap_horiz",
            "fact_check",
            "rule",
            "report",
            "face",
            "id_card",
            "gavel",
            "remove",
            "visibility",
            "check_circle",
            "more_horiz",
            "person_add",
            "login",
            "logout",
            "smart_toy",
            "desktop_windows",
            "language",
            "bookmark_add",
            "assignment_return",
            "cancel",
            "block",
            "delete",
            "lock_open",
            "shield_person",
            "verified_user",
            "schedule",
            "help",
            "storefront",
            "picture_as_pdf",
            "image",
            "anything else",
        ];
        for name in names {
            let id = glyph(name);
            assert!(sprite.contains(&format!("<symbol id=\"{id}\"")), "{name} -> {id}");
        }
    }

    /// The source of the staff pages without their tests.
    fn page_sources() -> Vec<(&'static str, &'static str)> {
        let code = |source: &'static str| source.split("#[cfg(test)]").next().unwrap();
        vec![
            ("staff.rs", code(include_str!("../staff.rs"))),
            ("overview.rs", code(include_str!("overview.rs"))),
            ("search.rs", code(include_str!("search.rs"))),
            ("shell.rs", code(include_str!("shell.rs"))),
            ("ui.rs", code(include_str!("ui.rs"))),
        ]
    }

    /// The text of the argument that starts at `from` (just after its
    /// opening bracket, or after `sym:`), up to the comma or bracket that
    /// ends it at its own depth.
    fn argument(source: &str, from: usize) -> &str {
        let mut depth = 0;
        let mut in_text = false;
        for (at, ch) in source[from..].char_indices() {
            match ch {
                '"' => in_text = !in_text,
                _ if in_text => {}
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' if depth == 0 => return &source[from..from + at],
                ')' | '}' | ']' => depth -= 1,
                ',' if depth == 0 => return &source[from..from + at],
                _ => {}
            }
        }
        &source[from..]
    }

    /// The icon names an argument can be: a string literal, each branch of
    /// an `if` (`{ "a" } else { "b" }`), or `glyph("…")` mapped through
    /// [`glyph`]. A variable names none.
    fn names_in(argument: &str) -> Vec<String> {
        let argument = argument.trim();
        let literals = |text: &str| -> Vec<String> { text.split('"').skip(1).step_by(2).map(str::to_owned).collect() };
        if let Some(inner) = argument.strip_prefix("glyph(") {
            return literals(inner).iter().map(|name| glyph(name).to_owned()).collect();
        }
        if argument.starts_with('"') {
            return literals(argument).into_iter().take(1).collect();
        }
        if argument.starts_with("if ") {
            return argument
                .split('{')
                .skip(1)
                .filter_map(|branch| branch.trim_start().strip_prefix('"'))
                .filter_map(|rest| rest.split('"').next())
                .map(str::to_owned)
                .collect();
        }
        Vec::new()
    }

    /// Every icon the staff pages name in their source: in `ic(…)`, as the
    /// first argument of `icon_with(…)`, as a `sym:` given to a pill or an
    /// action, and the ones staff.js swaps in (`"#name"`).
    fn named_icons() -> std::collections::BTreeSet<String> {
        let mut found = std::collections::BTreeSet::new();
        for (_, source) in page_sources() {
            for marker in ["ic(", "icon_with(", "sym: "] {
                for (at, _) in source.match_indices(marker) {
                    let before = source[..at].chars().next_back().unwrap_or(' ');
                    if before.is_alphanumeric() || before == '_' {
                        continue;
                    }
                    found.extend(names_in(argument(source, at + marker.len())));
                }
            }
        }
        let script = include_str!("../../staff.js");
        for piece in script.split("\"#").skip(1) {
            found.insert(piece[..piece.find('"').unwrap()].to_owned());
        }
        found.remove("");
        found
    }

    #[test]
    fn every_icon_named_in_the_source_is_in_the_sprite() {
        let sprite = include_str!("../../staff-icons.svg");
        let names = named_icons();
        // The scan finds them: the More menu's chevron, both branches of an
        // `if`, a pill's `sym:`, a `glyph("…")` and staff.js's swap.
        for expected in ["chevron-down", "bot", "monitor", "flask", "chevrons-left", "chevrons-right", "check-circle"] {
            assert!(names.contains(expected), "{expected} not found in {names:?}");
        }
        assert!(names.len() > 40, "{names:?}");
        for name in &names {
            assert!(sprite.contains(&format!("<symbol id=\"{name}\"")), "{name} is not in staff-icons.svg");
        }
    }

    #[test]
    fn the_sprite_is_inline_and_hidden() {
        let inline = sprite().to_string();
        assert!(inline.starts_with(
            r#"<svg xmlns="http://www.w3.org/2000/svg" class="sx-sprite" width="0" height="0" aria-hidden="true" focusable="false">"#
        ));
        assert!(!inline.contains("style") && !inline.contains("<script"), "{inline}");
        assert_eq!(
            ic("search").to_string(),
            r##"<svg class="ic" aria-hidden="true" focusable="false"><use href="#search"></use></svg>"##
        );
    }
}
