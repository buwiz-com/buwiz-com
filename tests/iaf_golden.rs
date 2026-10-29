//! Our IAF encryption must match eBIRForms' `Encrypt.exe` byte for byte.
//! The fixtures are official outputs for sample returns with placeholder
//! taxpayer data. The passphrase is a server secret and is not in this
//! repository: run with `BIR_IAF_PASSPHRASE` set, or the test is skipped.

use buwiz_cloud::bir::iaf;

#[test]
fn matches_encrypt_exe_output() {
    let Ok(passphrase) = std::env::var("BIR_IAF_PASSPHRASE") else {
        eprintln!("skipped: BIR_IAF_PASSPHRASE is not set");
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/iaf");
    for stem in ["1601C-062025", "2550Q-122025Q1", "1701-122025"] {
        let plain = std::fs::read(dir.join(format!("{stem}.plain.xml"))).unwrap();
        let official = std::fs::read(dir.join(format!("{stem}.iaf.xml"))).unwrap();
        assert_eq!(iaf::encrypt(&plain, &passphrase).unwrap(), official, "{stem}");
        assert_eq!(iaf::decrypt(&official, &passphrase).unwrap(), plain, "{stem}");
    }
}
