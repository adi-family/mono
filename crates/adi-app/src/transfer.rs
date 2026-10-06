//! Transfer a dashboard bundle through the paired node's control panel.
//!
//! The supplied password is used for this request only and is never stored.
//! For moves, stop the local copy only after a successful remote import (`200`);
//! upload failure must leave it running.

use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{
    Dashboard, DashboardTransferred, DashboardsState, TransferDashboard, TransferMode,
};

use crate::node;
use crate::scan;
use crate::viewer;

const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

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
                    &format!(
                        "{node} accepted the dashboard but answered something unreadable: {e}"
                    ),
                );
            }
        },
        Err(e) => return handlers::error(e.status, &e.message),
    };

    // Pairing grants only `http:app`. Adding the dashboard grant is best-effort:
    // the remote import already succeeded, even if its link cannot be opened yet.
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
        // The remote import succeeded; report the local cleanup failure.
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
