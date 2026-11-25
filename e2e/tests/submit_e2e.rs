//! V6, the whole way: Buwiz Forms (bir-core) queues a 1701Q for the demo
//! taxpayer Juana Santos (000-123-453-00000) and its queue worker sends it
//! through the Buwiz relay with the real bir-submit client: the app signs in
//! with the device flow, the client registers its device key, gets a ticket,
//! seals the XML and posts it; the Gatekeeper hands it to the Sender, which
//! opens it, checks it against the ticket and sends it through the app's pipe
//! to a fake dispatcher and an SFTP server with a pinned key. The receipt
//! verifies against the signed trust list and the queue ends at Submitted.
//!
//! The worker reads its settings from the process environment, as the app
//! does in a development build: this server (on this computer), the test
//! trust key, and a scratch folder for the database, the saved sign-in and
//! the device key. The keychain is never used.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bir_core::QueueAuthSource;
use bir_core::cloud_account::{self, AccountSecrets, StoredAccount};
use bir_core::db::Database;
use bir_core::forms::FilingStatus;
use bir_core::forms::form_1701q::{
    Form1701QAtc, Form1701QDeductionMethod, Form1701QDraft, Form1701QFilerType, Form1701QTaxRate,
};
use bir_core::forms::queueable::QueueableForm as _;
use bir_core::tax_profile::{BranchFacts, NewTaxpayer};
use buwiz_cloud::bir::iaf;
use common::{EMAIL, Harness, IAF_PASS, TIN, TIN_ROOT, TRUST_SEED, trust_public};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn a_queued_1701q_is_sent_through_the_gatekeeper_and_sender() {
    let scratch = tempfile::tempdir().unwrap();
    let database = scratch.path().join("bir_data.db");
    // SAFETY: set before any other thread of this test binary runs (it has
    // this one test, and the runtime below is not built yet).
    unsafe {
        std::env::set_var("BIR_DATABASE_PATH", &database);
        std::env::set_var(cloud_account::ENV_ACCOUNT_FILE, scratch.path().join("account.json"));
        std::env::set_var(bir_core::platform::NO_KEYCHAIN_ENV, "1");
        std::env::set_var(bir_core::transport::RELAY_SEND_ENV, "1");
        std::env::set_var(bir_submit::trust::KEYS_ENV, trust_public(&TRUST_SEED));
    }
    // The send future is deep: as in bir-core's own relay test, the
    // worker's threads get 16 MiB stacks, and so does this one.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .thread_stack_size(16 * 1024 * 1024)
                .build()
                .unwrap()
                .block_on(send_a_queued_1701q(database))
        })
        .unwrap()
        .join()
        .unwrap();
    drop(scratch);
}

async fn send_a_queued_1701q(database: std::path::PathBuf) {
    let harness = Harness::start().await;
    // SAFETY: no thread reads the environment yet: the worker has not
    // started, and nothing else in this process reads this variable.
    unsafe { std::env::set_var(cloud_account::ENV_SERVER_URL, &harness.base) };

    // The app's own sign-in (bir-core's device flow); Juana approves on the
    // account site (here: straight in the database layer).
    let base = harness.base.clone();
    let challenge = tokio::task::spawn_blocking(move || {
        cloud_account::start_device(&mut cloud_account::BlockingHttp::new(), &base, cloud_account::unix_now())
    })
    .await
    .unwrap()
    .expect("device flow starts");
    assert!(!challenge.verification_uri_complete.contains(&challenge.user_code));
    assert_eq!(
        buwiz_cloud::oauth::decide_device(&harness.db, &challenge.user_code, harness.account_id, true).await.unwrap(),
        buwiz_cloud::oauth::DeviceDecided::Done,
        "approved"
    );
    let base = harness.base.clone();
    tokio::task::spawn_blocking(move || {
        let mut http = cloud_account::BlockingHttp::new();
        let tokens = cloud_account::poll_until(&mut http, &challenge, &mut cloud_account::unix_now, &mut |delay| {
            std::thread::sleep(delay)
        })
        .expect("signed in");
        let (user_id, email, practice) = cloud_account::load_profile(&mut http, &base, &tokens.access_token).unwrap();
        assert_eq!(email.as_deref(), Some(EMAIL));
        assert!(!practice, "not a practice account");
        let now = cloud_account::unix_now();
        let mut stored = StoredAccount::from_secrets(&AccountSecrets {
            server_url: base,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            access_expires_at_unix: now + tokens.expires_in,
            user_id,
            email,
            offline: false,
            practice,
        });
        stored.last_refresh_unix = now;
        cloud_account::save_stored_account(&stored).unwrap();
    })
    .await
    .unwrap();

    // The local app: Juana Santos and her 1701Q for Q1 2026, queued.
    let db = Database::open(&database).unwrap();
    let taxpayer = db
        .create_taxpayer(NewTaxpayer {
            tin_root: TIN_ROOT.into(),
            name: "Juana Santos".into(),
            taxpayer_type: bir_core::profile::TaxpayerType::Individual,
            birth_date: chrono::NaiveDate::from_ymd_opt(1990, 1, 15),
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
    let profile = db.get_profile(&tin).unwrap().expect("the head office's profile");
    let mut draft = Form1701QDraft::new_from_profile(&profile, 2026, 1);
    draft.filer_type = Some(Form1701QFilerType::SingleProprietor);
    draft.atc = Some(Form1701QAtc::Ii012);
    draft.tax_rate = Some(Form1701QTaxRate::Graduated);
    draft.deduction_method = Some(Form1701QDeductionMethod::Osd);
    draft.compute();
    let errors = draft.validate();
    assert!(errors.is_empty(), "{errors:?}");
    db.save_queueable_draft(&draft).unwrap();
    draft.queue(QueueAuthSource::Gui).unwrap();
    db.save_queued_queueable(&draft).unwrap();
    assert_eq!(db.list_all_queued_submissions().unwrap().len(), 1);

    // The queue worker sends it through the relay.
    let db = Arc::new(Mutex::new(db));
    let worker = tokio::spawn(bir_core::background_cron::start_cron_jobs(db.clone()));
    let load = || db.lock().unwrap().get_queueable_draft::<Form1701QDraft>(&tin, 2026, 1).unwrap().expect("the 1701Q");
    let mut sent = load();
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        sent = load();
        if sent.lifecycle.status != FilingStatus::Queued {
            break;
        }
    }
    bir_core::background_cron::request_shutdown();
    bir_core::background_cron::wake();
    tokio::time::timeout(Duration::from_secs(15), worker).await.expect("the worker stopped").unwrap();
    let lifecycle = &sent.lifecycle;
    assert_eq!(lifecycle.status, FilingStatus::Submitted, "queue state: {:?}", lifecycle.submission_error);
    assert!(!lifecycle.submitted_dry_run, "the Sender sent it (to the local fake BIR)");

    // The receipt is kept on the return and verifies against the signed
    // trust list.
    let receipt_jws = lifecycle.relay_receipt.clone().expect("receipt stored");
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
    // The receipt names that same ticket: verified with the jti required,
    // by the worker's own client.
    let receipt = bir_core::relay::submitter()
        .unwrap()
        .verify_receipt(&receipt_jws, &xml_sha256, Some(&ticket_jti))
        .await
        .expect("receipt verifies for this XML and this ticket");
    assert_eq!(receipt.jti.as_deref(), Some(ticket_jti.as_str()));
    assert_eq!(receipt.filename, filename);
    assert_eq!(receipt.tin, "000-123-453-00000");
    assert_eq!((receipt.form_type.as_str(), receipt.period.as_str()), ("1701Qv2018", "2026Q1"));
    assert_eq!(receipt.status, "sent");
    assert_eq!(receipt.mode, "live");
    assert!(std::str::from_utf8(&xml).unwrap().contains("frm1701q:txtTIN3=453"));
    println!("V6 e2e: 1701Q for {TIN} sent; receipt {} verified", receipt.sub);
}
