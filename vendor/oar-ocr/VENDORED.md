# Vendored oar-ocr

This is oar-ocr 0.10.2 as published on crates.io, vendored only to raise its
`zip` requirement. Upstream declares `zip = "8.6"`, which has security
advisories (GHSA-m665-v7pm-7grj and others) fixed in zip 9.0.5. No upstream
release allows zip 9 yet, and oar-ocr's only use of zip (`src/indra/lm.rs`,
`ZipArchive::new`, `by_name`, `ZipError::FileNotFound`, `size`, `read_to_end`)
is unchanged in zip 9.

- Upstream: oar-ocr 0.10.2, .crate sha256
  `5efb1b5c83d5e18cf0082b4222b420b3c193694bbd2d27e78fbad5ee29ab517d`
  (the checksum Cargo.lock had for the registry package).
- Differences from the published crate: `zip` requirement `8.6` -> `9.0.5` in
  `Cargo.toml` and `Cargo.toml.orig`; `.cargo_vcs_info.json` and upstream's
  `.github/` (CI and Dependabot config, inert here) dropped; the
  published `Cargo.lock` is not committed (the crate's own `.gitignore` ignores
  it, and cargo ignores it for a dependency).

Remove this directory and the `[patch.crates-io]` entry in the root
`Cargo.toml` once an oar-ocr release allows zip >= 9.0.5.
