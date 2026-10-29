//! V6, the whole way: bir-core queues a 1701Q for the demo taxpayer Juana
//! Santos (000-123-453-00000); the real bir-submit client signs in with the
//! device flow, registers its device key, gets a ticket, seals the XML and
//! posts it; the Gatekeeper hands it to the Sender, which opens it, checks
//! it against the ticket and sends it through the app's pipe to a fake
//! dispatcher and an SFTP server with a pinned key. The receipt verifies
//! against the JWKS and the queue ends at Sent.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn};
use buwiz_cloud::bir::iaf;
use common::{EMAIL, Harness, IAF_PASS, TIN, TIN_ROOT};
use sha2::{Digest, Sha256};

const TEST_DB_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_1701q_is_sent_through_the_gatekeeper_and_sender() {
    let harness = Harness::start().await;
    let (submission, _store) = harness.signed_in().await;

    // The local app: Juana Santos and her 1701Q for Q1 2026, queued.
    // SAFETY: set before any thread reads it.
    unsafe { std::env::set_var("BIR_DB_KEY", TEST_DB_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let app = BirApp::open_at(dir.path().join("bir_data.db")).unwrap();
    let taxpayer = app
        .create_taxpayer(&NewTaxpayer {
            tin_root: TIN_ROOT.into(),
            name: "Juana Santos".into(),
            taxpayer_type: "Individual".into(),
            birth_date: Some("1990-01-15".into()),
            head_office: BranchFacts {
                rdo_code: "043".into(),
                registered_address: "1 Sample Street, Pasig City".into(),
                zip_code: "1600".into(),
                phone: "09170000100".into(),
                email: EMAIL.into(),
            },
        })
        .unwrap();
    let tin = taxpayer.branches[0].tin.clone();
    assert_eq!(tin, TIN);
    let slot = OpenReturn { tin: tin.clone(), form_code: "1701Q".into(), year: 2026, period: 1, key: None };
    let opened = app.open_return(&slot).unwrap();
    let edits = BTreeMap::from([
        ("frm1701q:optType_1".to_owned(), FieldValue::Tick(true)),
        ("frm1701q:optATC_1".to_owned(), FieldValue::Tick(true)),
        ("frm1701q:optTaxRate_1".to_owned(), FieldValue::Tick(true)),
        ("frm1701q:optMethodOfDeduction:_2".to_owned(), FieldValue::Tick(true)),
    ]);
    let filled = app.fill("1701Q", &opened.draft, &edits, &[]).unwrap();
    let validated = app.validate("1701Q", &filled.draft).unwrap();
    assert!(validated.errors.is_empty(), "{:?}", validated.errors);
    let saved = app.save("1701Q", &validated.draft).unwrap();
    app.queue_return("1701Q", &saved.draft.unwrap_or(validated.draft)).unwrap();
    assert_eq!(app.list_queue().unwrap().len(), 1);

    // The queue worker sends it with bir-submit's transport.
    let worker = app.start_queue_worker(submission.transport()).unwrap();
    let mut status = String::new();
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        status = app.open_return(&slot).unwrap().status;
        if status != "Queued" {
            break;
        }
    }
    bir_core::background_cron::request_shutdown();
    let sent = app.open_return(&slot).unwrap();
    assert!(
        matches!(status.as_str(), "Sent" | "Submitted"),
        "queue state {status}: {:?}",
        sent.draft.0.get("submission_error")
    );

    // The receipt is kept on the return and verifies against the JWKS.
    let receipt_jws = sent.draft.0["submission_receipt"].as_str().expect("receipt stored").to_owned();
    let files = harness.received.lock().await.clone();
    let filename = format!("{TIN}-1701Qv2018-2026Q1#{EMAIL}#.xml");
    let uploaded = files.get(&format!("/1701Qv2018/{filename}")).expect("the file reached the SFTP server");
    let xml = iaf::decrypt(uploaded, IAF_PASS).expect("IAF-encrypted with the Sender's passphrase");
    let xml_sha256 = hex(&Sha256::digest(&xml));
    // The Gatekeeper kept only the envelope, and dropped it after the send.
    let (status, envelope, ticket_jti): (String, Option<String>, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT status, envelope, ticket_jti FROM submissions WHERE account_id = $1")
            .bind(harness.account_id)
            .fetch_one(&harness.db)
            .await
            .unwrap();
    assert_eq!(status, "sent");
    assert_eq!(envelope, None);
    let ticket_jti = ticket_jti.expect("a v2 submission names its ticket").to_string();
    // The receipt names that same ticket: verified with the jti required.
    let receipt = submission
        .submitter()
        .verify_receipt(&receipt_jws, &xml_sha256, Some(&ticket_jti))
        .await
        .expect("receipt verifies for this XML and this ticket");
    assert_eq!(receipt.jti.as_deref(), Some(ticket_jti.as_str()));
    assert_eq!(receipt.filename, filename);
    assert_eq!(receipt.tin, "000-123-453-00000");
    assert_eq!((receipt.form_type.as_str(), receipt.period.as_str()), ("1701Qv2018", "2026Q1"));
    assert_eq!(receipt.status, "sent");
    assert!(std::str::from_utf8(&xml).unwrap().contains("frm1701q:txtTIN3=453"));
    let _ = worker;
    println!("V6 e2e: 1701Q for {TIN} sent; receipt {} verified", receipt.sub);
}
