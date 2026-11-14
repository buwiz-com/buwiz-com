//! The staff console's frame around every /staff page: the left sidebar
//! (an icon rail on tablets), the phone's top bar and tab bar, the document
//! head that loads staff.css and staff.js, and the icons inline. The page itself renders
//! `<main id="main">` and, where it has one, the inspector beside it.

use super::ui::{avatar, ic, sprite};
use super::*;
use crate::account::staff::Counts;
use topcoat::view::Child;
use topcoat::view::ViewExt;

/// Where the request is, for `aria-current` on the navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Here {
    Overview,
    Queue(QueueFilter),
    /// A claim's own page.
    Claim,
    Users(UserFilter),
    /// Another account's page.
    User,
    Me,
    Search,
    Other,
}

fn here(path: &str, status: Option<&str>, filter: Option<&str>, me: Uuid) -> Here {
    match path {
        "/staff" => Here::Overview,
        "/staff/queue" => Here::Queue(QueueFilter::parse(status)),
        "/staff/users" => Here::Users(UserFilter::parse(filter)),
        "/staff/search" => Here::Search,
        _ if path.starts_with("/staff/claims/") => Here::Claim,
        _ if path == format!("/staff/users/{me}") => Here::Me,
        _ if path.starts_with("/staff/users/") => Here::User,
        _ => Here::Other,
    }
}

/// The whole staff document. `title` is the page's (`head_for`); `admin`
/// shows New user; `counts` are the sidebar's numbers.
#[component]
pub(crate) async fn document(
    cx: &Cx,
    title: &'static str,
    me: Account,
    admin: bool,
    counts: Counts,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let path = uri(cx).path().to_owned();
    let at = here(&path, query(cx, "status").as_deref(), query(cx, "filter").as_deref(), me.id);
    let my_page = format!("/staff/users/{}", me.id);
    let queue_on = matches!(at, Here::Queue(_) | Here::Claim);
    let users_on = matches!(at, Here::Users(_) | Here::User);
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
                <title>(title)</title>
                <meta name="robots" content="noindex">
                <link rel="icon" type="image/svg+xml" href="/assets/brand/buwiz_favicon.svg">
                <link rel="icon" href="/favicon.ico" sizes="48x48">
                <link rel="apple-touch-icon" href="/assets/brand/apple-touch-icon.png">
                <meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)">
                <meta name="theme-color" content="#1c201f" media="(prefers-color-scheme: dark)">
                <link rel="preload" href="/assets/fonts/inter-variable.woff2" as="font" type="font/woff2" crossorigin="">
                <link rel="stylesheet" href="/assets/staff.css">
                // Not deferred: it marks the page as scripted before the
                // first paint, then waits for the page.
                <script src="/assets/staff.js"></script>
            </head>
            <body class="sx">
                // Every icon on the page points into this one copy.
                (sprite())
                <a class="sx-skip" href="#main">"Skip to content"</a>
                <div class="sx-app">
                    <aside class="sx-side" id="sx-side" aria-label="Staff">
                        <div class="sx-brand">
                            <a href="/staff" aria-label="Buwiz staff: overview">
                                <span class="brand-wordmark sx-word" aria-hidden="true"></span>
                                <span class="brand-mark sx-mark" aria-hidden="true"></span>
                            </a>
                            <span class="sx-stag" aria-hidden="true">"Staff"</span>
                            <button class="sx-ib sx-rail-toggle" type="button" aria-expanded="false" aria-controls="sx-side" aria-label="Show names" hidden=(true)>
                                (ic("chevrons-right"))
                            </button>
                        </div>
                        <form class="sx-search" method="get" action="/staff/search" role="search">
                            (ic("search"))
                            <label class="sx-vh" for="sx-q">"Search everything"</label>
                            <input id="sx-q" type="search" name="q" placeholder="Search email, name or TIN" autocomplete="off">
                        </form>
                        <a class="sx-ib sx-search-icon" href="/staff/search" aria-label="Search everything" title="Search everything">(ic("search"))</a>
                        if admin {
                            <a class="sx-btn pri sx-new" href="/staff/users?new=1" data-new-user="" title="New user">
                                (ic("user-plus"))
                                <span class="t">"New user"</span>
                            </a>
                        }
                        <nav class="sx-nav" aria-label="Staff sections">
                            <a class="sx-ni" href="/staff" if at == Here::Overview { aria-current="page" } title="Overview">
                                (ic("grid"))
                                <span class="t">"Overview"</span>
                            </a>
                            <div class="sx-ng" role="group" aria-labelledby="sx-ng-review">
                                <p class="sx-ng-h" id="sx-ng-review">"Review"</p>
                                <a class="sx-ni" href="/staff/queue?status=waiting" if at == Here::Queue(QueueFilter::Waiting) { aria-current="page" } title="Waiting">
                                    (ic("inbox"))
                                    <span class="t">"Waiting"</span>
                                    <span class=(if counts.waiting > 0 { "sx-count hot" } else { "sx-count" })>
                                        (counts.waiting.to_string())
                                        <span class="sx-vh">" waiting"</span>
                                    </span>
                                    if counts.waiting > 0 {
                                        <span class="sx-badge" aria-hidden="true">(counts.waiting.to_string())</span>
                                    }
                                </a>
                                <a class="sx-ni rail-hide" href="/staff/queue?status=sent_back" if at == Here::Queue(QueueFilter::SentBack) { aria-current="page" } title="Sent back">
                                    (ic("replay"))
                                    <span class="t">"Sent back"</span>
                                    <span class="sx-count">(counts.sent_back.to_string())</span>
                                </a>
                                <a class="sx-ni rail-hide" href="/staff/queue?status=auto" if at == Here::Queue(QueueFilter::Auto) { aria-current="page" } title="Activated automatically">
                                    (ic("bolt"))
                                    <span class="t">"Activated automatically"</span>
                                    <span class="sx-count">(counts.auto.to_string())</span>
                                </a>
                            </div>
                            <div class="sx-ng" role="group" aria-labelledby="sx-ng-people">
                                <p class="sx-ng-h" id="sx-ng-people">"People"</p>
                                <a class="sx-ni" href="/staff/users" if at == Here::Users(UserFilter::Everyone) || at == Here::User { aria-current="page" } title="All users">
                                    (ic("users"))
                                    <span class="t">"All users"</span>
                                    <span class="sx-count">(counts.users.to_string())</span>
                                </a>
                                <a class="sx-ni rail-hide" href="/staff/users?filter=staff" if at == Here::Users(UserFilter::Staff) { aria-current="page" } title="Staff">
                                    (ic("shield"))
                                    <span class="t">"Staff"</span>
                                    <span class="sx-count">(counts.staff.to_string())</span>
                                </a>
                                <a class="sx-ni rail-hide" href="/staff/users?filter=test" if at == Here::Users(UserFilter::Test) { aria-current="page" } title="Test accounts">
                                    (ic("flask"))
                                    <span class="t">"Test accounts"</span>
                                    <span class="sx-count">(counts.test.to_string())</span>
                                </a>
                                <a class="sx-ni rail-hide" href="/staff/users?filter=banned" if at == Here::Users(UserFilter::Banned) { aria-current="page" } title="Banned">
                                    (ic("ban"))
                                    <span class="t">"Banned"</span>
                                    <span class="sx-count">(counts.banned.to_string())</span>
                                </a>
                            </div>
                        </nav>
                        <div class="sx-foot">
                            <a class="sx-me" href=(my_page.clone()) if at == Here::Me { aria-current="page" } title="My account">
                                avatar(email: me.email.clone(), size: "", me: true)
                                <span class="x">
                                    <b>"My account"</b>
                                    <span>(me.email.clone())</span>
                                </span>
                                (ic("chevron-right"))
                            </a>
                            <div class="sx-footlinks">
                                <a class="sx-link" href="/tins">(ic("badge")) "My TINs"</a>
                                <form method="post" action="/logout">
                                    <button class="sx-link" type="submit">(ic("log-out")) "Sign out"</button>
                                </form>
                            </div>
                        </div>
                    </aside>
                    <header class="sx-top">
                        <a href="/staff" aria-label="Buwiz staff: overview">
                            <span class="brand-wordmark sx-word" aria-hidden="true"></span>
                        </a>
                        <span class="sx-stag" aria-hidden="true">"Staff"</span>
                        <span class="sx-sp"></span>
                        <a class="sx-ib" href="/staff/search" aria-label="Search everything">(ic("search"))</a>
                        <form method="post" action="/logout">
                            <button class="sx-ib" type="submit" aria-label="Sign out">(ic("log-out"))</button>
                        </form>
                    </header>
                    <div class="sx-body">(child)</div>
                    <nav class="sx-tabbar" aria-label="Staff sections">
                        <a href="/staff" if at == Here::Overview { aria-current="page" }>
                            <span class="i">(ic("grid"))</span>
                            "Overview"
                        </a>
                        <a href="/staff/queue?status=waiting" if queue_on { aria-current="page" }>
                            <span class="i">
                                (ic("inbox"))
                                if counts.waiting > 0 {
                                    <span class="sx-badge">(counts.waiting.to_string())<span class="sx-vh">" waiting"</span></span>
                                }
                            </span>
                            "Queue"
                        </a>
                        <a href="/staff/users" if users_on { aria-current="page" }>
                            <span class="i">(ic("users"))</span>
                            "Users"
                        </a>
                        <a href=(my_page) if at == Here::Me { aria-current="page" }>
                            <span class="i">(ic("person"))</span>
                            "Me"
                        </a>
                    </nav>
                </div>
            </body>
        </html>
    }
    .boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_navigation_knows_where_it_is() {
        let me = Uuid::nil();
        assert_eq!(here("/staff", None, None, me), Here::Overview);
        assert_eq!(here("/staff/queue", None, None, me), Here::Queue(QueueFilter::Waiting));
        assert_eq!(here("/staff/queue", Some("auto"), None, me), Here::Queue(QueueFilter::Auto));
        assert_eq!(here("/staff/users", None, Some("banned"), me), Here::Users(UserFilter::Banned));
        assert_eq!(here(&format!("/staff/users/{me}"), None, None, me), Here::Me);
        assert_eq!(here("/staff/users/0b6e", None, None, me), Here::User);
        assert_eq!(here("/staff/claims/0b6e", None, None, me), Here::Claim);
    }
}
