//! This node's own relay identity: an ed25519 signing key and an
//! X25519 sealing key, two independent 32-byte raw files under this shared store's own `relay`
//! module dir — a sibling of `mesh/`, deliberately not inside it, because this relationship has
//! nothing to do with mesh/fleet identity. **Never through `adi-secrets`**: these are node
//! identity material, the same shape `adi-mesh::identity`/`devicekey` already are, not a value a
//! run resolves into its environment.
//!
//! The only function in this crate that touches the shared store at all is [`load_or_create`]
//! (and [`load_or_create_in`]) — `adi-relay-server` depends on this crate for [`crate::openssh`],
//! [`crate::sign::verify`], and [`crate::seal::seal`], and calls none of these.

use adi_config::Config;
use anyhow::Context as _;
use ed25519_dalek::SigningKey;
use x25519_dalek::{PublicKey, StaticSecret};

/// The shared-store module this crate owns: `~/.adi/mono/relay/`.
pub const MODULE: &str = "relay";

const SIGNING_KEY_FILE: &str = "signing.key";
const SEALING_KEY_FILE: &str = "sealing.key";

/// This node's two relay keys, loaded together because every relay-backed trigger on the node
/// shares the one pair: one pair per node, not per trigger — the relay tells triggers apart by
/// `endpoint_id`, not by key.
pub struct RelayIdentity {
    pub signing: SigningKey,
    pub sealing: StaticSecret,
}

/// Neither key type implements `Debug` (secret material never should) — print only what's safe
/// to print: the rendered public signing key.
impl std::fmt::Debug for RelayIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayIdentity")
            .field("signing_public", &self.public_key_line())
            .finish_non_exhaustive()
    }
}

impl RelayIdentity {
    /// This node's OpenSSH-rendered public signing key — what every signed request's
    /// `X-Relay-Key` header carries and the panel's "copy public key" button shows.
    #[must_use]
    pub fn public_key_line(&self) -> String {
        crate::openssh::encode(&self.signing.verifying_key())
    }

    /// This node's public sealing key, raw 32 bytes — what `register`'s body sends as
    /// `sealing_key`, base64-encoded by the caller, not here.
    #[must_use]
    pub fn sealing_public(&self) -> [u8; 32] {
        *PublicKey::from(&self.sealing).as_bytes()
    }
}

/// Load this node's relay keys, generating and saving a fresh pair on first use — the same
/// `load_or_create` shape `adi-mesh::identity`/`devicekey` already use, and deliberately not
/// sharing their implementation: conflating this with the mesh identity key would make "can
/// prove this request came from the node" and "can read this node's inbox" one capability,
/// which is exactly the mistake to avoid.
///
/// # Errors
/// Fails if either key file exists but is not exactly 32 bytes, or on any store I/O error.
pub fn load_or_create() -> anyhow::Result<RelayIdentity> {
    load_or_create_in(&Config::open())
}

/// [`load_or_create`] against an explicit store — for tests and alternate installs.
///
/// # Errors
/// Fails if either key file exists but is not exactly 32 bytes, or on any store I/O error.
pub fn load_or_create_in(store: &Config) -> anyhow::Result<RelayIdentity> {
    let module = store.module(MODULE);
    let signing = load_or_create_32(&module, SIGNING_KEY_FILE).context("the relay signing key")?;
    let sealing = load_or_create_32(&module, SEALING_KEY_FILE).context("the relay sealing key")?;
    Ok(RelayIdentity {
        signing: SigningKey::from_bytes(&signing),
        sealing: StaticSecret::from(sealing),
    })
}

fn load_or_create_32(module: &adi_config::Module, file: &str) -> anyhow::Result<[u8; 32]> {
    if let Some(bytes) = module.read_raw(file)? {
        return bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "key at {} is {} bytes, expected 32",
                module.raw_path(file).display(),
                bytes.len()
            )
        });
    }
    let key = generate_32();
    module.write_raw(file, &key)?;
    Ok(key)
}

/// A fresh 256-bit key from the OS CSPRNG — the same primitive `adi-mesh::devicekey` uses, kept
/// as a one-line duplicate rather than a new cross-crate dependency.
fn generate_32() -> [u8; 32] {
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
            "adi-relay-keys-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn a_fresh_store_mints_a_pair_and_persists_it() {
        let store = scratch("fresh");
        let first = load_or_create_in(&store).expect("mint");
        let second = load_or_create_in(&store).expect("reload");
        assert_eq!(first.signing.to_bytes(), second.signing.to_bytes());
        assert_eq!(first.sealing.to_bytes(), second.sealing.to_bytes());
    }

    #[test]
    fn two_stores_mint_different_pairs() {
        let a = load_or_create_in(&scratch("a")).expect("a");
        let b = load_or_create_in(&scratch("b")).expect("b");
        assert_ne!(a.signing.to_bytes(), b.signing.to_bytes());
        assert_ne!(a.sealing.to_bytes(), b.sealing.to_bytes());
    }

    #[test]
    fn signing_and_sealing_keys_are_never_equal() {
        let id = load_or_create_in(&scratch("distinct")).expect("mint");
        assert_ne!(id.signing.to_bytes(), id.sealing.to_bytes());
    }

    #[test]
    fn public_key_line_round_trips_through_openssh() {
        let id = load_or_create_in(&scratch("line")).expect("mint");
        let line = id.public_key_line();
        assert_eq!(
            crate::openssh::parse(&line).expect("parses"),
            id.signing.verifying_key()
        );
    }

    #[cfg(unix)]
    #[test]
    fn both_key_files_are_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let store = scratch("perms");
        load_or_create_in(&store).expect("mint");
        let module = store.module(MODULE);
        for file in [SIGNING_KEY_FILE, SEALING_KEY_FILE] {
            let mode = std::fs::metadata(module.raw_path(file))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{file} must be 0600");
        }
    }

    #[test]
    fn a_wrong_length_key_file_is_rejected_not_rotated() {
        let store = scratch("wrong-length");
        store
            .module(MODULE)
            .write_raw(SIGNING_KEY_FILE, &[7u8; 16])
            .expect("write a bad key file");
        assert!(load_or_create_in(&store).is_err());
    }
}
