//! Search everything: people and TIN claims from one box, by the rules the
//! users list and the claims queue already search by (email, registered or
//! trade name, the TIN's digits). TINs show as the queue shows them. Each
//! part asks for its own power: people the users list's (SeeUsers), claims
//! the queue's (Review).

use super::ui::{avatar, ic, kind_icon, page_head, status_pill, sx_pill};
use super::*;
use topcoat::view::ViewExt;

/// A found account: the row, its registered names, how many TINs it holds
/// and its staff badge.
type Person = (queries::UserRow, String, usize, Option<(&'static str, &'static str)>);

/// Which parts of a search a staff member sees: (people, claims). Each is
/// the power of the list it searches.
fn search_parts(can: impl Fn(Power) -> bool) -> (bool, bool) {
    (can(Power::SeeUsers), can(Power::Review))
}

#[page("/staff/search")]
pub async fn search_page(cx: &Cx) -> Result<impl View> {
    let viewer = staff_member_with_any(cx, &[Power::SeeUsers, Power::Review]).await?;
    let (people_part, claims_part) = search_parts(|power| viewer.can(power));
    let config = &state(cx).config;
    let db = &state(cx).db;
    let search = queries::search_text(&query(cx, "q").unwrap_or_default());
    let searched = !search.trim().is_empty();
    let users = if searched && people_part {
        let mut users = queries::users(db, UserFilter::Everyone, &search, &config.admin_emails).await?;
        users.truncate(queries::SEARCH_LIMIT as usize);
        users
    } else {
        Vec::new()
    };
    let ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    let user_claims = queries::claims_of(db, &ids).await?;
    let people: Vec<Person> = users
        .into_iter()
        .map(|user| {
            let own: Vec<&queries::UserClaim> =
                user_claims.iter().filter(|claim| claim.account_id == user.id).collect();
            let mut names: Vec<String> = Vec::new();
            for claim in &own {
                if !names.contains(&claim.registered_name) {
                    names.push(claim.registered_name.clone());
                }
            }
            let live = own
                .iter()
                .filter(|claim| claim.status == "paused" || crate::filing::gate::is_live(&claim.status))
                .count();
            let badge = role_badge(user_role(config, &user));
            (user, names.join(" · "), live, badge)
        })
        .collect();
    let claims = if claims_part { queries::search_claims(db, &search).await? } else { Vec::new() };
    let sub = match (people_part, claims_part) {
        (true, true) => "People and TIN claims, by email, registered name or TIN",
        (true, false) => "People, by email, registered name or TIN",
        _ => "TIN claims, by email, registered name or TIN",
    };
    let (people_count, claim_count) = (people.len(), claims.len());
    Ok(view! {
        <main id="main" class="sx-main" tabindex="-1">
            page_head(title: "Search", sub: sub.to_owned())
            <div class="sx-scroll">
                <div class="sx-stack">
                    <form class="sx-search sx-bigsearch" method="get" action="/staff/search" role="search">
                        (ic("search"))
                        <label class="sx-vh" for="sx-search-q">"Search everything"</label>
                        <input id="sx-search-q" type="search" name="q" value=(search.clone()) placeholder="Search email, name or TIN" autofocus=(!searched)>
                    </form>
                    if !searched {
                        <p class="sx-empty">"Type an email, a registered or trade name, or a TIN's digits."</p>
                    } else {
                        if people_part {
                            <section class="sx-results" aria-labelledby="sx-found-people">
                                <h2 id="sx-found-people">"People" <span class="sx-count">(people_count.to_string())</span></h2>
                                if people.is_empty() {
                                    <p class="sx-card sx-empty big">"No accounts match that search."</p>
                                } else {
                                    <div class="sx-card sx-tablewrap">
                                        <table class="sx-table users">
                                            <colgroup><col class="c-acct"><col><col class="c-tins"></colgroup>
                                            <thead>
                                                <tr><th scope="col">"Account"</th><th scope="col">"Registered name"</th><th scope="col">"TINs"</th></tr>
                                            </thead>
                                            <tbody>
                                                for (user, names, live, badge) in people {
                                                    <tr data-id=(user.id.to_string())>
                                                        <td class="a-main">
                                                            <span class="sx-acct">
                                                                avatar(email: user.email.clone(), size: "sm")
                                                                <a class="sx-rowlink" href=(format!("/staff/users/{}", user.id))>(user.email.clone())</a>
                                                                if let Some((label, _)) = badge {
                                                                    sx_pill(label: label, sym: "shield", tone: "info")
                                                                }
                                                                if user.test_account {
                                                                    sx_pill(label: "Test", sym: "flask", tone: "test")
                                                                }
                                                                if user.banned_at.is_some() {
                                                                    sx_pill(label: "Banned", sym: "ban", tone: "bad")
                                                                }
                                                            </span>
                                                        </td>
                                                        <td class="m cut a-sub">(if names.is_empty() { "No TIN added".to_owned() } else { names })</td>
                                                        <td class="m a-side">(format!("{live} TIN{}", if live == 1 { "" } else { "s" }))</td>
                                                    </tr>
                                                }
                                            </tbody>
                                        </table>
                                    </div>
                                }
                            </section>
                        }
                        if claims_part {
                            <section class="sx-results" aria-labelledby="sx-found-claims">
                                <h2 id="sx-found-claims">"TIN claims" <span class="sx-count">(claim_count.to_string())</span></h2>
                                if claims.is_empty() {
                                    <p class="sx-card sx-empty big">"No claims match that search."</p>
                                } else {
                                    <div class="sx-card sx-tablewrap">
                                        <table class="sx-table">
                                            <colgroup><col class="c-tin"><col><col class="sx-col-wide"><col class="c-status"></colgroup>
                                            <thead>
                                                <tr><th scope="col">"TIN"</th><th scope="col">"Name"</th><th scope="col" class="sx-col-wide">"Email"</th><th scope="col">"Status"</th></tr>
                                            </thead>
                                            <tbody>
                                                for claim in claims {
                                                    <tr data-id=(claim.id.to_string())>
                                                        <td class="a-main">
                                                            <span class="sx-cell-tin">
                                                                kind_icon(kind: claim.kind.clone())
                                                                <a class="sx-rowlink" href=(format!("/staff/claims/{}", claim.id))>(claim.tin().to_string())</a>
                                                            </span>
                                                        </td>
                                                        <td class="b cut a-sub">(claim.registered_name.clone())</td>
                                                        <td class="m cut sx-col-wide">(claim.email.clone())</td>
                                                        <td class="a-side">status_pill(status: claim.shown_status().to_owned())</td>
                                                    </tr>
                                                }
                                            </tbody>
                                        </table>
                                    </div>
                                }
                            </section>
                        }
                        <p class="sx-fine">(format!("Up to {} of each show; add more of the email, name or TIN to narrow.", queries::SEARCH_LIMIT))</p>
                    }
                </div>
            </div>
        </main>
    }
    .boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_part_of_a_search_asks_for_its_own_power() {
        assert_eq!(search_parts(|_| true), (true, true));
        assert_eq!(search_parts(|power| power == Power::Review), (false, true));
        assert_eq!(search_parts(|power| power == Power::SeeUsers), (true, false));
        // As the roles are today, Support and Admins see both.
        for role in [Role::Support, Role::Admin] {
            assert_eq!(search_parts(|power| role.can(power)), (true, true), "{role:?}");
        }
    }
}
