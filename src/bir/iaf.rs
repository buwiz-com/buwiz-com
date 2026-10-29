//! IAF file encryption, byte-compatible with eBIRForms' `Encrypt.exe`:
//! zlib level 9, then DCPcrypt's AES-256-CBC. The key is SHA-256 of the
//! IAF passphrase; the IV is the key encrypting an all-zero block. DCPcrypt
//! finishes a partial last block in CFB style instead of padding, so the
//! ciphertext is as long as the compressed data.

use std::io::{Read, Write};

use aes::Aes256;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use sha2::{Digest, Sha256};

fn cipher_and_iv(passphrase: &str) -> (Aes256, [u8; 16]) {
    let key: [u8; 32] = Sha256::digest(passphrase.as_bytes()).into();
    let cipher = Aes256::new(&key.into());
    let mut iv = GenericArray::from([0u8; 16]);
    cipher.encrypt_block(&mut iv);
    (cipher, iv.into())
}

/// DCPcrypt's last partial block: XOR with the encrypted chain value. The
/// same operation encrypts and decrypts.
fn xor_tail(cipher: &Aes256, cv: [u8; 16], tail: &mut [u8]) {
    if tail.is_empty() {
        return;
    }
    let mut stream = GenericArray::from(cv);
    cipher.encrypt_block(&mut stream);
    for (byte, key) in tail.iter_mut().zip(stream) {
        *byte ^= key;
    }
}

/// Compresses and encrypts a plaintext IAF XML file for upload.
pub fn encrypt(plaintext: &[u8], passphrase: &str) -> std::io::Result<Vec<u8>> {
    // C zlib at level 9 reproduces Encrypt.exe's `78 DA` stream byte for byte.
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(plaintext)?;
    let mut data = encoder.finish()?;

    let (cipher, iv) = cipher_and_iv(passphrase);
    let mut cv = iv;
    let (blocks, tail) = data.as_chunks_mut::<16>();
    for block in blocks {
        for (byte, chain) in block.iter_mut().zip(cv) {
            *byte ^= chain;
        }
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
        cv = *block;
    }
    xor_tail(&cipher, cv, tail);
    Ok(data)
}

/// The inverse of [`encrypt`]. Used by tests and support tooling.
pub fn decrypt(ciphertext: &[u8], passphrase: &str) -> std::io::Result<Vec<u8>> {
    let (cipher, iv) = cipher_and_iv(passphrase);
    let mut data = ciphertext.to_vec();
    let mut cv = iv;
    let (blocks, tail) = data.as_chunks_mut::<16>();
    for block in blocks {
        let next_cv = *block;
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        for (byte, chain) in block.iter_mut().zip(cv) {
            *byte ^= chain;
        }
        cv = next_cv;
    }
    xor_tail(&cipher, cv, tail);
    let mut plaintext = Vec::new();
    ZlibDecoder::new(data.as_slice()).read_to_end(&mut plaintext)?;
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_at_and_off_block_boundaries() {
        for len in [0, 1, 15, 16, 17, 100, 4096, 4099] {
            let plaintext: Vec<u8> = (0..len).map(|index| (index * 31 % 251) as u8).collect();
            let encrypted = encrypt(&plaintext, "test passphrase").unwrap();
            assert_eq!(decrypt(&encrypted, "test passphrase").unwrap(), plaintext, "len {len}");
        }
    }

    #[test]
    fn output_is_as_long_as_the_zlib_stream() {
        let encrypted = encrypt(b"<xml/>", "test passphrase").unwrap();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(b"<xml/>").unwrap();
        assert_eq!(encrypted.len(), encoder.finish().unwrap().len());
    }

    #[test]
    fn a_different_passphrase_does_not_decrypt() {
        let encrypted = encrypt(b"<xml>hello</xml>", "right").unwrap();
        assert!(decrypt(&encrypted, "wrong").map_or(true, |plain| plain != b"<xml>hello</xml>"));
    }
}
