//! Loopback ports that survive a relaunch.
//!
//! Each `(node, service)` is one browser origin, and an origin is `scheme://host:port` — so the
//! port is part of the identity of the page, not an implementation detail. Cookies, `localStorage`,
//! service workers and saved credentials are all keyed to it. Handing out an ephemeral port would
//! mean every launch of the app presented the node's control panel as a site the browser had never
//! seen, silently discarding whatever it had stored.
//!
//! So the port is recorded the first time it is bound and re-used ever after. Re-use is *best
//! effort*: another process can hold the port by the time we come back, and a service that refuses
//! to open because a port moved would be a worse failure than an origin that reset. A failed
//! re-bind falls back to a fresh port and records that instead.

use std::collections::BTreeMap;

use anyhow::Context as _;
use tokio::net::TcpListener;
use tracing::{debug, warn};

/// Where the map lives inside the mesh module's directory.
const PORTS_FILE: &str = "ios-ports.json";

/// `"<node>/<service>"` → port. A flat string key so the file stays readable, and `BTreeMap` so it
/// stays in a stable order across writes.
type PortMap = BTreeMap<String, u16>;

/// Bind the loopback port recorded for `(node, service)`, or a fresh one, recording what we got.
///
/// # Errors
/// Only if no loopback port can be bound at all.
pub async fn bind_stable(node: &str, service: &str) -> anyhow::Result<TcpListener> {
    let key = format!("{node}/{service}");
    let mut map = load();

    if let Some(&port) = map.get(&key) {
        match TcpListener::bind(super::loopback(port)).await {
            Ok(listener) => {
                debug!(%node, %service, port, "re-bound the recorded port");
                return Ok(listener);
            }
            // Taken, or otherwise unusable this run. Fall through to a fresh one.
            Err(e) => warn!(%node, %service, port, error = %e, "recorded port unavailable; taking a new one"),
        }
    }

    let listener = TcpListener::bind(super::loopback(0))
        .await
        .context("binding a loopback port")?;
    let port = listener.local_addr()?.port();
    map.insert(key, port);
    save(&map);
    Ok(listener)
}

/// Read the map, treating any problem as "no record yet".
///
/// A corrupt or unreadable file must not stop a node from opening — the cost of losing it is one
/// reset origin, and the next successful bind rewrites it.
fn load() -> PortMap {
    let Ok(Some(bytes)) = adi_config::Config::open()
        .module(adi_mesh::config::MODULE)
        .read_raw(PORTS_FILE)
    else {
        return PortMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        warn!(error = %e, "the recorded port map is unreadable; starting a new one");
        PortMap::new()
    })
}

/// Persist the map, best effort for the same reason [`load`] is forgiving.
fn save(map: &PortMap) {
    let Ok(json) = serde_json::to_vec_pretty(map) else {
        return;
    };
    if let Err(e) = adi_config::Config::open()
        .module(adi_mesh::config::MODULE)
        .write_raw(PORTS_FILE, &json)
    {
        warn!(error = %e, "could not record the port map; ports may move on the next launch");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test rather than three, because each would have to point `$HOME` at its own store and
    /// the environment is process-wide: run in parallel, they would race for it and the failure
    /// would look like a port bug rather than a test bug.
    #[tokio::test]
    async fn a_recorded_port_comes_back_and_a_taken_one_gives_way() {
        let home = std::env::temp_dir().join(format!("adi-ios-ports-{}", std::process::id()));
        // SAFETY: set once, before the first store access, on the only test in this crate that
        // reads `$HOME`.
        unsafe { std::env::set_var("HOME", &home) };

        // The property the module exists for: a relaunch presents the same origin.
        let first = bind_stable("laptop-b", "app").await.expect("first bind");
        let port = first.local_addr().unwrap().port();
        drop(first);
        let again = bind_stable("laptop-b", "app").await.expect("second bind");
        assert_eq!(
            again.local_addr().unwrap().port(),
            port,
            "a relaunch must present the same origin"
        );

        // A different service on the same node is a different origin, so a different port.
        let other = bind_stable("laptop-b", "nosh").await.expect("other bind");
        assert_ne!(other.local_addr().unwrap().port(), port);

        // `again` is still bound, so the recorded port is unavailable — which must cost an origin,
        // not the service.
        let fallback = bind_stable("laptop-b", "app").await.expect("fallback bind");
        assert_ne!(
            fallback.local_addr().unwrap().port(),
            port,
            "the fallback must not collide with the live listener"
        );

        let _ = std::fs::remove_dir_all(&home);
    }
}
