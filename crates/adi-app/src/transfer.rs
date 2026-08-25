//! `POST /api/dashboards/transfer` — take a dashboard that runs here and make it run on a node.
//!
//! A dashboard is a directory (`docs/fleet.md` §4), and every machine in the fleet already knows
//! how to turn one into a running pair of bun servers. So "run this in the cloud" needs no new
//! deployment machinery at all: pack the directory, hand it to the node's own control panel, and
//! let the node's supervisor do there exactly what ours does here.
//!
//! The call out to the node is [`crate::node`]'s — through this machine's mesh gateway, addressed
//! by `Host`, with the credential in an `Authorization` header. Two things about this particular
//! caller are worth stating.
//!
//! **The password is asked for and never kept.** This machine holds a *verifier* for each node's
//! credential, not the credential (`docs/fleet.md` §8), and a deploy button is not a reason to
//! start keeping one. It rides in the request body, becomes an `Authorization` header, and is
//! dropped with the request. (The rail that *lists* a node's dashboards does keep one, deliberately
//! and per node — see [`crate::viewer`] — but that is a viewer holding its own credential, not a
//! transfer inventing a reason to.)
//!
//! **The local copy is stood down last, and only on a `200`.** [`handlers::complete_move`] runs
//! after the node has confirmed it holds the files — never in parallel, never optimistically. A
//! move whose upload failed leaves this machine exactly as it was.

use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{
    Dashboard, DashboardTransferred, DashboardsState, TransferDashboard, TransferMode,
};

use crate::node;
use crate::scan;
use crate::viewer;

/// How long the upload may take. Generous next to the control-plane calls: it carries the whole
/// dashboard, and a relayed mesh round trip is a third of a second before any payload
/// (`docs/fleet.md` §9).
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Send a dashboard to a paired node, and in [`TransferMode::Move`] stand the local copy down.
pub(crate) async fn transfer_dashboard(
    projects: &Projects,
    ports: &Ports,
    body: &[u8],
) -> Response {
    let req: TransferDashboard = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let (id, node) = (req.id.trim().to_string(), req.node.trim().to_string());
    if id.is_empty() || node.is_empty() {
        return handlers::error(400, "a transfer needs both a dashboard and a node");
    }
    if req.password.is_empty() {
        return handlers::error(
            400,
            &format!(
                "{node} asks for its password before it will accept anything — it was printed \
                 once, on the node, when it joined this fleet"
            ),
        );
    }
    if let Err(response) = node::require_paired(&node) {
        return response;
    }

    // Pack before dialling: an unknown id or an oversized directory is a local answer, and there
    // is no reason to have opened a connection to learn it.
    let bundle = match handlers::export_bundle(projects.config(), &id) {
        Ok(bundle) => bundle,
        Err(response) => return response,
    };
    let payload = match serde_json::to_vec(&bundle) {
        Ok(payload) => payload,
        Err(e) => return handlers::error(500, &format!("packing the dashboard: {e}")),
    };

    let auth = node::basic_auth(req.username.as_deref(), &req.password);
    let sent = node::post(
        &node,
        "/api/dashboards/import",
        &auth,
        payload,
        UPLOAD_TIMEOUT,
    )
    .await;
    let remote: Dashboard = match sent {
        Ok(body) => match serde_json::from_str(&body) {
            Ok(remote) => remote,
            Err(e) => {
                return handlers::error(
                    502,
                    &format!("{node} accepted the dashboard but answered something unreadable: {e}"),
                );
            }
        },
        Err(e) => return handlers::error(e.status, &e.message),
    };

    // Reaching it from here needs a grant on the node, and pairing hands out only `http:app`
    // (`docs/fleet.md` §8). Best-effort by design: the dashboard is already running over there,
    // so a grant we could not add is a link that 502s, not a transfer that failed.
    let granted = match node::service_name(remote.host.as_deref()) {
        Some(name) => viewer::grant_self(&node, &auth, &name).await.is_ok(),
        None => false,
    };

    let local = match req.mode {
        TransferMode::Copy => {
            handlers::dashboards(projects.config(), ports, &scan::listening_ports())
        }
        TransferMode::Move => handlers::complete_move(
            projects.config(),
            ports,
            &scan::listening_ports(),
            &id,
            &node,
            req.delete_local,
        ),
    };
    if local.status != 200 {
        // The node has it; only the local half went wrong. Say so with the node's status intact,
        // rather than reporting a failure for a transfer that did happen.
        return local;
    }
    let Ok(dashboards) = serde_json::from_str::<DashboardsState>(&local.body) else {
        return handlers::error(500, "could not read back the local dashboards listing");
    };

    handlers::ok_json(&DashboardTransferred {
        url: node::mesh_url(&node, remote.host.as_deref()),
        node,
        dashboard: remote,
        granted,
        dashboards,
    })
}
