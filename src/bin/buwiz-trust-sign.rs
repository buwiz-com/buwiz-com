//! Offline tool for the relay trust list (see `buwiz_cloud::trust`). Run it
//! on a machine that keeps the trust signing key; the server never sees it.
//!
//! buwiz-trust-sign keygen KEY_FILE
//!     Writes a new private key (base64 seed) to KEY_FILE (mode 0600, must
//!     not exist) and prints the public key to pin in apps.
//! buwiz-trust-sign sign KEY_FILE DAYS SENDER_KEYS_FILE HOST:PORT...
//!     Prints the signed list, valid for DAYS, for `TRUST_LIST_JWS`.
//!     SENDER_KEYS_FILE is what `buwiz-sender public` prints, or a JSON
//!     array of several (old and new Sender keys while rotating).

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use buwiz_cloud::trust::{self, Contents};
use ed25519_dalek::SigningKey;
use serde_json::Value;
use zeroize::Zeroizing;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => keygen(&args[1]),
        Some("sign") if args.len() >= 5 => {
            let days: i64 = args[2].parse().context("DAYS must be a number")?;
            sign(&args[1], days, &args[3], &args[4..])
        }
        _ => bail!("usage: buwiz-trust-sign keygen KEY_FILE | sign KEY_FILE DAYS SENDER_KEYS_FILE HOST:PORT..."),
    }
}

fn keygen(path: &str) -> anyhow::Result<()> {
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::RngCore::fill_bytes(&mut rand::rng(), seed.as_mut());
    let key = SigningKey::from_bytes(&seed);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).with_context(|| format!("could not create {path}"))?;
    std::io::Write::write_all(&mut file, STANDARD.encode(seed.as_ref()).as_bytes())?;
    let public = key.verifying_key();
    println!("public key (pin in bir-submit TRUST_KEYS): {}", URL_SAFE_NO_PAD.encode(public.as_bytes()));
    println!("kid: {}", buwiz_cloud::jws::key_id(&public));
    Ok(())
}

fn sign(path: &str, days: i64, sender_keys: &str, hosts: &[String]) -> anyhow::Result<()> {
    if !(1..=730).contains(&days) {
        bail!("DAYS must be 1 to 730");
    }
    let raw = Zeroizing::new(std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?);
    let seed = Zeroizing::new(STANDARD.decode(raw.trim()).context("the key file is not base64")?);
    let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| anyhow::anyhow!("the key must be 32 bytes"))?;
    let key = SigningKey::from_bytes(&seed);
    let senders: Value = serde_json::from_str(
        &std::fs::read_to_string(sender_keys).with_context(|| format!("could not read {sender_keys}"))?,
    )
    .context("SENDER_KEYS_FILE is not JSON")?;
    let mut contents = Contents { hosts: hosts.to_vec(), ..Contents::default() };
    match &senders {
        Value::Array(all) => all.iter().try_for_each(|public| contents.add_sender(public))?,
        public => contents.add_sender(public)?,
    }
    let iat = time::OffsetDateTime::now_utc().unix_timestamp();
    println!("{}", trust::sign_list(&key, &contents, iat, iat + days * 86_400)?);
    Ok(())
}
