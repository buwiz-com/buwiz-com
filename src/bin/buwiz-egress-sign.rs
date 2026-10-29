//! Offline tool for the egress list (see `buwiz_cloud::egress`). Run it on
//! a machine that keeps the egress signing key; the server never sees it.
//!
//! buwiz-egress-sign keygen KEY_FILE
//!     Writes a new private key (base64 seed) to KEY_FILE (mode 0600, must
//!     not exist) and prints the public key to pin in apps.
//! buwiz-egress-sign sign KEY_FILE DAYS HOST:PORT...
//!     Prints the signed list, valid for DAYS, for `EGRESS_LIST_JWS`.

use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => keygen(&args[1]),
        Some("sign") if args.len() >= 4 => {
            let days: i64 = args[2].parse().context("DAYS must be a number")?;
            sign(&args[1], days, &args[3..])
        }
        _ => bail!("usage: buwiz-egress-sign keygen KEY_FILE | sign KEY_FILE DAYS HOST:PORT..."),
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
    println!("public key (pin in bir-submit EGRESS_KEYS): {}", URL_SAFE_NO_PAD.encode(public.as_bytes()));
    println!("kid: {}", buwiz_cloud::jws::key_id(&public));
    Ok(())
}

fn sign(path: &str, days: i64, hosts: &[String]) -> anyhow::Result<()> {
    if !(1..=730).contains(&days) {
        bail!("DAYS must be 1 to 730");
    }
    let raw = Zeroizing::new(std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?);
    let seed = Zeroizing::new(STANDARD.decode(raw.trim()).context("the key file is not base64")?);
    let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| anyhow::anyhow!("the key must be 32 bytes"))?;
    let key = SigningKey::from_bytes(&seed);
    let iat = time::OffsetDateTime::now_utc().unix_timestamp();
    println!("{}", buwiz_cloud::egress::sign_list(&key, hosts, iat, iat + days * 86_400)?);
    Ok(())
}
