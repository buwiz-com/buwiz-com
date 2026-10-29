//! Object storage against a real S3-compatible server. Runs only when
//! TEST_S3_ENDPOINT is set (with TEST_S3_KEY / TEST_S3_SECRET); creates the
//! bucket `buwiz-test` if needed.

use std::time::Duration;

use buwiz_cloud::storage::ObjectStore;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

#[tokio::test]
async fn objects_round_trip_and_delete() {
    let Ok(endpoint) = std::env::var("TEST_S3_ENDPOINT") else {
        eprintln!("skipped: TEST_S3_ENDPOINT is not set");
        return;
    };
    let key = std::env::var("TEST_S3_KEY").unwrap();
    let secret = std::env::var("TEST_S3_SECRET").unwrap();
    let bucket = Bucket::new(endpoint.parse().unwrap(), UrlStyle::Path, "buwiz-test", "auto").unwrap();
    let credentials = Credentials::new(&key, &secret);
    let _ = reqwest::Client::new()
        .put(bucket.create_bucket(&credentials).sign(Duration::from_secs(60)))
        .send()
        .await
        .unwrap();

    let store = ObjectStore::new(&endpoint, "auto", "buwiz-test", &key, &secret).unwrap();
    store.put("claims/a/b", b"ciphertext".to_vec()).await.unwrap();
    assert_eq!(store.get("claims/a/b").await.unwrap().as_deref(), Some(&b"ciphertext"[..]));
    assert_eq!(store.get("claims/a/missing").await.unwrap(), None);
    store.delete("claims/a/b").await.unwrap();
    assert_eq!(store.get("claims/a/b").await.unwrap(), None);
    store.delete("claims/a/b").await.unwrap();
}
