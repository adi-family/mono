//! The `auto_unlock` background renew worker (`docs/byo-credentials.md` §9, ADI-MONO-180 P2.4's
//! own gap, closed here by ADI-MONO-181): for every server connection in this device's own
//! `connections.toml` that has `auto_unlock` and a non-empty `share`, unlock on the first tick
//! and renew on an interval while the server is reachable — the device-side half of §9 steps 1-5,
//! run unattended.
//!
//! **A supervised loop inside `adi-app`, not a spawned process.** A run's background work dies
//! with it (every other worker in this crate — [`crate::prober`], [`crate::event_log`],
//! [`crate::live`] — is the same `tokio::spawn`/`std::thread::spawn` shape, for the same reason):
//! the device key this worker reads to unwrap a DEK, and the plaintext it briefly holds on the
//! way to resealing it for the server's session, must not outlive the process that read them in a
//! detached child nobody is watching.
//!
//! **The credential problem §9's own note names.** An unattended loop needs this device's
//! Basic-auth password for each server without a human retyping it every tick, which collides
//! with `docs/fleet.md` §3.1's "neither side stores it" — unless the operator has already handed
//! it to *this* `adi-app`, which is exactly what [`crate::viewer::HeldCredentials`] already holds,
//! at the user's own request, for dashboard transfer's outbound calls
//! (`adi-app/src/viewer.rs`'s `unlock`). Reusing that store here, rather than inventing a second
//! one, is the shape §9 suggested and this implements: a connection this device has never also
//! unlocked for dashboards (no password held) is skipped with a debug log, not an error — the
//! operator unlocks it once from the Fleet page and this worker picks it up on its next tick.

use std::collections::HashMap;
use std::time::Duration;

use adi_mesh::connections::{Connections, SecretsSettings};
use adi_mesh::gateway::NodeCredentials as _;
use adi_secrets::Secrets;
use base64::Engine as _;
use tracing::debug;

use crate::node::{self, CONTROL_TIMEOUT};
use crate::viewer;

/// How often the worker wakes to unlock or renew every `auto_unlock` connection. Comfortably
/// inside `leases::CONNECTED_GRACE` (90s, `lease = 0`'s own grace) so a connection sharing under
/// `lease = 0` never lapses between two ticks even on the slowest one, and a human watching
/// `mesh share --auto-unlock` take effect does not wait long for it.
const TICK: Duration = Duration::from_secs(20);

/// Start the worker. Nothing returned: like every other background loop in this crate, it runs
/// until the process exits.
pub fn start(secrets: Secrets) {
    tokio::spawn(run(secrets));
}

async fn run(secrets: Secrets) {
    // This boot's own view of each server's current epoch, so an ordinary renew is one request
    // rather than the full unlock sequence — kept here, never on disk: a restart's whole point is
    // that it forgets this along with every lease it names (§9's "a server restart drops every
    // lease").
    let mut epochs: HashMap<String, String> = HashMap::new();
    loop {
        tick(&secrets, &mut epochs).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(secrets: &Secrets, epochs: &mut HashMap<String, String>) {
    let Ok(connections) = Connections::load_from(secrets.config()) else {
        return;
    };
    for (petname, connection) in &connections.connections {
        if !connection.secrets.auto_unlock || connection.secrets.share.is_empty() {
            // Nothing (or nothing any more) to keep unlocked here — drop any epoch we were
            // tracking so a later `mesh share --auto-unlock` starts clean rather than renewing
            // against a stale one.
            epochs.remove(petname);
            continue;
        }
        if let Err(error) = ensure_unlocked(secrets, petname, &connection.secrets, epochs).await {
            debug!(node = %petname, %error, "lease worker: could not unlock or renew");
        }
    }
}

/// Renew `petname`'s lease if this worker already knows its current epoch and the server still
/// recognizes it; unlock from scratch otherwise — the first tick for a connection, or any tick
/// after the server answered "unknown epoch" (a restart) or "no live lease" (the lease ran out its
/// own grace and was evicted). Any other failure (the node unreachable, most often) is left for
/// the next tick, with the cached epoch kept: there is nothing wrong with it, only with reaching
/// the node right now.
async fn ensure_unlocked(
    secrets: &Secrets,
    petname: &str,
    settings: &SecretsSettings,
    epochs: &mut HashMap<String, String>,
) -> anyhow::Result<()> {
    let Some(auth) = viewer::HeldCredentials.authorization(petname) else {
        anyhow::bail!(
            "no password held for {petname} yet — unlock it once from the Fleet page \
             (the same credential dashboard transfer uses) so this worker can reach it unattended"
        );
    };
    if let Some(epoch) = epochs.get(petname).cloned() {
        let body = serde_json::to_vec(&serde_json::json!({ "epoch": epoch }))?;
        match node::post(petname, "/api/leases/renew", &auth, body, CONTROL_TIMEOUT).await {
            Ok(_) => return Ok(()),
            Err(error) if error.status == 404 || error.status == 409 => {
                // "no live lease" (evicted) or "unknown epoch" (a restart) — re-unlock below.
            }
            Err(error) => anyhow::bail!("{}", error.message),
        }
    }
    let epoch = unlock(secrets, petname, &auth, settings).await?;
    epochs.insert(petname.to_string(), epoch);
    Ok(())
}

/// This device's own half of `docs/byo-credentials.md` §9 steps 1-4, the unattended twin of
/// `adi-mono mesh unlock` (`adi-cli/src/mesh.rs`) — the same sequence, `auth` supplied by
/// [`ensure_unlocked`] instead of a password prompt. Returns the epoch unlocked against, for
/// [`ensure_unlocked`]'s own renew cache.
async fn unlock(secrets: &Secrets, petname: &str, auth: &str, settings: &SecretsSettings) -> anyhow::Result<String> {
    let info: serde_json::Value =
        serde_json::from_str(&node::get(petname, "/api/leases/session", auth, CONTROL_TIMEOUT).await.map_err(|e| anyhow::anyhow!("{}", e.message))?)?;
    let user = info["user"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{petname}: no user scope to unlock"))?;
    let epoch = info["epoch"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{petname}: malformed session answer"))?
        .to_string();
    let session_public = decode_b64_32(info["public_key"].as_str().unwrap_or_default())
        .ok_or_else(|| anyhow::anyhow!("{petname}: malformed session public key"))?;

    let mine: serde_json::Value =
        serde_json::from_str(&node::get(petname, "/api/secrets/mine", auth, CONTROL_TIMEOUT).await.map_err(|e| anyhow::anyhow!("{}", e.message))?)?;
    let wrap_key = adi_mesh::devicekey::load_or_create_in(secrets.config())?;
    let device = adi_mesh::identity::endpoint_id()?.to_string();

    let mut sealed = Vec::new();
    for entry in mine["names"].as_array().cloned().unwrap_or_default() {
        let Some(name) = entry["name"].as_str() else { continue };
        if !settings.share.iter().any(|n| n == name) {
            continue;
        }
        let Some(wrap) = entry["wraps"].as_array().and_then(|w| w.first()) else {
            continue; // nothing wrapped under this name yet.
        };
        let Ok(dek) = adi_secrets::wrap::unwrap_dek(
            &wrap_key,
            petname,
            name,
            wrap["nonce"].as_str().unwrap_or_default(),
            wrap["wrapped"].as_str().unwrap_or_default(),
        ) else {
            continue; // wrapped by a different device, or corrupt — skip rather than abort.
        };
        let Ok(plaintext) = adi_secrets::wrap::open_value(
            &dek,
            petname,
            name,
            entry["nonce"].as_str().unwrap_or_default(),
            entry["ciphertext"].as_str().unwrap_or_default(),
        ) else {
            continue;
        };
        let aad = format!("user:{user}/{name}|{epoch}|{device}");
        let Ok(seal) = adi_mesh::lease_seal::seal(&session_public, &aad, &plaintext) else {
            continue;
        };
        sealed.push(serde_json::json!({
            "name": name,
            "device_public": encode_b64(&seal.device_public),
            "nonce": seal.nonce,
            "ciphertext": seal.ciphertext,
        }));
    }
    anyhow::ensure!(
        !sealed.is_empty(),
        "{petname}: none of the shared names have a value stored there yet"
    );

    let body = serde_json::to_vec(&serde_json::json!({
        "epoch": epoch,
        "lease": settings.lease.to_string(),
        "sealed": sealed,
    }))?;
    node::post(petname, "/api/leases/unlock", auth, body, CONTROL_TIMEOUT)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    Ok(epoch)
}

fn decode_b64_32(text: &str) -> Option<[u8; 32]> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()
        .and_then(|b| b.try_into().ok())
}

fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-app-lease-worker-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    /// A connection with nothing to share, or `auto_unlock` off, is skipped before this worker
    /// ever asks for a credential — and an epoch left over from a previous tick is forgotten, so
    /// turning `auto_unlock` back on starts clean.
    #[tokio::test]
    async fn tick_skips_connections_with_nothing_to_share_and_forgets_their_cached_epoch() {
        let config = scratch("skip");
        let secrets = Secrets::with_config(config.clone());
        let mut connections = Connections::load_from(&config).expect("load");
        connections.edit_secrets("off", |s| {
            s.auto_unlock = false;
            s.share = vec!["K".to_string()];
        });
        connections.edit_secrets("empty", |s| {
            s.auto_unlock = true;
        });
        connections.save_to(&config).expect("save");

        let mut epochs = HashMap::from([("off".to_string(), "stale".to_string())]);
        tick(&secrets, &mut epochs).await;
        assert!(epochs.is_empty(), "nothing eligible leaves nothing cached");
    }

    /// No password held for a connection (this device has never unlocked a dashboard on it
    /// either) is a quiet skip, not a panic or a hang — proven directly against
    /// [`ensure_unlocked`], since a real tick would also need a reachable gateway.
    #[tokio::test]
    async fn ensure_unlocked_without_a_held_credential_fails_without_touching_the_network() {
        let config = scratch("no-credential");
        let secrets = Secrets::with_config(config);
        let settings = SecretsSettings { share: vec!["K".to_string()], ..SecretsSettings::default() };
        let mut epochs = HashMap::new();
        let err = ensure_unlocked(&secrets, "adi-mono-181-lease-worker-test-unpaired", &settings, &mut epochs)
            .await
            .expect_err("no credential, no network call");
        assert!(err.to_string().contains("no password held"), "{err}");
    }
}
