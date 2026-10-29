//! Object storage for claim documents: any S3-compatible service
//! (Cloudflare R2 in production). Objects are already AES-256-GCM
//! ciphertext when they get here; the storage provider never sees a
//! readable document.

use std::time::Duration;

use anyhow::Context;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

const SIGNATURE_TTL: Duration = Duration::from_secs(60);

pub struct ObjectStore {
    bucket: Bucket,
    credentials: Credentials,
    http: reqwest::Client,
}

impl ObjectStore {
    pub fn new(endpoint: &str, region: &str, bucket: &str, key_id: &str, secret: &str) -> anyhow::Result<Self> {
        let endpoint = endpoint.parse().context("S3_ENDPOINT must be a URL")?;
        let bucket = Bucket::new(endpoint, UrlStyle::Path, bucket.to_owned(), region.to_owned())
            .context("S3 bucket settings are not valid")?;
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
        Ok(Self { bucket, credentials: Credentials::new(key_id, secret), http })
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        let url = self.bucket.put_object(Some(&self.credentials), key).sign(SIGNATURE_TTL);
        self.http.put(url).body(bytes).send().await?.error_for_status().context("object upload failed")?;
        Ok(())
    }

    /// The object's bytes, or `None` if it does not exist.
    pub async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let url = self.bucket.get_object(Some(&self.credentials), key).sign(SIGNATURE_TTL);
        let response = self.http.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes = response.error_for_status().context("object download failed")?.bytes().await?;
        Ok(Some(bytes.to_vec()))
    }

    /// Deleting a missing object succeeds.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let url = self.bucket.delete_object(Some(&self.credentials), key).sign(SIGNATURE_TTL);
        let response = self.http.delete(url).send().await?;
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            response.error_for_status().context("object delete failed")?;
        }
        Ok(())
    }
}

/// Where claim document ciphertext lives: in Postgres (development) or in
/// object storage (production).
pub enum DocumentStore {
    Database,
    Objects(Box<ObjectStore>),
}

impl DocumentStore {
    /// Removes objects whose rows were purged. Failures are logged; the
    /// bytes are ciphertext and the key that opens them stays with us.
    pub async fn delete_all(&self, keys: &[String]) {
        let Self::Objects(store) = self else { return };
        for key in keys {
            if let Err(error) = store.delete(key).await {
                tracing::error!(%key, error = ?error, "could not delete a claim document object");
            }
        }
    }
}
