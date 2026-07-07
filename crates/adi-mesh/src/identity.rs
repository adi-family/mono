//! This machine's stable iroh identity.
//!
//! The [`SecretKey`] is generated once and persisted as 32 raw bytes at
//! `~/.adi/mono/mesh/identity.key`, so the derived [`EndpointId`] a peer dials stays the
//! same across restarts. Load it before binding the endpoint; peers reference the machine
//! by the resulting id.

use adi_config::Config;
use anyhow::Context as _;
use iroh::{EndpointId, SecretKey};

/// The raw file holding the 32-byte secret key within the `mesh` module dir.
const IDENTITY_FILE: &str = "identity.key";

/// Load the persisted secret key, generating and saving one on first run.
///
/// # Errors
/// Fails if the key file exists but is not exactly 32 bytes, or on any store I/O error.
pub fn load_or_create() -> anyhow::Result<SecretKey> {
    let module = Config::open().module(crate::config::MODULE);
    if let Some(bytes) = module.read_raw(IDENTITY_FILE)? {
        let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "identity key at {} is {} bytes, expected 32",
                module.raw_path(IDENTITY_FILE).display(),
                bytes.len()
            )
        })?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let secret = SecretKey::generate();
    module
        .write_raw(IDENTITY_FILE, &secret.to_bytes())
        .context("persisting the mesh identity key")?;
    Ok(secret)
}

/// This machine's [`EndpointId`] — the value peers dial — without binding an endpoint.
///
/// # Errors
/// Propagates any error from [`load_or_create`].
pub fn endpoint_id() -> anyhow::Result<EndpointId> {
    Ok(load_or_create()?.public())
}
