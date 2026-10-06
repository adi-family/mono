//! List and control paired nodes through their authenticated control panels.
//!
//! Service discovery uses `/api/dashboards`: the mesh protocol rejects unauthorized peers
//! before route lookup to prevent service enumeration. Pairing grants only `http:app`;
//! opening other services requires a grant via [`allow`].
//!
//! Credentials are encrypted by [`adi_secrets`] and never returned to the browser.

use std::collections::BTreeMap;

use adi_mesh::fleet::{FleetRegistry, Grant, Target};
use adi_projects::Projects;
use adi_secrets::Secrets;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{
    Dashboard, DashboardsState, FleetDashboards, FleetGrantRef, FleetNodeAccess, FleetNodes,
    FleetReach, FleetRef, FleetState, NodeDashboard, NodeDashboards, NodeReach, NodeServiceRef,
    Reach, UnlockNode,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::node::{self, CONTROL_TIMEOUT};

/// A non-UUID scope cannot collide with project IDs. Never use global secrets here:
/// `Secrets::resolve` would inject node passwords into agent environments.
const CREDENTIAL_SCOPE: &str = "fleet-nodes";

/// One JSON map keyed by petname avoids collisions when converting DNS labels to
/// secret names, which must be environment identifiers.
const CREDENTIAL_SECRET: &str = "NODE_CREDENTIALS";

const CREDENTIAL_NOTE: &str =
    "Passwords for the paired nodes whose dashboards this machine lists. Delete to re-lock them.";

/// Keep recurring reads shorter than deliberate writes so sleeping nodes do not stall the UI.
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Credential {
    /// Defaults to the pairing user.
    #[serde(default)]
    user: Option<String>,
    password: String,
}

impl Credential {
    fn auth(&self) -> String {
        node::basic_auth(self.user.as_deref(), &self.password)
    }
}

type Credentials = BTreeMap<String, Credential>;

/// List paired nodes, including locked and unavailable nodes.
pub(crate) async fn fleet_dashboards(secrets: &Secrets) -> Response {
    match listing(secrets).await {
        Ok(state) => handlers::ok_json(&state),
        Err(e) => handlers::error(500, &e),
    }
}

/// Read local pairing and credential state without contacting nodes.
pub(crate) fn nodes(secrets: &Secrets) -> Response {
    let registry = match FleetRegistry::load() {
        Ok(registry) => registry,
        Err(e) => return handlers::error(500, &format!("reading the fleet registry: {e}")),
    };
    let held = credentials(secrets);
    let nodes = registry
        .nodes
        .into_keys()
        .map(|node| FleetNodeAccess {
            locked: !held.contains_key(&node),
            node,
        })
        .collect();
    handlers::ok_json(&FleetNodes { nodes })
}

/// Allow time for an initial relay dial without holding every poll for a full listing timeout.
const REACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) async fn reach(secrets: &Secrets) -> Response {
    let registry = match FleetRegistry::load() {
        Ok(registry) => registry,
        Err(e) => return handlers::error(500, &format!("reading the fleet registry: {e}")),
    };
    let held = credentials(secrets);
    let mut dialling = tokio::task::JoinSet::new();
    for (node, record) in registry.nodes {
        let auth = held.get(&node).map(Credential::auth).unwrap_or_default();
        dialling.spawn(async move {
            let reach = node::reach(&node, &auth, REACH_TIMEOUT).await;
            (NodeReach { node, reach }, record.key)
        });
    }
    let dialled = dialling.join_all().await;
    // A refusal still proves the node was reached.
    let db = adi_db::Db::open();
    let now = crate::now_secs();
    for (n, key) in &dialled {
        if matches!(n.reach, Reach::Reachable | Reach::Refused) {
            adi_mesh::activity::record_reached(&db, key, now);
        }
    }
    let mut nodes: Vec<NodeReach> = dialled.into_iter().map(|(n, _)| n).collect();
    nodes.sort_by(|a, b| a.node.cmp(&b.node));
    handlers::ok_json(&FleetReach { nodes })
}

/// Verify the password against the node before storing it.
pub(crate) async fn unlock(secrets: &Secrets, body: &[u8]) -> Response {
    let req: UnlockNode = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let petname = req.node.trim().to_string();
    if petname.is_empty() || req.password.is_empty() {
        return handlers::error(400, "unlocking a node needs its name and its password");
    }
    if let Err(response) = node::require_paired(&petname) {
        return response;
    }

    let credential = Credential {
        user: req
            .username
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        password: req.password,
    };
    if let Err(e) = node::get(
        &petname,
        "/api/dashboards",
        &credential.auth(),
        CONTROL_TIMEOUT,
    )
    .await
    {
        return handlers::error(e.status, &e.message);
    }

    let mut held = credentials(secrets);
    held.insert(petname.clone(), credential);
    if let Err(e) = save(secrets, &held) {
        return handlers::error(500, &e);
    }
    info!(node = %petname, "viewer: this machine can now list the node's dashboards");
    fleet_dashboards(secrets).await
}

/// Forget the local password without changing the node's grants.
pub(crate) async fn forget(secrets: &Secrets, body: &[u8]) -> Response {
    let req: FleetRef = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let petname = req.petname.trim().to_string();
    if petname.is_empty() {
        return handlers::error(400, "expected JSON body { \"petname\": \"<node>\" }");
    }
    let mut held = credentials(secrets);
    held.remove(&petname);
    if let Err(e) = save(secrets, &held) {
        return handlers::error(500, &e);
    }
    fleet_dashboards(secrets).await
}

/// Drop the source and its stored password.
pub(crate) fn drop_source(projects: &Projects, secrets: &Secrets, body: &[u8]) -> Response {
    let response = handlers::fleet_drop_source(projects.config(), body);
    if response.status == 200
        && let Ok(node) = serde_json::from_slice::<FleetRef>(body)
    {
        forget_quietly(secrets, node.petname.trim());
    }
    response
}

/// Drop the viewer; retain its password if it is still a source.
pub(crate) fn drop_viewer(projects: &Projects, secrets: &Secrets, body: &[u8]) -> Response {
    let response = handlers::fleet_drop_viewer(projects.config(), body);
    if response.status == 200
        && let Ok(node) = serde_json::from_slice::<FleetRef>(body)
        && let Ok(state) = serde_json::from_str::<FleetState>(&response.body)
        && !state.nodes.iter().any(|n| n.petname == node.petname.trim())
    {
        forget_quietly(secrets, node.petname.trim());
    }
    response
}

/// Credential cleanup is best-effort because the fleet edit has already succeeded.
fn forget_quietly(secrets: &Secrets, petname: &str) {
    let mut held = credentials(secrets);
    if held.remove(petname).is_some()
        && let Err(e) = save(secrets, &held)
    {
        warn!(node = %petname, error = %e, "viewer: could not forget a dropped node's password");
    }
}

pub(crate) async fn allow(secrets: &Secrets, body: &[u8]) -> Response {
    let req: NodeServiceRef = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let (petname, service) = (
        req.node.trim().to_string(),
        req.service.trim().to_lowercase(),
    );
    if petname.is_empty() || service.is_empty() {
        return handlers::error(400, "a grant needs both a node and a service");
    }
    if let Err(response) = node::require_paired(&petname) {
        return response;
    }
    let Some(credential) = credentials(secrets).remove(&petname) else {
        return handlers::error(
            403,
            &format!("{petname} is locked here — give this machine its password first"),
        );
    };

    match grant_self(&petname, &credential.auth(), &service).await {
        Ok(_) => fleet_dashboards(secrets).await,
        Err(e) => handlers::error(e.status, &e.message),
    }
}

/// Stay under `/api` to inherit origin checks and shared-read collapsing.
pub(crate) const NODE_PREFIX: &str = "/api/node/";

/// Split a node API path, preserving its query string.
pub(crate) fn split_node_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix(NODE_PREFIX)?;
    let cut = after.find('/')?;
    let (node, rest) = after.split_at(cut);
    let route = rest.split('?').next().unwrap_or(rest);
    // WebSocket upgrades are unsupported. Reject nested forwarding so a third node's
    // data cannot appear under the addressed node's name.
    let reachable =
        route.starts_with("/api/") && route != "/api/ws" && !route.starts_with(NODE_PREFIX);
    (!node.is_empty() && reachable).then_some((node, rest))
}

/// Forward GET/POST to a paired node using its stored credential.
/// The node still enforces mesh grants and Basic auth. Writes remain JSON except
/// attachments, whose raw bytes, Content-Type, and X-Adi-Filename pass through.
pub(crate) async fn proxy(
    secrets: &Secrets,
    method: &str,
    node: &str,
    path: &str,
    content_type: Option<&str>,
    filename: Option<&str>,
    body: &[u8],
) -> Response {
    if let Err(response) = node::require_paired(node) {
        return response;
    }
    let Some(credential) = credentials(secrets).remove(node) else {
        return handlers::error(
            401,
            &format!(
                "{node} is locked here — give this machine its password on the Fleet page first"
            ),
        );
    };
    let auth = credential.auth();

    // Periodic reads use a shorter timeout than user-initiated writes.
    let answered = match method {
        "GET" => node::get(node, path, &auth, LIST_TIMEOUT).await,
        "POST" if path == "/api/agents/attachment" => {
            node::post_bytes(
                node,
                path,
                &auth,
                content_type.unwrap_or("application/octet-stream"),
                filename,
                body.to_vec(),
                CONTROL_TIMEOUT,
            )
            .await
        }
        "POST" => node::post(node, path, &auth, body.to_vec(), CONTROL_TIMEOUT).await,
        other => {
            return handlers::error(
                405,
                &format!("{other} is not forwarded to a node — only GET and POST are"),
            );
        }
    };
    match answered {
        Ok(body) => Response { status: 200, body },
        Err(e) => handlers::error(e.status, &e.message),
    }
}

/// Grant this machine `http:<service>` using the node's petname for our mesh key.
pub(crate) async fn grant_self(
    petname: &str,
    auth: &str,
    service: &str,
) -> Result<String, node::CallError> {
    let us = node::local_key().ok_or_else(|| node::CallError {
        status: 500,
        message: "this machine's own mesh identity could not be read".to_string(),
    })?;
    let fleet = node::get(petname, "/api/fleet", auth, CONTROL_TIMEOUT).await?;
    let (me, _) = find_me(&fleet, &us).ok_or_else(|| node::CallError {
        status: 409,
        message: format!(
            "{petname} does not have this machine's key on file, so it has no peer to grant — \
             pair again"
        ),
    })?;

    let grant = FleetGrantRef {
        petname: me.clone(),
        grant: format!("http:{service}"),
    };
    let payload = serde_json::to_vec(&grant).map_err(|e| node::CallError {
        status: 500,
        message: format!("building the grant: {e}"),
    })?;
    node::post(
        petname,
        "/api/fleet/grants/add",
        auth,
        payload,
        CONTROL_TIMEOUT,
    )
    .await?;
    info!(node = %petname, %service, "viewer: the node now lets this machine open a dashboard");
    Ok(me)
}

async fn node_dashboards(petname: String, credential: Option<Credential>) -> NodeDashboards {
    let locked = NodeDashboards {
        node: petname.clone(),
        locked: true,
        error: None,
        me: None,
        dashboards: Vec::new(),
    };
    let Some(credential) = credential else {
        return locked;
    };
    let auth = credential.auth();

    let listing = match node::get(&petname, "/api/dashboards", &auth, LIST_TIMEOUT).await {
        Ok(body) => body,
        Err(e) => {
            debug!(node = %petname, error = %e.message, "viewer: could not list the node");
            // Mark rejected credentials as locked so the UI offers to replace them.
            return NodeDashboards {
                locked: e.status == 401,
                error: Some(e.message),
                ..locked
            };
        }
    };
    let listing: DashboardsState = match serde_json::from_str(&listing) {
        Ok(listing) => listing,
        Err(e) => {
            return NodeDashboards {
                locked: false,
                error: Some(format!(
                    "{petname} answered something that is not a dashboard listing: {e}"
                )),
                ..locked
            };
        }
    };

    // Grant lookup failure must not discard the dashboard listing already fetched.
    let mine = match node::get(&petname, "/api/fleet", &auth, LIST_TIMEOUT).await {
        Ok(body) => node::local_key().and_then(|us| find_me(&body, &us)),
        Err(e) => {
            debug!(node = %petname, error = %e.message, "viewer: could not read the node's fleet");
            None
        }
    };
    let (me, grants) = match mine {
        Some((me, grants)) => (Some(me), grants),
        None => (None, Vec::new()),
    };

    NodeDashboards {
        node: petname.clone(),
        locked: false,
        error: None,
        me,
        dashboards: assemble(&petname, listing.dashboards, &grants),
    }
}

/// Archived dashboards have no supervised services to open.
fn assemble(petname: &str, dashboards: Vec<Dashboard>, grants: &[Grant]) -> Vec<NodeDashboard> {
    dashboards
        .into_iter()
        .filter(|d| !d.is_archived())
        .map(|d| {
            let service = node::service_name(d.host.as_deref());
            let allowed = service
                .as_deref()
                .is_some_and(|label| grants.iter().any(|g| g.allows(Target::Http(label))));
            NodeDashboard {
                url: node::mesh_url(petname, d.host.as_deref()),
                id: d.id,
                name: d.name,
                description: d.description,
                service,
                running: d.frontend_running,
                allowed,
                icon: d.icon,
            }
        })
        .collect()
}

/// Match by mesh key: each node chooses its own petname for this machine.
fn find_me(fleet: &str, us: &str) -> Option<(String, Vec<Grant>)> {
    let fleet: FleetState = serde_json::from_str(fleet).ok()?;
    let peer = fleet.nodes.into_iter().find(|peer| peer.key == us)?;
    let grants = peer
        .grants
        .iter()
        // Preserve known grants when a newer node returns an unfamiliar rule.
        .filter_map(|raw| raw.parse::<Grant>().ok())
        .collect();
    Some((peer.petname, grants))
}

/// Query concurrently, then sort answered, errored, and locked nodes by name.
async fn listing(secrets: &Secrets) -> Result<FleetDashboards, String> {
    let registry = FleetRegistry::load().map_err(|e| format!("reading the fleet registry: {e}"))?;
    let held = credentials(secrets);

    let mut asking = tokio::task::JoinSet::new();
    for petname in registry.nodes.into_keys() {
        let credential = held.get(&petname).cloned();
        asking.spawn(node_dashboards(petname, credential));
    }

    let mut nodes: Vec<NodeDashboards> = Vec::new();
    while let Some(joined) = asking.join_next().await {
        match joined {
            Ok(node) => nodes.push(node),
            Err(e) => warn!(error = %e, "viewer: a node listing task did not finish"),
        }
    }
    nodes.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.node.cmp(&b.node)));
    Ok(FleetDashboards { nodes })
}

fn rank(node: &NodeDashboards) -> u8 {
    match (node.locked, node.error.is_some()) {
        (true, _) => 2,
        (false, true) => 1,
        (false, false) => 0,
    }
}

/// Missing or unreadable credentials leave nodes locked so they can be unlocked again.
fn credentials(secrets: &Secrets) -> Credentials {
    let raw = match secrets.reveal(Some(CREDENTIAL_SCOPE), CREDENTIAL_SECRET) {
        Ok(Some(raw)) => raw,
        Ok(None) => return Credentials::new(),
        Err(e) => {
            warn!(error = %e, "viewer: the stored node credentials could not be read");
            return Credentials::new();
        }
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| {
        warn!(error = %e, "viewer: the stored node credentials are not readable JSON");
        Credentials::new()
    })
}

/// Supply held credentials to outgoing mesh-gateway requests.
/// Read the store on every call so locking a node takes effect on the next connection.
#[derive(Debug)]
pub(crate) struct HeldCredentials;

impl adi_mesh::gateway::NodeCredentials for HeldCredentials {
    fn authorization(&self, node: &str) -> Option<String> {
        credentials(&Secrets::open())
            .get(node)
            .map(Credential::auth)
    }
}

/// Remove an empty credential set so the Secrets page has no stale entry.
fn save(secrets: &Secrets, held: &Credentials) -> Result<(), String> {
    if held.is_empty() {
        return secrets
            .remove(Some(CREDENTIAL_SCOPE), CREDENTIAL_SECRET)
            .map(|_| ())
            .map_err(|e| format!("dropping the stored node credentials: {e}"));
    }
    let raw =
        serde_json::to_string(held).map_err(|e| format!("encoding the node credentials: {e}"))?;
    secrets
        .set(
            Some(CREDENTIAL_SCOPE),
            CREDENTIAL_SECRET,
            &raw,
            Some(CREDENTIAL_NOTE),
        )
        .map(|_| ())
        .map_err(|e| format!("storing the node credentials: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Secrets {
        let root = std::env::temp_dir().join(format!(
            "adi-app-viewer-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Secrets::with_config(adi_config::Config::with_root(root))
    }

    fn grants(raw: &[&str]) -> Vec<Grant> {
        raw.iter()
            .map(|g| g.parse().expect("a valid grant"))
            .collect()
    }

    fn dashboard(id: &str, host: Option<&str>) -> Dashboard {
        Dashboard {
            id: id.to_string(),
            dir: format!("/tmp/{id}"),
            name: id.to_uppercase(),
            description: None,
            project: None,
            host: host.map(ToString::to_string),
            frontend_port: Some(8010),
            backend_port: Some(8011),
            frontend_running: true,
            backend_running: true,
            modules: Vec::new(),
            routes: Vec::new(),
            archived_at: None,
            moved_to: None,
            never_started: false,
            icon: None,
        }
    }

    #[test]
    fn a_listing_becomes_rows_and_the_grants_decide_which_are_open() {
        let rows = assemble(
            "laptop-b",
            vec![
                dashboard("nosh", Some("nosh.adi")),
                dashboard("books", Some("books.adi")),
                dashboard("draft", None),
            ],
            &grants(&["http:app", "http:nosh"]),
        );

        assert_eq!(rows.len(), 3, "every live dashboard is listed");
        assert_eq!(rows[0].name, "NOSH");
        assert!(rows[0].allowed, "http:nosh covers it");
        assert_eq!(
            rows[0].url.as_deref(),
            Some("http://nosh.laptop-b.n.adi/"),
            "the address is built here — the node cannot know what we call it"
        );
        assert!(!rows[1].allowed, "no grant names `books`");
        assert!(
            rows[1].url.is_some(),
            "a row that needs a grant still knows where it would go"
        );
        assert_eq!(rows[2].service, None, "no host is no label");
        assert_eq!(rows[2].url, None, "and nothing to link to");
        assert!(!rows[2].allowed);
    }

    #[test]
    fn a_wildcard_grant_opens_every_dashboard() {
        let rows = assemble(
            "laptop-b",
            vec![
                dashboard("nosh", Some("nosh.adi")),
                dashboard("books", Some("books.adi")),
            ],
            &grants(&["http:*"]),
        );
        assert!(rows.iter().all(|row| row.allowed), "http:* covers the lot");
    }

    #[test]
    fn an_archived_dashboard_is_not_offered() {
        let archived = Dashboard {
            archived_at: Some(1),
            ..dashboard("old", Some("old.adi"))
        };
        let rows = assemble(
            "laptop-b",
            vec![archived, dashboard("nosh", Some("nosh.adi"))],
            &grants(&["http:*"]),
        );
        assert_eq!(
            rows.len(),
            1,
            "archiving stops the services over there; the row could only fail"
        );
        assert_eq!(rows[0].id, "nosh");
    }

    #[test]
    fn this_machine_finds_itself_in_a_nodes_fleet_page_by_key_alone() {
        let fleet = r#"{"nodes":[
            {"petname":"desk","key":"aaaa","grants":["http:*"],"nickname":"desk",
             "paired_at":1,"has_password":true},
            {"petname":"studio","key":"bbbb","grants":["http:app","tcp:127.0.0.1:22"],
             "nickname":"studio","paired_at":2,"has_password":true}
        ]}"#;

        let (me, grants) = find_me(fleet, "bbbb").expect("this machine is listed");
        assert_eq!(me, "studio", "the node's name for us, not ours for it");
        assert_eq!(grants.len(), 2, "every grant it holds, whatever kind");
        assert!(grants.iter().any(|g| g.allows(Target::Http("app"))));

        assert!(
            find_me(fleet, "cccc").is_none(),
            "an unpaired key is not there"
        );
        assert!(find_me("not json", "bbbb").is_none(), "nor is a bad page");
    }

    #[test]
    fn an_unparseable_grant_does_not_cost_the_peer_its_others() {
        let fleet = r#"{"nodes":[{"petname":"studio","key":"bbbb",
            "grants":["http:app","quantum:entangle"],"nickname":"s","paired_at":1,
            "has_password":true}]}"#;
        let (_, grants) = find_me(fleet, "bbbb").expect("listed");
        assert_eq!(
            grants.len(),
            1,
            "the rule we do not know is skipped, not fatal"
        );
        assert!(grants[0].allows(Target::Http("app")));
    }

    #[test]
    fn credentials_round_trip_and_the_last_one_out_removes_the_secret() {
        let secrets = store();
        assert!(
            credentials(&secrets).is_empty(),
            "nothing is held to begin with"
        );

        let mut held = Credentials::new();
        held.insert(
            "laptop-b".to_string(),
            Credential {
                user: None,
                password: "hunter2".to_string(),
            },
        );
        held.insert(
            "studio".to_string(),
            Credential {
                user: Some("igor".to_string()),
                password: "hunter3".to_string(),
            },
        );
        save(&secrets, &held).expect("saved");

        let read = credentials(&secrets);
        assert_eq!(read.len(), 2);
        assert_eq!(read["laptop-b"].password, "hunter2");
        assert_eq!(read["laptop-b"].auth(), "Basic YWRpOmh1bnRlcjI=");
        assert_ne!(
            read["studio"].auth(),
            node::basic_auth(None, "hunter3"),
            "a stored username is used, not the default"
        );

        save(&secrets, &Credentials::new()).expect("emptied");
        assert!(credentials(&secrets).is_empty());
        assert!(
            secrets
                .get(Some(CREDENTIAL_SCOPE), CREDENTIAL_SECRET)
                .expect("readable")
                .is_none(),
            "an empty store leaves no row claiming this machine keeps passwords"
        );
    }

    #[test]
    fn a_node_path_names_the_node_and_the_path_it_answers() {
        assert_eq!(
            split_node_path("/api/node/zomro-de1/api/agents"),
            Some(("zomro-de1", "/api/agents"))
        );
        assert_eq!(
            split_node_path("/api/node/laptop-b/api/agents/runs/all?limit=100"),
            Some(("laptop-b", "/api/agents/runs/all?limit=100"))
        );
    }

    #[test]
    fn nothing_but_a_nodes_api_is_reachable_through_it() {
        for path in [
            "/api/agents",                   // not addressed to a node at all
            "/api/node/",                    // no node
            "/api/node/laptop-b",            // no path
            "/api/node//api/agents",         // an empty node label
            "/api/node/laptop-b/index.html", // the node's web app, which this cannot carry
            "/api/node/laptop-b/api/ws",     // a socket upgrade, not a request
            "/api/node/laptop-b/apiagents",  // a prefix that is not the segment
            "/api/node/laptop-b/api/node/studio/api/agents",
        ] {
            assert_eq!(split_node_path(path), None, "{path} must not be forwarded");
        }
    }

    #[test]
    fn the_node_list_says_which_ones_this_machine_can_ask() {
        let secrets = store();
        let mut held = Credentials::new();
        held.insert(
            "laptop-b".to_string(),
            Credential {
                user: None,
                password: "hunter2".to_string(),
            },
        );
        save(&secrets, &held).expect("saved");

        let held = credentials(&secrets);
        assert!(!held.contains_key("studio"), "never asked, never stored");
        assert!(held.contains_key("laptop-b"));
    }

    #[test]
    fn stored_node_passwords_never_reach_a_runs_environment() {
        let secrets = store();
        let mut held = Credentials::new();
        held.insert(
            "laptop-b".to_string(),
            Credential {
                user: None,
                password: "hunter2".to_string(),
            },
        );
        save(&secrets, &held).expect("saved");

        for scope in [None, Some("some-project")] {
            let env = secrets.resolve(scope).expect("resolves");
            assert!(
                !env.values().any(|v| v.contains("hunter2")),
                "a node password reached the environment of a {scope:?} run: {env:?}"
            );
            assert!(!env.contains_key(CREDENTIAL_SECRET), "{env:?}");
        }
    }
}
