//! The upload server's SSH host key, read from the bytes the Gatekeeper
//! bridges. An SSH server sends its key in the key-exchange reply (message
//! 31, `KEXDH_REPLY` / `KEX_ECDH_REPLY`, or 33, `KEX_DH_GEX_REPLY`), before
//! `NEWKEYS` (21) switches encryption on, so it passes in clear (RFC 4253
//! §6, §8; RFC 5656 §4; RFC 4419 §3). This reads the server's side of the
//! stream up to that point and gives the key's `SHA256:` fingerprint, the
//! form `BIR_SSH_HOST_KEYS` uses. It reads at most the first 64 KB and gives
//! up, silently, on anything it does not understand.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use sha2::{Digest, Sha256};

/// The most of a stream it reads.
pub const MAX_BYTES: usize = 64 * 1024;
/// The longest binary packet RFC 4253 §6.1 makes every implementation take.
const MAX_PACKET: usize = 35_000;
/// Lines a server may send before its version line.
const MAX_LINES: usize = 20;
const MAX_LINE: usize = 1024;

const MSG_NEWKEYS: u8 = 21;
const MSG_KEX_REPLY: u8 = 31;
const MSG_KEX_GEX_REPLY: u8 = 33;

/// Host-key types a server may offer (not certificates: the Sender
/// refuses those; nor security-key types, which only users hold).
const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-ed448",
    "ssh-rsa",
    "ssh-dss",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// `SHA256:` and the unpadded base64 of SHA-256 over the key blob, as
/// OpenSSH writes it.
pub fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Before the version line; `lines` read so far.
    Banner {
        lines: usize,
    },
    Packets,
    Done,
}

/// Reads one stream's server bytes, as they come.
pub struct HostKeyReader {
    buffer: Vec<u8>,
    /// Where parsing stands in `buffer`.
    at: usize,
    state: State,
}

impl Default for HostKeyReader {
    fn default() -> Self {
        Self::new()
    }
}

impl HostKeyReader {
    pub fn new() -> Self {
        Self { buffer: Vec::new(), at: 0, state: State::Banner { lines: 0 } }
    }

    /// Whether it found the key or gave up: it reads nothing more.
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Takes the next bytes from the server. Gives the fingerprint once,
    /// when it finds the key.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<String> {
        if self.state == State::Done {
            return None;
        }
        let room = MAX_BYTES - self.buffer.len();
        self.buffer.extend_from_slice(&bytes[..bytes.len().min(room)]);
        let found = self.parse();
        if found.is_some() || self.buffer.len() >= MAX_BYTES {
            self.done();
        }
        found
    }

    fn done(&mut self) {
        self.state = State::Done;
        self.buffer = Vec::new();
        self.at = 0;
    }

    fn parse(&mut self) -> Option<String> {
        loop {
            match self.state {
                State::Done => return None,
                State::Banner { lines } => {
                    let rest = &self.buffer[self.at..];
                    let Some(end) = rest.iter().position(|&byte| byte == b'\n') else {
                        if rest.len() > MAX_LINE {
                            self.done();
                        }
                        return None;
                    };
                    let line = &rest[..end];
                    self.at += end + 1;
                    if line.starts_with(b"SSH-") {
                        if line.starts_with(b"SSH-2.0-") || line.starts_with(b"SSH-1.99-") {
                            self.state = State::Packets;
                        } else {
                            self.done();
                        }
                    } else if end > MAX_LINE || lines + 1 >= MAX_LINES {
                        self.done();
                    } else {
                        self.state = State::Banner { lines: lines + 1 };
                    }
                }
                State::Packets => {
                    let rest = &self.buffer[self.at..];
                    if rest.len() < 5 {
                        return None;
                    }
                    let length = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
                    let padding = rest[4] as usize;
                    // Before NEWKEYS: no MAC, padding of at least 4, the
                    // whole packet a multiple of 8 is not checked (some
                    // servers pad to 16).
                    if !(5..=MAX_PACKET).contains(&length) || padding < 4 || padding + 1 >= length {
                        self.done();
                        return None;
                    }
                    if rest.len() < 4 + length {
                        return None;
                    }
                    let payload = &rest[5..4 + length - padding];
                    self.at += 4 + length;
                    match payload.first().copied() {
                        Some(MSG_NEWKEYS) => {
                            self.done();
                            return None;
                        }
                        // 31 is the group of a group exchange too (RFC 4419
                        // `KEX_DH_GEX_GROUP`), whose first field is a prime,
                        // not a key: it reads as no key and the reply (33)
                        // comes after.
                        Some(MSG_KEX_REPLY | MSG_KEX_GEX_REPLY) => {
                            if let Some(blob) = host_key_blob(&payload[1..]) {
                                return Some(fingerprint(blob));
                            }
                        }
                        Some(_) => {}
                        None => {
                            self.done();
                            return None;
                        }
                    }
                }
            }
        }
    }
}

/// An SSH `string`: its length, then its bytes. (it, the rest)
fn ssh_string(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let body = bytes.get(4..4usize.checked_add(length)?)?;
    Some((body, &bytes[4 + length..]))
}

/// The reply's first field, the host key `K_S`, if it reads as one.
fn host_key_blob(fields: &[u8]) -> Option<&[u8]> {
    let (blob, _) = ssh_string(fields)?;
    let (name, key) = ssh_string(blob)?;
    let name = std::str::from_utf8(name).ok()?;
    if !KEY_TYPES.contains(&name) {
        return None;
    }
    // An Ed25519 key is exactly its 32 bytes.
    if name == "ssh-ed25519" {
        let (point, rest) = ssh_string(key)?;
        if point.len() != 32 || !rest.is_empty() {
            return None;
        }
    }
    Some(blob)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};

    fn string(bytes: &[u8]) -> Vec<u8> {
        let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(bytes);
        out
    }

    /// A binary packet before NEWKEYS: length, padding length, payload,
    /// padding (to a multiple of 8, at least 4), no MAC.
    fn packet(payload: &[u8]) -> Vec<u8> {
        let mut padding = 8 - (payload.len() + 5) % 8;
        if padding < 4 {
            padding += 8;
        }
        let length = payload.len() + padding + 1;
        let mut out = (length as u32).to_be_bytes().to_vec();
        out.push(padding as u8);
        out.extend_from_slice(payload);
        out.extend(std::iter::repeat_n(0x5a, padding));
        out
    }

    fn kexinit() -> Vec<u8> {
        let mut payload = vec![20];
        payload.extend_from_slice(&[7; 16]);
        for list in [
            "curve25519-sha256,ecdh-sha2-nistp256,diffie-hellman-group-exchange-sha256",
            "ssh-ed25519,ecdsa-sha2-nistp256",
            "aes128-ctr",
            "aes128-ctr",
            "hmac-sha2-256",
            "hmac-sha2-256",
            "none",
            "none",
            "",
            "",
        ] {
            payload.extend(string(list.as_bytes()));
        }
        payload.extend_from_slice(&[0, 0, 0, 0, 0]);
        payload
    }

    fn ed25519_blob(seed: u8) -> Vec<u8> {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let mut blob = string(b"ssh-ed25519");
        blob.extend(string(key.verifying_key().as_bytes()));
        blob
    }

    /// A made-up ECDSA P-256 key: the curve name and an uncompressed point.
    fn ecdsa_blob() -> Vec<u8> {
        let mut point = vec![4];
        point.extend((0..64).map(|byte| byte as u8));
        let mut blob = string(b"ecdsa-sha2-nistp256");
        blob.extend(string(b"nistp256"));
        blob.extend(string(&point));
        blob
    }

    /// `KEX_ECDH_REPLY` (curve25519 and ecdh-sha2 alike): K_S, Q_S, the
    /// signature.
    fn ecdh_reply(blob: &[u8]) -> Vec<u8> {
        let mut payload = vec![MSG_KEX_REPLY];
        payload.extend(string(blob));
        payload.extend(string(&[9; 32]));
        let mut signature = string(b"ssh-ed25519");
        signature.extend(string(&[3; 64]));
        payload.extend(string(&signature));
        payload
    }

    /// A server's side up to NEWKEYS.
    fn server_side(blob: &[u8], preamble: &str) -> Vec<u8> {
        let mut bytes = preamble.as_bytes().to_vec();
        bytes.extend_from_slice(b"SSH-2.0-OpenSSH_9.6 test\r\n");
        bytes.extend(packet(&kexinit()));
        bytes.extend(packet(&ecdh_reply(blob)));
        bytes.extend(packet(&[MSG_NEWKEYS]));
        bytes.extend_from_slice(&[0xc3; 300]);
        bytes
    }

    fn read_all(bytes: &[u8], chunk: usize) -> Option<String> {
        let mut reader = HostKeyReader::new();
        let mut found = None;
        for piece in bytes.chunks(chunk.max(1)) {
            if let Some(fingerprint) = reader.feed(piece) {
                assert!(found.is_none(), "the key is given once");
                found = Some(fingerprint);
            }
        }
        found
    }

    #[test]
    fn the_fingerprint_is_openssh_s() {
        // `ssh-keygen -lf` of this blob reads the same: SHA-256, base64
        // without padding.
        let blob = ed25519_blob(1);
        let expected = format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(&blob)));
        assert_eq!(fingerprint(&blob), expected);
        assert_eq!(expected.len(), "SHA256:".len() + 43);
        assert!(!expected.ends_with('='));
    }

    #[test]
    fn reads_the_key_from_a_curve25519_or_ecdh_reply() {
        for blob in [ed25519_blob(1), ed25519_blob(2), ecdsa_blob()] {
            let bytes = server_side(&blob, "");
            for chunk in [1, 3, 7, 64, 1500, bytes.len()] {
                assert_eq!(read_all(&bytes, chunk), Some(fingerprint(&blob)), "chunks of {chunk}");
            }
        }
        assert_ne!(fingerprint(&ed25519_blob(1)), fingerprint(&ed25519_blob(2)));
    }

    #[test]
    fn lines_before_the_version_are_skipped() {
        let blob = ed25519_blob(4);
        let bytes = server_side(&blob, "Welcome to the test server\r\nAuthorized use only\r\n");
        assert_eq!(read_all(&bytes, 5), Some(fingerprint(&blob)));
    }

    #[test]
    fn a_group_exchange_gives_its_key_in_the_reply_after_the_group() {
        let blob = ed25519_blob(5);
        let mut group = vec![MSG_KEX_REPLY];
        let mut prime = vec![0];
        prime.extend([0xff; 256]);
        group.extend(string(&prime));
        group.extend(string(&[2]));
        let mut reply = vec![MSG_KEX_GEX_REPLY];
        reply.extend(string(&blob));
        reply.extend(string(&[0x42; 256]));
        reply.extend(string(&[1; 83]));
        let mut bytes = b"SSH-2.0-test\r\n".to_vec();
        for payload in [kexinit(), group, reply, vec![MSG_NEWKEYS]] {
            bytes.extend(packet(&payload));
        }
        assert_eq!(read_all(&bytes, 11), Some(fingerprint(&blob)));
    }

    #[test]
    fn gives_up_on_what_it_does_not_understand() {
        let blob = ed25519_blob(6);
        let whole = server_side(&blob, "");
        // Cut before the reply is whole: nothing yet.
        let reply_ends = whole.len() - 300 - packet(&[MSG_NEWKEYS]).len();
        assert_eq!(read_all(&whole[..reply_ends - 1], 9), None);
        // Not SSH 2, a key type it does not know, a certificate, NEWKEYS
        // before any reply, a broken packet length.
        let cases: Vec<Vec<u8>> = vec![
            [b"SSH-1.5-old\r\n".as_slice(), &whole[26..]].concat(),
            server_side(&[string(b"ssh-foo").as_slice(), &string(&[1; 32])].concat(), ""),
            server_side(&[string(b"ssh-ed25519-cert-v01@openssh.com").as_slice(), &string(&[1; 32])].concat(), ""),
            server_side(&[string(b"ssh-ed25519").as_slice(), &string(&[1; 31])].concat(), ""),
            [b"SSH-2.0-x\r\n".as_slice(), &packet(&[MSG_NEWKEYS]), &packet(&ecdh_reply(&blob))].concat(),
            [b"SSH-2.0-x\r\n".as_slice(), &[0xff, 0xff, 0xff, 0xff, 4], &packet(&ecdh_reply(&blob))].concat(),
            [b"SSH-2.0-x\r\n".as_slice(), &[0, 0, 0, 12, 2], &[0; 12], &packet(&ecdh_reply(&blob))].concat(),
            vec![b'x'; 4000],
        ];
        for (index, bytes) in cases.iter().enumerate() {
            assert_eq!(read_all(bytes, 17), None, "case {index}");
        }
        // After giving up it reads nothing more.
        let mut reader = HostKeyReader::new();
        reader.feed(b"SSH-1.5-old\r\n");
        assert!(reader.is_done());
        assert_eq!(reader.feed(&whole), None);
    }

    #[test]
    fn reads_no_more_than_64_kb() {
        let blob = ed25519_blob(7);
        let mut bytes = b"SSH-2.0-x\r\n".to_vec();
        let filler = packet(&[2; 30_000]);
        while bytes.len() < MAX_BYTES {
            bytes.extend_from_slice(&filler);
        }
        bytes.extend(packet(&ecdh_reply(&blob)));
        let mut reader = HostKeyReader::new();
        assert_eq!(reader.feed(&bytes), None);
        assert!(reader.is_done());
    }

    /// Garbage, encrypted-looking bytes, and valid streams cut and bitten
    /// anywhere never panic, and what it finds in a damaged stream is a
    /// fingerprint of 43 characters.
    #[test]
    fn never_panics_on_any_bytes() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x5eed);
        let valid = server_side(&ed25519_blob(8), "banner\r\n");
        for round in 0..3000 {
            let mut bytes: Vec<u8> = match round % 3 {
                0 => (0..rng.random_range(0..5000)).map(|_| rng.random()).collect(),
                1 => {
                    let mut bytes = b"SSH-2.0-x\r\n".to_vec();
                    bytes.extend((0..rng.random_range(0..5000)).map(|_| rng.random::<u8>()));
                    bytes
                }
                _ => valid.clone(),
            };
            if round % 3 == 2 {
                for _ in 0..rng.random_range(1..6) {
                    let at = rng.random_range(0..bytes.len());
                    bytes[at] = rng.random();
                }
                bytes.truncate(rng.random_range(0..=bytes.len()));
            }
            let chunk = rng.random_range(1..400);
            if let Some(found) = read_all(&bytes, chunk) {
                assert!(found.starts_with("SHA256:") && found.len() == 50, "{found}");
            }
        }
    }
}
