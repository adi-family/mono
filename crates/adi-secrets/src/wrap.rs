//! The device-wrap crypto for a [`crate::Scope::User`] secret (`docs/byo-credentials.md` §8):
//! a fresh per-secret data key (DEK) seals the value, and the DEK itself is sealed ("wrapped")
//! to the device's own key. Both steps are plain `XChaCha20-Poly1305`, the same primitive
//! [`crate::crypto`] uses for the master key — only the key and the AAD differ.
//!
//! **Nothing here runs on the server with a plaintext value in hand.** A device calls
//! [`seal_value`]/[`wrap_dek`] *before* a secret ever reaches `POST /api/secrets/mine/set`; the
//! server only ever calls [`Secrets::set_wrapped`](crate::Secrets::set_wrapped), which stores
//! what it is given and decrypts none of it. The reverse half — [`open_value`]/[`unwrap_dek`] —
//! is for the device's own unlock path (`docs/byo-credentials.md` §9) and is never reachable from
//! any server-side code; this module doesn't know or care which process calls it, so that
//! boundary is enforced by who is given the device's key, not by anything here.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::error::{Error, Result};

/// A fresh 256-bit data-encryption key, one per secret (`docs/byo-credentials.md` §8: "setting a
/// new value mints a new DEK").
#[must_use]
pub fn generate_dek() -> [u8; 32] {
    let key = XChaCha20Poly1305::generate_key(&mut OsRng);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(key.as_slice());
    bytes
}

/// The AAD binding a user secret's value (or its wrapped DEK) to exactly this user and name —
/// the device-side mirror of [`crate::aad`], which the server never calls for a wrapped secret
/// since it never holds a key that could use it.
fn value_aad(user: &str, name: &str) -> String {
    format!("user:{user}/{name}")
}

/// The wrap's own AAD — distinct from [`value_aad`] (with a `wrap:` prefix) so a value's
/// ciphertext and its DEK's wrap, which sit side by side in one manifest, can never be swapped
/// for each other even though both are sealed under keys only the device holds.
fn wrap_aad(user: &str, name: &str) -> String {
    format!("wrap:{}", value_aad(user, name))
}

/// Seal `plaintext` under `dek`, bound to `user`/`name`. Returns `(nonce, ciphertext)`, both
/// base64 — exactly the shape `Manifest::nonce`/`ciphertext` store for a [`crate::Scope::User`]
/// secret.
///
/// # Errors
/// [`Error::Crypto`] if the underlying AEAD call fails (not expected to happen in practice).
pub fn seal_value(dek: &[u8; 32], user: &str, name: &str, plaintext: &[u8]) -> Result<(String, String)> {
    encrypt(dek, &value_aad(user, name), plaintext)
}

/// Open a value sealed by [`seal_value`] under the same `dek`/`user`/`name`.
///
/// # Errors
/// [`Error::Decrypt`] for a wrong key, a tampered ciphertext, or a value bound to a different
/// user or name.
pub fn open_value(dek: &[u8; 32], user: &str, name: &str, nonce: &str, ciphertext: &str) -> Result<Vec<u8>> {
    decrypt(dek, &value_aad(user, name), nonce, ciphertext)
}

/// Wrap `dek` under the device's own key-wrapping key, bound to `user`/`name` so a wrap copied
/// into another secret's manifest — or another user's — fails to open.
///
/// # Errors
/// [`Error::Crypto`] if the underlying AEAD call fails.
pub fn wrap_dek(wrap_key: &[u8; 32], user: &str, name: &str, dek: &[u8; 32]) -> Result<(String, String)> {
    encrypt(wrap_key, &wrap_aad(user, name), dek)
}

/// Unwrap a DEK sealed by [`wrap_dek`] under the same device key/`user`/`name`.
///
/// # Errors
/// [`Error::Decrypt`] for a wrong device key, a tampered wrap, or one bound to a different user
/// or name — including the common case of a device that isn't the one this secret was wrapped
/// to.
pub fn unwrap_dek(wrap_key: &[u8; 32], user: &str, name: &str, nonce: &str, wrapped: &str) -> Result<[u8; 32]> {
    let bytes = decrypt(wrap_key, &wrap_aad(user, name), nonce, wrapped)?;
    bytes.try_into().map_err(|_| Error::Decrypt)
}

fn encrypt(key: &[u8; 32], aad: &str, plaintext: &[u8]) -> Result<(String, String)> {
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

fn decrypt(key: &[u8; 32], aad: &str, nonce_b64: &str, ct_b64: &str) -> Result<Vec<u8>> {
    let nonce_bytes = B64.decode(nonce_b64).map_err(|_| Error::Decrypt)?;
    if nonce_bytes.len() != 24 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dek_round_trips_through_wrap_and_unwrap() {
        let wrap_key = generate_dek(); // any 32 random bytes will do for a key-wrapping key
        let dek = generate_dek();
        let (nonce, wrapped) = wrap_dek(&wrap_key, "u-aaa", "K", &dek).expect("wrap");
        assert_eq!(unwrap_dek(&wrap_key, "u-aaa", "K", &nonce, &wrapped).expect("unwrap"), dek);
    }

    #[test]
    fn a_value_round_trips_through_seal_and_open() {
        let dek = generate_dek();
        let (nonce, ct) = seal_value(&dek, "u-aaa", "K", b"sk-mine").expect("seal");
        assert_eq!(open_value(&dek, "u-aaa", "K", &nonce, &ct).expect("open"), b"sk-mine");
    }

    #[test]
    fn a_wrap_bound_to_one_user_or_name_fails_to_unwrap_for_another() {
        let wrap_key = generate_dek();
        let dek = generate_dek();
        let (nonce, wrapped) = wrap_dek(&wrap_key, "u-aaa", "K", &dek).expect("wrap");
        assert!(matches!(
            unwrap_dek(&wrap_key, "u-bbb", "K", &nonce, &wrapped),
            Err(Error::Decrypt)
        ));
        assert!(matches!(
            unwrap_dek(&wrap_key, "u-aaa", "OTHER", &nonce, &wrapped),
            Err(Error::Decrypt)
        ));
    }

    #[test]
    fn a_different_device_key_cannot_unwrap_the_dek() {
        let dek = generate_dek();
        let (nonce, wrapped) = wrap_dek(&generate_dek(), "u-aaa", "K", &dek).expect("wrap");
        assert!(matches!(
            unwrap_dek(&generate_dek(), "u-aaa", "K", &nonce, &wrapped),
            Err(Error::Decrypt)
        ));
    }

    #[test]
    fn a_value_sealed_for_one_secret_does_not_open_as_another() {
        let dek = generate_dek();
        let (nonce, ct) = seal_value(&dek, "u-aaa", "K", b"v").expect("seal");
        assert!(matches!(
            open_value(&dek, "u-aaa", "OTHER", &nonce, &ct),
            Err(Error::Decrypt)
        ));
    }
}
