//! BIR's `tinDispatcherSFTP.php`: given a TIN and form, it answers which
//! SFTP server to upload to and with which login. Each field in the answer
//! is wrapped: `Base64(salt[32] || iv[16] || AES-256-CBC(PKCS7(BOM + text)))`
//! with the key PBKDF2-HMAC-SHA1(passphrase, salt, 100 000 rounds).
//!
//! The request runs over any byte stream, so the relay can send it through
//! the taxpayer's own connection. The app relaying it sees only the wrapped
//! fields.

use std::time::Duration;

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper_util::rt::TokioIo;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::Deserialize;
use sha1::Sha1;
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroizing;

pub const PATH: &str = "/tinDispatcherSFTP.php";
const CLIENT_VERSION: &str = "7.9.6.0";
const ROUNDS: u32 = 100_000;
const SALT_LEN: usize = 32;
const IV_LEN: usize = 16;
const MAX_BODY: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum DispatcherError {
    #[error("dispatcher request failed: {0}")]
    Http(String),
    #[error("dispatcher answered {0}")]
    Status(http::StatusCode),
    #[error("dispatcher answer was not understood: {0}")]
    Parse(String),
    #[error("dispatcher has no upload server for this TIN (mode 0)")]
    NoService,
    #[error("dispatcher field did not unwrap: {0}")]
    Unwrap(&'static str),
}

/// Where and how to log in, after unwrapping.
pub struct SftpLogin {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Zeroizing<String>,
}

impl std::fmt::Debug for SftpLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpLogin").field("host", &self.host).field("port", &self.port).finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Answer {
    mode: String,
    server: String,
    port: String,
    username: String,
    password: String,
}

type Encryptor = cbc::Encryptor<Aes256>;
type Decryptor = cbc::Decryptor<Aes256>;

fn key(passphrase: &str, salt: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac::<Sha1>(passphrase.as_bytes(), salt, ROUNDS, key.as_mut());
    key
}

pub fn unwrap_field(wrapped: &str, passphrase: &str) -> Result<String, DispatcherError> {
    let raw = STANDARD.decode(wrapped.trim()).map_err(|_| DispatcherError::Unwrap("not base64"))?;
    if raw.len() < SALT_LEN + IV_LEN + 16 {
        return Err(DispatcherError::Unwrap("too short"));
    }
    let (salt, rest) = raw.split_at(SALT_LEN);
    let (iv, ciphertext) = rest.split_at(IV_LEN);
    let mut buffer = ciphertext.to_vec();
    let plain = Decryptor::new(key(passphrase, salt).as_ref().into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| DispatcherError::Unwrap("bad padding"))?;
    let text = std::str::from_utf8(plain).map_err(|_| DispatcherError::Unwrap("not UTF-8"))?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

/// The inverse of [`unwrap_field`], for test dispatchers.
pub fn wrap_field(plain: &str, passphrase: &str) -> String {
    let mut salt = [0u8; SALT_LEN];
    let mut iv = [0u8; IV_LEN];
    rand::rng().fill_bytes(&mut salt);
    rand::rng().fill_bytes(&mut iv);
    let mut payload = "\u{feff}".as_bytes().to_vec();
    payload.extend_from_slice(plain.as_bytes());
    let ciphertext =
        Encryptor::new(key(passphrase, &salt).as_ref().into(), (&iv).into()).encrypt_padded_vec_mut::<Pkcs7>(&payload);
    let mut out = Vec::with_capacity(SALT_LEN + IV_LEN + ciphertext.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ciphertext);
    STANDARD.encode(out)
}

/// Reads an answer body. The official one uses single quotes.
pub fn parse_answer(body: &str, passphrase: &str) -> Result<SftpLogin, DispatcherError> {
    let answer: Answer = serde_json::from_str(&body.trim().replace('\'', "\""))
        .map_err(|error| DispatcherError::Parse(error.to_string()))?;
    if answer.mode == "0" {
        return Err(DispatcherError::NoService);
    }
    Ok(SftpLogin {
        host: unwrap_field(&answer.server, passphrase)?,
        port: answer.port.trim().parse().map_err(|_| DispatcherError::Parse("port".into()))?,
        username: unwrap_field(&answer.username, passphrase)?,
        password: Zeroizing::new(unwrap_field(&answer.password, passphrase)?),
    })
}

/// `GET /tinDispatcherSFTP.php?t=…&f=…&v=…` over `stream`, which is already
/// connected (and TLS-wrapped if needed) to `host`.
pub async fn fetch<S>(stream: S, host: &str, tin_root: &str, form_type: &str) -> Result<String, DispatcherError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let request = async {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|error| DispatcherError::Http(error.to_string()))?;
        tokio::spawn(connection);
        let uri = format!("{PATH}?t={tin_root}&f={form_type}&v={CLIENT_VERSION}");
        let request = http::Request::get(uri)
            .header(http::header::HOST, host)
            .header(http::header::USER_AGENT, "eBIRForms")
            .body(Empty::<Bytes>::new())
            .map_err(|error| DispatcherError::Http(error.to_string()))?;
        let response = sender.send_request(request).await.map_err(|error| DispatcherError::Http(error.to_string()))?;
        if !response.status().is_success() {
            return Err(DispatcherError::Status(response.status()));
        }
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|error| DispatcherError::Http(error.to_string()))?
            .to_bytes();
        String::from_utf8(body.to_vec()).map_err(|_| DispatcherError::Parse("body is not UTF-8".into()))
    };
    tokio::time::timeout(TIMEOUT, request).await.map_err(|_| DispatcherError::Http("timed out".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_and_lose_the_bom() {
        let wrapped = wrap_field("ebf2.bir.gov.ph", "test passphrase");
        assert_eq!(unwrap_field(&wrapped, "test passphrase").unwrap(), "ebf2.bir.gov.ph");
        assert!(unwrap_field(&wrapped, "other").is_err());
    }

    #[test]
    fn reads_the_official_single_quoted_answer() {
        let pass = "test passphrase";
        let body = format!(
            "{{'mode':'1','server':'{}','SSLPort':'443','port':'22','username':'{}','password':'{}'}}",
            wrap_field("ebf2.bir.gov.ph", pass),
            wrap_field("user", pass),
            wrap_field("secret", pass),
        );
        let login = parse_answer(&body, pass).unwrap();
        assert_eq!((login.host.as_str(), login.port, login.username.as_str()), ("ebf2.bir.gov.ph", 22, "user"));
        assert_eq!(login.password.as_str(), "secret");
        assert!(!format!("{login:?}").contains("secret"));
    }

    #[test]
    fn mode_zero_means_no_service() {
        let body = "{'mode':'0','server':'','port':'','username':'','password':''}";
        assert!(matches!(parse_answer(body, "x"), Err(DispatcherError::NoService)));
    }
}
