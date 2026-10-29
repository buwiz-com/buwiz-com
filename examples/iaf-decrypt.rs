//! Decrypts an IAF file to stdout: `iaf-decrypt FILE`, with
//! `BIR_IAF_PASSPHRASE` set. For support and lab checks.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: iaf-decrypt FILE"))?;
    let passphrase = std::env::var("BIR_IAF_PASSPHRASE")?;
    let plain = buwiz_cloud::bir::iaf::decrypt(&std::fs::read(path)?, &passphrase)?;
    std::io::Write::write_all(&mut std::io::stdout(), &plain)?;
    Ok(())
}
