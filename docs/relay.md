# The submission relay

The relay files a return with BIR for an active TIN. The wire protocol is in
[relay-protocol.md](relay-protocol.md). This page covers how the server side
is put together.

## Two processes

**The Gatekeeper** (`buwiz-cloud`) is public:
- it checks devices and tickets;
- it stores the sealed envelopes, without being able to open them.

**The Sender** (`buwiz-sender`, `src/sender/`) is private. It alone keeps:
- BIR's passphrases;
- the SFTP login;
- the HPKE private key.

It also does the IAF encryption. This is byte-identical to eBIRForms'
`Encrypt.exe`. Without `SENDER_URL`, the Sender runs inside the Gatekeeper's
process (development only).

## v2 flow

1. The app signs each request with a registered device key (`src/devices.rs`).
2. It gets a five-minute ticket for one return (`src/tickets.rs`).
3. It seals the XML to the Sender with HPKE (`src/envelope.rs`).

v1 still works: plaintext XML over TLS.

The traffic leaves from the taxpayer's computer, so BIR sees the taxpayer's
own connection:
- the app is a dumb pipe that connects only to BIR's hosts;
- SSH runs end to end through it, with BIR's host key pinned.

Each send ends in a signed receipt (`RECEIPT_SIGNING_KEY`). The trust list
is served at `/.well-known/buwiz-trust.json`.

## The `bir` feature

The BIR-specific code is behind the `bir` cargo feature, in files kept out
of this public repository. That code is:
- BIR's dispatcher;
- the IAF encryption;
- the SFTP upload;
- BIR's host list;
- the v1 reference client.

| | `cargo build` (public) | `cargo build --features bir` (production) |
|---|---|---|
| Needs the private BIR files (`src/bir/`, `src/relay/client.rs`, `examples/iaf-decrypt.rs`, `tests/relay.rs`, `tests/iaf_golden.rs`, `tests/fixtures/iaf/`) | No | Yes |
| Accounts, TIN claims, devices, tickets, HPKE sealing, trust list, receipts | Yes | Yes |
| The Sender | Checks tickets and opens envelopes, then answers every send `failed`, "relay not built in". Nothing reaches BIR, so it is safe to retry. | Sends through the relay |
| `buwiz-relay`, `iaf-decrypt`, the relay tests | Not built | Built |

BIR's hosts and the other BIR details are documented with those files.

## Relay modes

These need a build with `--features bir`. `BIR_RELAY_MODE` decides how far
a submission goes:

- `dry_run` (default): encrypts and checks everything, sends nothing, and
  returns a receipt marked `dry_run`.
- `lab`: uploads to a test SFTP server, set in `BIR_LAB_SFTP`.
- `live`: files with BIR.

Lab test with a real OpenSSH server:

```sh
docker run -d -p 127.0.0.1:2222:22 atmoz/sftp lab:labpass:1001::1601Cv2018
ssh-keyscan -p 2222 -t ed25519 127.0.0.1 | ssh-keygen -lf -      # pin this
BIR_RELAY_MODE=lab BIR_LAB_SFTP=lab:labpass@127.0.0.1:2222 \
  BIR_SSH_HOST_KEYS=SHA256:… BIR_IAF_PASSPHRASE=lab cargo run --features bir
# Sign in, get a TIN approved, then act as the app:
cargo run --features bir --bin buwiz-relay -- --server http://localhost:3008 --token <access token> \
  --file return.xml --filename '12345678900000-1601Cv2018-062025#you@example.com#.xml' \
  --allow 127.0.0.1:2222
```
