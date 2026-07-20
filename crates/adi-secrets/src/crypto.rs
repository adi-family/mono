//! Master key + authenticated encryption for secret values.
//!
//! Values are sealed with **XChaCha20-Poly1305**: a 256-bit master key, a fresh random
//! 192-bit nonce per write, and the secret's own location (`"<scope>/<name>"`) as additional
//! authenticated data — so a ciphertext copied into a different file, or a tampered tag, fails
//! to decrypt rather than silently returning a wrong or foreign value.
//!
//! The master key lives in a `0600` key-file (`~/.adi/mono/secrets/.master-key`, base64),
//! generated on first use. It sits beside the ciphertext, so this defeats plaintext-in-TOML,
//! accidental commits, and casual reads — not a full-store backup that captures the key too.
//! Set `ADI_SECRETS_KEY_FILE` to relocate the key outside the store (e.g. off a synced dir).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::error::{Error, Result};

/// Environment override for the master-key path — lets the key live outside the store dir.
const KEY_FILE_ENV: &str = "ADI_SECRETS_KEY_FILE";
/// Default key-file name within the `secrets/` module directory.
pub(crate) const KEY_FILE: &str = ".master-key";
/// XChaCha20-Poly1305 key length.
const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 nonce length (192-bit extended nonce).
const NONCE_LEN: usize = 24;

/// The master-key path: the `ADI_SECRETS_KEY_FILE` override, else `<secrets_dir>/.master-key`.
pub(crate) fn key_path(secrets_dir: &Path) -> PathBuf {
    std::env::var_os(KEY_FILE_ENV).map_or_else(|| secrets_dir.join(KEY_FILE), PathBuf::from)
}

/// Load the master key, generating and persisting a fresh one (`0600`) on first use. A
/// present-but-unreadable or wrong-length key file is an error, never silently rotated —
/// rotating would strand every existing ciphertext.
pub(crate) fn load_or_create_key(secrets_dir: &Path) -> Result<[u8; KEY_LEN]> {
    let path = key_path(secrets_dir);
    match std::fs::read_to_string(&path) {
        Ok(encoded) => decode_key(encoded.trim()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = generate_key();
            write_key(&path, &key)?;
            Ok(key)
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Encrypt `plaintext` for the secret at `aad` (its `"<scope>/<name>"`), returning
/// `(nonce, ciphertext)` both base64.
pub(crate) fn encrypt(key: &[u8; KEY_LEN], aad: &str, plaintext: &[u8]) -> Result<(String, String)> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| Error::Crypto("encryption failed".to_string()))?;
    Ok((B64.encode(nonce.as_slice()), B64.encode(ciphertext)))
}

/// Decrypt, verifying the authentication tag and the `aad` binding. Wrong key, a tampered
/// value, or a value moved out of its file all surface as [`Error::Decrypt`].
pub(crate) fn decrypt(key: &[u8; KEY_LEN], aad: &str, nonce_b64: &str, ct_b64: &str) -> Result<Vec<u8>> {
    let nonce_bytes = B64.decode(nonce_b64).map_err(|_| Error::Decrypt)?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(Error::Decrypt);
    }
    let ciphertext = B64.decode(ct_b64).map_err(|_| Error::Decrypt)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            XNonce::from_slice(&nonce_bytes),
            Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| Error::Decrypt)
}

/// A fresh 256-bit key from the OS CSPRNG.
fn generate_key() -> [u8; KEY_LEN] {
    let key = XChaCha20Poly1305::generate_key(&mut OsRng);
    let mut bytes = [0u8; KEY_LEN];
    bytes.copy_from_slice(key.as_slice());
    bytes
}

/// Parse a base64 key file into raw bytes, rejecting bad encoding or the wrong length.
fn decode_key(encoded: &str) -> Result<[u8; KEY_LEN]> {
    let raw = B64
        .decode(encoded)
        .map_err(|_| Error::Crypto("master key file is not valid base64".to_string()))?;
    raw.try_into()
        .map_err(|_| Error::Crypto("master key file has the wrong length".to_string()))
}

/// Write the base64 key with `0700` on its dir and `0600` on the file.
fn write_key(path: &Path, key: &[u8; KEY_LEN]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        crate::harden_dir(parent)?;
    }
    std::fs::write(path, B64.encode(key))?;
    crate::harden_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let key = generate_key();
        let (nonce, ct) = encrypt(&key, "global/API_KEY", b"hunter2").expect("encrypt");
        let plain = decrypt(&key, "global/API_KEY", &nonce, &ct).expect("decrypt");
        assert_eq!(plain, b"hunter2");
    }

    #[test]
    fn ciphertext_is_not_the_plaintext() {
        let key = generate_key();
        let (_, ct) = encrypt(&key, "global/X", b"plaintext-value").expect("encrypt");
        let raw = B64.decode(&ct).expect("b64");
        assert!(!raw.windows(9).any(|w| w == b"plaintext"));
    }

    #[test]
    fn a_different_key_fails_to_decrypt() {
        let (nonce, ct) = encrypt(&generate_key(), "global/X", b"v").expect("encrypt");
        assert!(matches!(
            decrypt(&generate_key(), "global/X", &nonce, &ct),
            Err(Error::Decrypt)
        ));
    }

    #[test]
    fn a_different_aad_fails_to_decrypt() {
        let key = generate_key();
        let (nonce, ct) = encrypt(&key, "global/X", b"v").expect("encrypt");
        // Same key + nonce + ciphertext, but the location it claims to live at differs.
        assert!(matches!(
            decrypt(&key, "projects/p/X", &nonce, &ct),
            Err(Error::Decrypt)
        ));
    }

    #[test]
    fn a_tampered_ciphertext_fails_to_decrypt() {
        let key = generate_key();
        let (nonce, ct) = encrypt(&key, "global/X", b"v").expect("encrypt");
        let mut raw = B64.decode(&ct).expect("b64");
        raw[0] ^= 0xff;
        let tampered = B64.encode(&raw);
        assert!(matches!(
            decrypt(&key, "global/X", &nonce, &tampered),
            Err(Error::Decrypt)
        ));
    }

    #[test]
    fn a_bad_key_file_is_rejected_not_rotated() {
        assert!(matches!(decode_key("not-base64!!"), Err(Error::Crypto(_))));
        assert!(matches!(decode_key(&B64.encode([0u8; 16])), Err(Error::Crypto(_))));
        assert!(decode_key(&B64.encode([7u8; 32])).is_ok());
    }
}
