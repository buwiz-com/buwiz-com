//! The older submission path (`POST /api/submissions`) checks the return
//! inside against its file name, as the Sender checks a sealed return
//! against its ticket: an account active for one TIN cannot file a return
//! whose XML names another TIN, branch, form or period. A refused return is
//! not stored, so there is nothing for the relay to hand to the Sender.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::*;
use serde_json::{Value, json};
use sqlx::PgPool;

async fn post_v1(site: &Site, token: &str, filename: &str, xml: &str) -> axum::response::Response {
    let body = json!({ "filename": filename, "xml": xml });
    let request = Request::post("/api/submissions")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    site.send(request).await
}

async fn stored(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM submissions").fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_v1_return_must_be_the_one_its_file_name_names(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    // Ana owns 000-123-453 (head office) and 000-123-454; bob owns 000-123-455.
    add_claim(&site.db, ana, "000123453", "approved").await;
    add_claim(&site.db, ana, "000123454", "approved").await;
    let (bob, _) = site.person("bob@example.test").await;
    add_claim(&site.db, bob, "000123455", "approved").await;
    let token = app_token(&site.db, ana).await;
    let name = "00012345300000-1701Qv2018-2026Q1#ana@example.test#.xml";

    // A return that is what its name says goes through.
    let matching = iaf_return("frm1701q", "00012345300000", "2026");
    let response = post_v1(&site, &token, name, &matching).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let view: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(view["status"], "ready");
    assert_eq!(stored(&site.db).await, 1);

    for (xml, words) in [
        // Another account's TIN inside, ana's on the outside.
        (iaf_return("frm1701q", "00012345500000", "2026"), "the TIN in the return is not the file name's TIN"),
        // Ana's other TIN: still not the one named.
        (iaf_return("frm1701q", "00012345400000", "2026"), "the TIN in the return is not the file name's TIN"),
        // The same TIN at a branch.
        (iaf_return("frm1701q", "00012345300001", "2026"), "the TIN in the return is not the file name's TIN"),
        // Another form.
        (
            iaf_return("frm1601c", "00012345300000", "2026"),
            "the return is a frm1601c form, not the file name's 1701Qv2018",
        ),
        // Another year.
        (iaf_return("frm1701q", "00012345300000", "2025"), "the return is for 2025, not the file name's period 2026Q1"),
        // Not a return at all.
        ("<return/>".to_owned(), "the return has no form fields"),
    ] {
        let (status, code, message) = refusal(post_v1(&site, &token, name, &xml).await).await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "return_mismatch", format!("The return does not match its file name: {words}.").as_str()),
        );
    }
    // Nothing refused was kept, so the relay has nothing to send.
    assert_eq!(stored(&site.db).await, 1);

    // The file name's branch counts too: a branch return in a head-office
    // name, and the other way round.
    let branch_name = "00012345300001-1701Qv2018-2026Q1#ana@example.test#.xml";
    let (status, code, _) = refusal(post_v1(&site, &token, branch_name, &matching).await).await;
    assert_eq!((status, code.as_str()), (400, "return_mismatch"));

    // The check comes before the send gate: a mismatch on a TIN that is not
    // ana's reads the same as on her own, and a matching return for bob's
    // TIN is refused by the gate as before.
    let bobs = "00012345500000-1701Qv2018-2026Q1#ana@example.test#.xml";
    let (status, code, _) = refusal(post_v1(&site, &token, bobs, &matching).await).await;
    assert_eq!((status, code.as_str()), (400, "return_mismatch"));
    let theirs = iaf_return("frm1701q", "00012345500000", "2026");
    let (status, code, _) = refusal(post_v1(&site, &token, bobs, &theirs).await).await;
    assert_eq!((status, code.as_str()), (403, "tin_not_active"));
    assert_eq!(stored(&site.db).await, 1);
}
