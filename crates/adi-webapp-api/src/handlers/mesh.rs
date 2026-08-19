use adi_mesh::config::Forward;
use adi_mesh::config::MeshConfig;
use adi_mesh::identity;
use adi_mesh::ticket;

use crate::types::{
    MeshForward, MeshForwardRef, MeshListenRef, MeshPeerRef, MeshPortRef, MeshState,
};

use super::response::{FromBody, Response, error, ok_json, require};

/// `GET /api/mesh` — this machine's mesh identity, published ticket, and config. `running`
/// is the host's authoritative view of whether the in-process daemon is up (the host owns
/// the daemon's lifecycle, so it passes this in — the same way `health` takes its identity).
#[must_use]
pub fn mesh(running: bool) -> Response {
    match mesh_snapshot(running) {
        Ok(state) => ok_json(&state),
        Err(e) => error(500, &e),
    }
}

/// `POST /api/mesh/allow` — expose a local TCP port to peers.
#[must_use]
pub fn mesh_allow(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshPortRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    mesh_edit(running, |cfg| {
        cfg.allow_port(req.port);
    })
}

/// `POST /api/mesh/deny` — stop exposing a local TCP port.
#[must_use]
pub fn mesh_deny(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshPortRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    mesh_edit(running, |cfg| {
        cfg.deny_port(req.port);
    })
}

/// `POST /api/mesh/peers/allow` — authorize a peer (ticket or id) for the exposed ports;
/// the canonical id is what gets stored.
#[must_use]
pub fn mesh_allow_peer(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshPeerRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match ticket::target_id(&req.peer) {
        Ok(id) => id.to_string(),
        Err(e) => return error(400, &format!("invalid peer: {e}")),
    };
    mesh_edit(running, move |cfg| {
        cfg.allow_peer(id);
    })
}

/// `POST /api/mesh/peers/deny` — revoke a peer's authorization.
#[must_use]
pub fn mesh_deny_peer(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshPeerRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    mesh_edit(running, move |cfg| {
        cfg.deny_peer(&req.peer);
    })
}

/// `POST /api/mesh/forwards/add` — forward a local port to a peer's port.
#[must_use]
pub fn mesh_add_forward(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshForwardRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match ticket::target_id(&req.peer) {
        Ok(id) => id,
        Err(e) => return error(400, &format!("invalid peer: {e}")),
    };
    let name = req
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| default_forward_name(&id.to_string(), req.port));
    let forward = Forward {
        name,
        listen: req.listen,
        peer: req.peer,
        port: req.port,
    };
    mesh_edit(running, move |cfg| {
        cfg.add_forward(forward);
    })
}

/// `POST /api/mesh/forwards/remove` — remove the forward bound to a local port.
#[must_use]
pub fn mesh_remove_forward(running: bool, body: &[u8]) -> Response {
    let req = match require::<MeshListenRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    mesh_edit(running, move |cfg| {
        cfg.remove_forward(req.listen);
    })
}

/// Build the current mesh state: identity, the daemon's published ticket, config, and the
/// host-supplied `running` flag.
fn mesh_snapshot(running: bool) -> Result<MeshState, String> {
    let id = identity::endpoint_id()
        .map_err(|e| format!("reading mesh identity: {e}"))?
        .to_string();
    let cfg = MeshConfig::load().map_err(|e| format!("reading mesh config: {e}"))?;
    Ok(MeshState {
        id,
        running,
        ticket: ticket::published(),
        allow: cfg.host.allow,
        authorized_peers: cfg.host.authorized_peers,
        forwards: cfg
            .forwards
            .into_iter()
            .map(|f| MeshForward {
                name: f.name,
                listen: f.listen,
                peer: f.peer,
                port: f.port,
            })
            .collect(),
    })
}

/// Load the config, apply `edit`, save it, and return the fresh [`MeshState`] so the
/// client updates from one round-trip.
fn mesh_edit(running: bool, edit: impl FnOnce(&mut MeshConfig)) -> Response {
    let mut cfg = match MeshConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => return error(500, &format!("reading mesh config: {e}")),
    };
    edit(&mut cfg);
    if let Err(e) = cfg.save() {
        return error(500, &format!("saving mesh config: {e}"));
    }
    mesh(running)
}

impl FromBody for MeshPortRef {
    const EXPECTED: &'static str = "expected JSON body { \"port\": <1-65535> }";

    fn is_complete(&self) -> bool {
        self.port != 0
    }
}

impl FromBody for MeshPeerRef {
    const EXPECTED: &'static str = "expected JSON body { \"peer\": \"<id-or-ticket>\" }";

    fn is_complete(&self) -> bool {
        !self.peer.trim().is_empty()
    }
}

impl FromBody for MeshForwardRef {
    const EXPECTED: &'static str = "expected JSON body { listen, peer, port, name? }";

    fn is_complete(&self) -> bool {
        self.listen != 0 && self.port != 0 && !self.peer.trim().is_empty()
    }
}

impl FromBody for MeshListenRef {
    const EXPECTED: &'static str = "expected JSON body { \"listen\": <port> }";

    fn is_complete(&self) -> bool {
        self.listen != 0
    }
}

/// A short forward label: the peer id's prefix and the remote port.
fn default_forward_name(peer_id: &str, port: u16) -> String {
    let prefix: String = peer_id.chars().take(8).collect();
    format!("{prefix}:{port}")
}
