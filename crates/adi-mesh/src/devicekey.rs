//! This device's own key-wrapping key (`docs/byo-credentials.md` §8) — the key that seals (and
//! only ever this device unseals) the DEK behind each of this user's own secrets wrapped here.
//!
//! **Decided 2026-10-28 (Igor), replacing the original design's Keychain/WebAuthn PRF plan for
//! v1:** a random 256-bit key, persisted as 32 raw bytes in a `0600` file in this machine's own
//! `mesh` module dir — [`KEY_FILE`], beside [`identity`](crate::identity)'s own key file, but
//! never the same key or the same file. The mesh identity key authenticates a *connection*;
//! reusing it here would make "can pair as you" and "can read your subscription" one capability
//! (§8's own reasoning for why they must differ). A macOS Keychain item and a browser
//! `adi-mesh-client` `WebAuthn` PRF key are later hardening for the same role, not built here — see
//! `docs/byo-credentials.md` §15 Q12, which this decision settles as moot for v1: there is no
//! Keychain prompt to worry the separate `adi-mono` binary with, because nothing here calls the
//! Keychain at all.
//!
//! **Still device-owned code only.** Nothing in this module runs on a server unwrapping a value
//! that isn't its own — a server only ever stores what [`crate::devicekey::load_or_create`]'s
//! caller already sealed (`adi_secrets::wrap`), on whichever box this function's caller is
//! running on.

use adi_config::Config;
use anyhow::Context as _;

/// The raw file holding this device's 32-byte key-wrapping key, within the `mesh` module dir —
/// beside [`identity::IDENTITY_FILE`](crate::identity), never the same file.
const KEY_FILE: &str = "wrap.key";

/// The only wrap `kind` this build produces or opens (`adi_secrets::Wrap::kind`) — see this
/// module's own doc for why it isn't `"keychain-v1"`/`"webauthn-prf-v1"` yet.
pub const KIND: &str = "device-key-v1";

/// Load this device's key-wrapping key, generating and saving a fresh one on first use —
/// the same shape as [`identity::load_or_create`](crate::identity::load_or_create), and
/// deliberately not sharing its implementation: that function returns an iroh [`iroh::SecretKey`]
/// for a different purpose (authenticating a mesh connection), and conflating the two code paths
/// would risk the one mistake this key most needs to avoid — ending up derived from, or equal to,
/// the identity key.
///
/// # Errors
/// Fails if the key file exists but is not exactly 32 bytes, or on any store I/O error.
pub fn load_or_create() -> anyhow::Result<[u8; 32]> {
    load_or_create_in(&Config::open())
}

/// [`load_or_create`] against an explicit store — for tests and alternate installs.
///
/// # Errors
/// Fails if the key file exists but is not exactly 32 bytes, or on any store I/O error.
pub fn load_or_create_in(store: &Config) -> anyhow::Result<[u8; 32]> {
    let module = store.module(crate::config::MODULE);
    if let Some(bytes) = module.read_raw(KEY_FILE)? {
        return bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "device key at {} is {} bytes, expected 32",
                module.raw_path(KEY_FILE).display(),
                bytes.len()
            )
        });
    }
    let key = generate_key();
    module
        .write_raw(KEY_FILE, &key)
        .context("persisting this device's own key-wrapping key")?;
    Ok(key)
}

/// A fresh 256-bit key from the OS CSPRNG — the same primitive `adi_secrets::wrap::generate_dek`
/// uses, kept as a one-line duplicate here rather than a new cross-crate dependency: this crate
/// has no other reason to depend on `adi-secrets`, which itself depends on nothing in this one.
fn generate_key() -> [u8; 32] {
    use rand::TryRng as _;
    let mut bytes = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the OS random source is unavailable");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-devicekey-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn a_fresh_store_mints_a_key_and_persists_it() {
        let store = scratch("fresh");
        let key = load_or_create_in(&store).expect("mint");
        let reloaded = load_or_create_in(&store).expect("reload");
        assert_eq!(key, reloaded, "the same key comes back on a second call");
    }

    #[test]
    fn two_stores_mint_different_keys() {
        assert_ne!(
            load_or_create_in(&scratch("a")).expect("a"),
            load_or_create_in(&scratch("b")).expect("b"),
        );
    }

    #[test]
    fn the_device_key_is_never_the_identity_key() {
        let store = scratch("not-identity");
        let device = load_or_create_in(&store).expect("device key");
        let identity = crate::identity::load_or_create_in(&store)
            .expect("identity key")
            .to_bytes();
        assert_ne!(device, identity);
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let store = scratch("perms");
        load_or_create_in(&store).expect("mint");
        let path = store.module(crate::config::MODULE).raw_path(KEY_FILE);
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_wrong_length_key_file_is_rejected_not_rotated() {
        let store = scratch("wrong-length");
        store
            .module(crate::config::MODULE)
            .write_raw(KEY_FILE, &[7u8; 16])
            .expect("write a bad key file");
        assert!(load_or_create_in(&store).is_err());
    }
}
