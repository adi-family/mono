//! Encryption at rest for a queued webhook record, modelled directly on
//! `adi-mesh/src/lease_seal.rs` — the one other place in this codebase anonymously
//! encrypts a value to a known X25519 public key over an untrusted hop. [`seal`] is the relay's
//! side, run against the node's **long-lived** public sealing key (unlike `lease_seal`'s
//! ephemeral session key); [`open`] is the node's, the new half neither `lease_seal` nor
//! anything else in this tree has needed before now.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// The HKDF `info` binding the derived key to this exact use — so the same ECDH output, reused
/// nowhere else, could never collide with a different protocol's key.
const INFO: &[u8] = b"adi-webhook-relay-seal-v1";

/// One sealed record, exactly the three fields the pull response's wire contract carries and
/// the relay's own schema stores — base64 on the wire, raw bytes here; the caller decides which
/// it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub ephemeral_pubkey: [u8; 32],
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

impl Sealed {
    #[must_use]
    pub fn ephemeral_pubkey_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.ephemeral_pubkey)
    }

    #[must_use]
    pub fn nonce_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.nonce)
    }

    #[must_use]
    pub fn ciphertext_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.ciphertext)
    }

    /// Rebuild from the three base64 fields the pull response's JSON carries.
    ///
    /// # Errors
    /// `Err` with a short reason if any field isn't valid base64 or isn't the expected length.
    pub fn from_b64(
        ephemeral_pubkey: &str,
        nonce: &str,
        ciphertext: &str,
    ) -> Result<Self, &'static str> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let ephemeral_pubkey: [u8; 32] = b64
            .decode(ephemeral_pubkey)
            .map_err(|_| "ephemeral_pubkey is not valid base64")?
            .try_into()
            .map_err(|_| "ephemeral_pubkey must be 32 bytes")?;
        let nonce: [u8; 24] = b64
            .decode(nonce)
            .map_err(|_| "nonce is not valid base64")?
            .try_into()
            .map_err(|_| "nonce must be 24 bytes")?;
        let ciphertext = b64
            .decode(ciphertext)
            .map_err(|_| "ciphertext is not valid base64")?;
        Ok(Self {
            ephemeral_pubkey,
            nonce,
            ciphertext,
        })
    }
}

/// The AAD binding a ciphertext to the one record it was sealed for: a row copied or
/// replayed into a different record's slot fails to decrypt instead of opening as the wrong
/// thing — the same reasoning `lease_seal.rs`'s own AAD gives for itself.
#[must_use]
pub fn aad(endpoint_id: &str, record_id: i64) -> String {
    format!("{endpoint_id}|{record_id}")
}

/// Seal `plaintext` to `sealing_public` (the node's long-lived public sealing key) — the relay's
/// side. A fresh ephemeral keypair every call, discarded after sealing, never reused.
///
/// # Panics
/// Never in practice: encryption only fails on a key/nonce-size mismatch, and both are fixed by
/// this function's own types.
#[must_use]
pub fn seal(
    sealing_public: &[u8; 32],
    endpoint_id: &str,
    record_id: i64,
    plaintext: &[u8],
) -> Sealed {
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&PublicKey::from(*sealing_public));
    let key = derive_key(shared.as_bytes());

    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let aad_bytes = aad(endpoint_id, record_id);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad_bytes.as_bytes(),
            },
        )
        .expect("encrypting to a freshly derived key never fails");

    Sealed {
        ephemeral_pubkey: *ephemeral_public.as_bytes(),
        nonce: nonce
            .as_slice()
            .try_into()
            .expect("XChaCha20Poly1305's nonce is 24 bytes"),
        ciphertext,
    }
}

/// Everything that can go wrong opening a sealed record — deliberately one variant: every cause
/// (wrong AAD, tampered ciphertext, a `sealing_secret` that doesn't match the key the record was
/// sealed to) looks the same to a caller and none should be diagnosable from the outside, the
/// same stance `lease_seal::open` takes.
#[derive(Debug)]
pub struct OpenError;

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not open the sealed record")
    }
}

impl std::error::Error for OpenError {}

/// Open a record sealed by [`seal`] — the node's side, with its own long-lived sealing secret.
///
/// # Errors
/// [`OpenError`] on any decrypt failure.
pub fn open(
    sealing_secret: &StaticSecret,
    endpoint_id: &str,
    record_id: i64,
    sealed: &Sealed,
) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    let shared = sealing_secret.diffie_hellman(&PublicKey::from(sealed.ephemeral_pubkey));
    let key = derive_key(shared.as_bytes());
    let aad_bytes = aad(endpoint_id, record_id);

    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad: aad_bytes.as_bytes(),
            },
        )
        .map_err(|_| OpenError)?;
    Ok(Zeroizing::new(plaintext))
}

/// HKDF-SHA256 the raw ECDH output into a 32-byte AEAD key, bound to [`INFO`].
fn derive_key(shared_secret: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut key = Zeroizing::new([0u8; 32]);
    hk.expand(INFO, key.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_keys() -> (StaticSecret, [u8; 32]) {
        let secret = StaticSecret::from([9u8; 32]);
        let public = *PublicKey::from(&secret).as_bytes();
        (secret, public)
    }

    #[test]
    fn a_record_sealed_to_the_node_opens_with_its_secret() {
        let (secret, public) = node_keys();
        let sealed = seal(&public, "wh_abc", 1, b"hello webhook");
        let opened = open(&secret, "wh_abc", 1, &sealed).expect("open");
        assert_eq!(&*opened, b"hello webhook");
    }

    #[test]
    fn a_mismatched_endpoint_or_record_id_fails_to_open() {
        let (secret, public) = node_keys();
        let sealed = seal(&public, "wh_abc", 1, b"v");
        assert!(open(&secret, "wh_other", 1, &sealed).is_err());
        assert!(open(&secret, "wh_abc", 2, &sealed).is_err());
    }

    #[test]
    fn a_different_nodes_secret_cannot_open_it() {
        let (_secret_a, public_a) = node_keys();
        let secret_b = StaticSecret::from([3u8; 32]);
        let sealed = seal(&public_a, "wh_abc", 1, b"v");
        assert!(open(&secret_b, "wh_abc", 1, &sealed).is_err());
    }

    #[test]
    fn two_seals_of_the_same_value_use_different_ephemeral_keys_and_ciphertexts() {
        let (_secret, public) = node_keys();
        let first = seal(&public, "wh_abc", 1, b"v");
        let second = seal(&public, "wh_abc", 1, b"v");
        assert_ne!(first.ephemeral_pubkey, second.ephemeral_pubkey);
        assert_ne!(first.ciphertext, second.ciphertext);
    }

    #[test]
    fn sealed_round_trips_through_its_base64_fields() {
        let (secret, public) = node_keys();
        let sealed = seal(&public, "wh_abc", 7, b"payload bytes");
        let rebuilt = Sealed::from_b64(
            &sealed.ephemeral_pubkey_b64(),
            &sealed.nonce_b64(),
            &sealed.ciphertext_b64(),
        )
        .expect("rebuilds");
        assert_eq!(rebuilt, sealed);
        assert_eq!(
            &*open(&secret, "wh_abc", 7, &rebuilt).expect("open"),
            b"payload bytes"
        );
    }

    #[test]
    fn from_b64_rejects_a_wrong_length_field() {
        assert!(Sealed::from_b64("AAAA", "AAAA", "AAAA").is_err());
    }
}
