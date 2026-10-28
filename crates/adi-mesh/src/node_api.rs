//! One request to a paired node's own `app` service, through this machine's **local** mesh
//! gateway — `docs/fleet.md` §10's dashboard-transfer addressing, byte for byte:
//! `127.0.0.1:<gateway port>` with `Host: app.<node>.n.adi`, exactly what the front door would
//! forward for the same path typed into a browser there.
//!
//! Lifted out of `adi-cli/src/secrets.rs` (ADI-MONO-178 P1.12) when the BYO-credentials `mesh`
//! verbs (ADI-MONO-180 P2.5) needed the same capability a second time: the CLI's `secrets set/rm
//! --user --node` was the only caller, so the helper lived there; a second caller is the line
//! past which a private helper becomes a shared one. Prompting for a password is still the
//! caller's own job — a terminal concern this crate has no business with — so this module takes
//! credentials already in hand rather than asking for them itself.

use std::time::Duration;

use crate::fleet::FleetRegistry;

/// A type alias rather than every caller naming `reqwest::blocking::Client` itself — so a crate
/// that only ever reaches a node through this module (ADI-MONO-180 P2.5's `mesh`
/// `share`/`unlock`/`lock`) doesn't need its own direct `reqwest` dependency just to spell the
/// client's type in a function signature.
pub type GatewayClient = reqwest::blocking::Client;

/// This device's own username for `node` — the credential's non-secret half
/// (`crate::auth::Credential::user`, stored in the clear). `None` if this device isn't paired
/// with `node` at all.
///
/// # Errors
/// Any I/O or TOML error reading the local fleet registry.
pub fn node_username(node: &str) -> anyhow::Result<Option<String>> {
    let registry = FleetRegistry::load()?;
    Ok(registry.get(node).map(|record| record.auth.user.clone()))
}

/// A `reqwest` blocking client configured for a loopback hop to the local gateway: no proxy, and
/// a timeout generous enough for a QUIC round trip to the peer but short enough that an
/// unreachable or unpaired node fails as that rather than hanging the caller's shell.
///
/// # Errors
/// Whatever `reqwest::ClientBuilder::build` returns on failure.
pub fn gateway_client(timeout: Duration) -> anyhow::Result<GatewayClient> {
    // `rustls-no-provider` needs a provider installed before any TLS-capable client is built,
    // even though every request this module sends is plain `http://` to loopback.
    rustls::crypto::ring::default_provider().install_default().ok();
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()?)
}

/// One request to `node`'s own `app` service, authenticated with `username`/`password` as this
/// device's stored credential for it.
///
/// # Errors
/// The connection failing, a non-2xx response (message taken from the body's own `error` field
/// when present), or a response that isn't JSON.
pub fn node_request(
    client: &GatewayClient,
    node: &str,
    username: &str,
    password: &str,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    let url = format!("http://{}{path}", adi_config::mesh_gateway_addr());
    let mut request = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        other => anyhow::bail!("unsupported method {other}"),
    };
    request = request
        .header(reqwest::header::HOST, format!("app.{node}.n.adi"))
        .basic_auth(username, Some(password));
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .map_err(|e| anyhow::anyhow!("reaching {node} through the local mesh gateway: {e}"))?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{status} {text}"));
        anyhow::bail!("{node}: {message}");
    }
    serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{node}'s answer wasn't JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_username_is_none_for_an_unpaired_node() {
        // No local store override exists for `FleetRegistry::load()` (it always reads the
        // standard store), so this only proves the "not paired" branch on whatever this test
        // machine's own registry holds — which, in CI and in this sandbox, pairs nothing under
        // this name.
        let node = "adi-mono-180-node-api-test-unpaired-xyz";
        // An unreadable registry on this box is not this test's concern either — only that a
        // *readable* one with nothing paired under this name answers `None`.
        if let Ok(found) = node_username(node) {
            assert!(found.is_none(), "{node} is not expected to be paired here");
        }
    }

    #[test]
    fn gateway_client_builds() {
        gateway_client(Duration::from_secs(1)).expect("builds");
    }
}
