//! Offline tool for the relay trust list (see `buwiz_cloud::filing::trust`). Run it
//! on a machine that keeps the trust signing key; the server never sees it.
//!
//! buwiz-trust-sign keygen KEY_FILE
//!     Writes a new private key (base64 seed) to KEY_FILE (mode 0600, must
//!     not exist) and prints the public key to pin in apps.
//! buwiz-trust-sign sign KEY_FILE DAYS SENDER_KEYS_FILE HOST:PORT...
//!     Prints the signed list, valid for DAYS, for `TRUST_LIST_JWS`.
//!     SENDER_KEYS_FILE is what `buwiz-sender public` prints, or a JSON
//!     array of several (old and new Sender keys while rotating).
//! buwiz-trust-sign public KEY_FILE
//!     Prints the public key of KEY_FILE (base64url), as `keygen` did.
//! buwiz-trust-sign verify PUBLIC_KEY JWS_FILE
//!     Checks that JWS_FILE is a trust list signed by PUBLIC_KEY and not
//!     expired, and prints what it lists.

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use buwiz_cloud::filing::trust::{self, Contents};
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
        Some("public") if args.len() == 2 => {
            println!("{}", URL_SAFE_NO_PAD.encode(read_key(&args[1])?.verifying_key().as_bytes()));
            Ok(())
        }
        Some("verify") if args.len() == 3 => verify(&args[1], &args[2]),
        _ => bail!(
            "usage: buwiz-trust-sign keygen KEY_FILE | sign KEY_FILE DAYS SENDER_KEYS_FILE HOST:PORT... \
             | public KEY_FILE | verify PUBLIC_KEY JWS_FILE"
        ),
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
    println!("kid: {}", buwiz_cloud::filing::jws::key_id(&public));
    Ok(())
}

fn sign(path: &str, days: i64, sender_keys: &str, hosts: &[String]) -> anyhow::Result<()> {
    if !(1..=730).contains(&days) {
        bail!("DAYS must be 1 to 730");
    }
    let key = read_key(path)?;
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

fn read_key(path: &str) -> anyhow::Result<SigningKey> {
    let raw = Zeroizing::new(std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?);
    let seed = Zeroizing::new(STANDARD.decode(raw.trim()).context("the key file is not base64")?);
    let seed: Zeroizing<[u8; 32]> =
        Zeroizing::new(seed.as_slice().try_into().map_err(|_| anyhow::anyhow!("the key must be 32 bytes"))?);
    Ok(SigningKey::from_bytes(&seed))
}

fn verify(public: &str, path: &str) -> anyhow::Result<()> {
    let bytes = URL_SAFE_NO_PAD.decode(public.trim()).context("PUBLIC_KEY is not base64url")?;
    let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| anyhow::anyhow!("PUBLIC_KEY must be 32 bytes"))?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).context("PUBLIC_KEY is not an Ed25519 key")?;
    let jws = std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?;
    let claims = buwiz_cloud::filing::jws::verify(&[key], trust::TYP, jws.trim())
        .map_err(|error| anyhow::anyhow!("not a trust list signed by this key: {error}"))?;
    let exp = claims["exp"].as_i64().context("the list has no exp")?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if exp <= now {
        bail!("the list expired");
    }
    let count = |field: &str| claims[field].as_array().map_or(0, Vec::len);
    let expires = time::OffsetDateTime::from_unix_timestamp(exp)?.date();
    println!(
        "ok: signed by {}, {} hosts, {} sealing keys, {} receipt keys, expires {expires}",
        buwiz_cloud::filing::jws::key_id(&key),
        count("hosts"),
        count("sealing_keys"),
        count("receipt_keys"),
    );
    Ok(())
}
