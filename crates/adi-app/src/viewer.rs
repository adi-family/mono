//! This machine as a **viewer of its own fleet**: what every paired node is running, listed here.
//!
//! `apps/ios` already does this — a phone holds a node's password in its Keychain and asks that
//! node's own control panel what it serves (`adi-mesh-ffi/src/viewer/catalog.rs`). A desktop panel
//! wanting the same rail is the same problem, so it is the same answer, and the reasoning that
//! module states applies here word for word:
//!
//! **The list comes from the panel, not from the protocol.** `adi/mesh/http/1` deliberately cannot
//! answer "what do you serve?" — a node refuses an unauthorized peer *before* consulting its route
//! table, precisely so nobody can enumerate a machine's services by watching `ServiceUnknown` and
//! `NotAuthorized` differ. So the list comes from somewhere that already knows who is asking: `app`
//! is a service like any other, it is what the default grant names (`docs/fleet.md` §8), it sits
//! behind the node's Basic-auth gate (§5), and it already publishes `GET /api/dashboards`.
//!
//! **Listing may also grant.** Pairing hands out `http:app` and nothing else, so a list on its own
//! would be a list of rows that all refuse to open. [`allow`] asks the node for `http:<label>`, and
//! that escalates nothing: `http:app` plus the password *is* the control panel, which can already
//! create dashboards, move ports and run tasks. The grant adds reach, not authority — the browser
//! gets the page on its own origin (§4) instead of driving it through the panel.
//!
//! ## The one thing that is new here: this machine keeps the password
//!
//! A transfer asks for a node's password per transfer and stores nothing (§8), which is right for
//! a button pressed once. A rail is not pressed once — it refreshes — so re-prompting would make it
//! unusable, and the credential is stored. That is the bargain the phone already makes with the
//! Keychain; here the store is [`adi_secrets`], encrypted at rest under a `0600` master key.
//!
//! It is filed under a reserved **scope**, never as a global secret, and that is load-bearing:
//! `Secrets::resolve` injects every *global* secret into every agent run's environment, and a
//! node's password has no business in a subprocess's env. A scope no project id can equal keeps it
//! out of every resolve while still being visible on the Secrets page, where an operator can delete
//! it. Nothing here ever puts a password on the wire back to the browser — [`FleetDashboards`] says
//! only whether a node is locked.

use std::collections::BTreeMap;

use adi_mesh::fleet::{FleetRegistry, Grant, Target};
use adi_secrets::Secrets;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{
    Dashboard, DashboardsState, FleetDashboards, FleetGrantRef, FleetState, FleetRef, NodeDashboard,
    NodeDashboards, NodeServiceRef, UnlockNode,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::node::{self, CONTROL_TIMEOUT};

/// The secrets scope the viewer's node credentials live in. A word, where every real project id is
/// a UUID, so nothing a project could ever be named collides with it — and being a scope rather
/// than the global set is what keeps these out of `Secrets::resolve` and therefore out of every
/// agent run's environment.
const CREDENTIAL_SCOPE: &str = "fleet-nodes";

/// The one secret in that scope: every node's credential, as a JSON object keyed by petname. One
/// secret and not one per node because a secret's name must be an env identifier and a petname is
/// a DNS label — `laptop-b` has no spelling there that `laptop_b` could not also claim.
const CREDENTIAL_SECRET: &str = "NODE_CREDENTIALS";

/// What the Secrets page says about that row, so it is not an unexplained blob.
const CREDENTIAL_NOTE: &str =
    "Passwords for the paired nodes whose dashboards this machine lists. Delete to re-lock them.";

/// How long one node may take to answer *the listing*, which is shorter than the bound a
/// deliberate click gets ([`CONTROL_TIMEOUT`]) and deliberately so.
///
/// The rail is asked on page load, and a node that has gone away does not refuse — it never
/// answers, so the whole wait is spent. A mesh round trip settles at 0.1–0.4 s (`docs/fleet.md`
/// §9), so ten seconds is many times the honest case and still short enough that a fleet with a
/// sleeping machine in it fills rather than hangs. Unlocking keeps the longer bound: that one is a
/// person waiting on something they asked for, where a spurious "did not answer" over a bad link
/// costs more than the wait.
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// One node's Basic-auth credential, as this machine keeps it for asking that node questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Credential {
    /// The username the node's gate expects. Defaults to the one pairing mints.
    #[serde(default)]
    user: Option<String>,
    password: String,
}

impl Credential {
    /// The `Authorization` header this credential makes.
    fn auth(&self) -> String {
        node::basic_auth(self.user.as_deref(), &self.password)
    }
}

/// Every node credential this machine holds, by petname.
type Credentials = BTreeMap<String, Credential>;

// ---------------------------------------------------------------------------------------
// The endpoints
// ---------------------------------------------------------------------------------------

/// `GET /api/fleet/dashboards` — what every paired node is running.
///
/// One entry per paired node whatever happened to it: locked (no password here), errored (with the
/// node's own refusal, phrased for a person), or listed. A node that is down must still be a row,
/// or a fleet would appear to shrink whenever a machine slept.
///
/// The nodes are asked concurrently. A mesh round trip is a third of a second before any payload
/// (`docs/fleet.md` §9), and asking six nodes one after another is the difference between a rail
/// that fills and a rail that hangs.
pub(crate) async fn fleet_dashboards(secrets: &Secrets) -> Response {
    match listing(secrets).await {
        Ok(state) => handlers::ok_json(&state),
        Err(e) => handlers::error(500, &e),
    }
}

/// `POST /api/fleet/dashboards/unlock` — store a node's password here, so its dashboards can be
/// listed without asking again.
///
/// The password is **checked against the node before it is written**: a credential that does not
/// work is worse than none, because the rail would then report an error instead of a lock and the
/// fix would read as the node's fault. A `401` comes back as a `401`.
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
        user: req.username.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        password: req.password,
    };
    // The cheapest authenticated call the panel has, and the very one the rail will make.
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

/// `POST /api/fleet/dashboards/forget` — drop a node's stored password. The node is not involved
/// and nothing it granted changes; this machine simply stops being able to ask it anything.
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
    // Forgetting what was never held is not an error — the caller asked for a state, and that
    // state is what it gets.
    held.remove(&petname);
    if let Err(e) = save(secrets, &held) {
        return handlers::error(500, &e);
    }
    fleet_dashboards(secrets).await
}

/// `POST /api/fleet/dashboards/allow` — ask a node to let this machine reach one of its services,
/// so a listed dashboard becomes a link that opens instead of one that refuses.
pub(crate) async fn allow(secrets: &Secrets, body: &[u8]) -> Response {
    let req: NodeServiceRef = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let (petname, service) = (req.node.trim().to_string(), req.service.trim().to_lowercase());
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

// ---------------------------------------------------------------------------------------
// Asking one node
// ---------------------------------------------------------------------------------------

/// Ask a node to grant this machine `http:<service>`, and return the petname the grant was filed
/// under.
///
/// The node files us under a petname of *its* choosing, which we have no way of knowing — so the
/// key is what identifies us (§2): read its fleet, find the record whose key is ours, and grant
/// against that name. Shared with [`crate::transfer`], which asks for exactly this on behalf of the
/// dashboard it has just sent.
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

/// One node's row: its dashboards and what this machine may open, or why neither could be learned.
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
            // A rejected password is a lock, not a fault: the fix is to give this machine the
            // node's current one, which is exactly what an unlocked-but-erroring row would hide.
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

    // A failure here is not fatal: the dashboards are already in hand, and the only thing the
    // fleet page adds is *whose* grants they are checked against.
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

/// Turn a node's own listing into the rows the rail shows.
///
/// Archived dashboards are dropped: archiving takes both of a dashboard's services out of the
/// supervisor's imports, so its host resolves to nothing over there and a row for it could only
/// ever fail.
fn assemble(petname: &str, dashboards: Vec<Dashboard>, grants: &[Grant]) -> Vec<NodeDashboard> {
    dashboards
        .into_iter()
        .filter(|d| !d.is_archived())
        .map(|d| {
            let service = node::host_label(d.host.as_deref());
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
            }
        })
        .collect()
}

/// This machine's petname and grants on a node, out of that node's own fleet page.
///
/// Matched **by key**, never by name: the node names this machine whatever it likes, and the key is
/// the only identity of record (§2).
fn find_me(fleet: &str, us: &str) -> Option<(String, Vec<Grant>)> {
    let fleet: FleetState = serde_json::from_str(fleet).ok()?;
    let peer = fleet.nodes.into_iter().find(|peer| peer.key == us)?;
    let grants = peer
        .grants
        .iter()
        // An unparseable grant is one rule this build does not know, not a reason to report the
        // peer as holding nothing.
        .filter_map(|raw| raw.parse::<Grant>().ok())
        .collect();
    Some((peer.petname, grants))
}

// ---------------------------------------------------------------------------------------
// The fan-out
// ---------------------------------------------------------------------------------------

/// Ask every paired node what it runs, concurrently, and answer in a stable, useful order.
///
/// **Never completion order** — a rail that reshuffled itself on every refresh according to which
/// node answered first would be unreadable. The order is what a reader wants first: the nodes that
/// answered, then the ones that refused, then the locked ones, alphabetical within each. That last
/// band matters more than it looks: a *viewer* (a phone) is a peer in this registry too, and it
/// hosts nothing and answers nothing (`adi-mesh-ffi/src/viewer.rs`) — so it can only ever sit
/// locked, and it sits at the bottom rather than above the machines that do serve something.
async fn listing(secrets: &Secrets) -> Result<FleetDashboards, String> {
    let registry =
        FleetRegistry::load().map_err(|e| format!("reading the fleet registry: {e}"))?;
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
            // Only reachable if a task panicked; the fleet is still worth answering without it.
            Err(e) => warn!(error = %e, "viewer: a node listing task did not finish"),
        }
    }
    nodes.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.node.cmp(&b.node)));
    Ok(FleetDashboards { nodes })
}

/// Which band a node sorts into: answered, refused, locked. See [`listing`].
fn rank(node: &NodeDashboards) -> u8 {
    match (node.locked, node.error.is_some()) {
        (true, _) => 2,
        (false, true) => 1,
        (false, false) => 0,
    }
}

// ---------------------------------------------------------------------------------------
// The credential store
// ---------------------------------------------------------------------------------------

/// Every node credential this machine holds. A missing, unreadable or malformed secret reads as
/// "none": the rail then shows every node as locked, which is both true and fixable, where an
/// error would leave a person with a broken page and nothing to press.
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

/// Write the credential set back, removing the secret entirely once it holds nothing — an empty
/// object left behind would be a row on the Secrets page that says a machine keeps passwords it
/// does not keep.
fn save(secrets: &Secrets, held: &Credentials) -> Result<(), String> {
    if held.is_empty() {
        return secrets
            .remove(Some(CREDENTIAL_SCOPE), CREDENTIAL_SECRET)
            .map(|_| ())
            .map_err(|e| format!("dropping the stored node credentials: {e}"));
    }
    let raw = serde_json::to_string(held)
        .map_err(|e| format!("encoding the node credentials: {e}"))?;
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
        }
    }

    #[test]
    fn a_listing_becomes_rows_and_the_grants_decide_which_are_open() {
        let rows = assemble(
            "laptop-b",
            vec![
                dashboard("nosh", Some("nosh.adi")),
                dashboard("books", Some("books.adi")),
                // Declares no host at all: listed, but nothing to open.
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

        assert!(find_me(fleet, "cccc").is_none(), "an unpaired key is not there");
        assert!(find_me("not json", "bbbb").is_none(), "nor is a bad page");
    }

    #[test]
    fn an_unparseable_grant_does_not_cost_the_peer_its_others() {
        let fleet = r#"{"nodes":[{"petname":"studio","key":"bbbb",
            "grants":["http:app","quantum:entangle"],"nickname":"s","paired_at":1,
            "has_password":true}]}"#;
        let (_, grants) = find_me(fleet, "bbbb").expect("listed");
        assert_eq!(grants.len(), 1, "the rule we do not know is skipped, not fatal");
        assert!(grants[0].allows(Target::Http("app")));
    }

    /// The credential survives a round trip, and the store is emptied rather than left holding an
    /// empty object.
    #[test]
    fn credentials_round_trip_and_the_last_one_out_removes_the_secret() {
        let secrets = store();
        assert!(credentials(&secrets).is_empty(), "nothing is held to begin with");

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
        // `adi:hunter2` — the default user pairing mints, since none was stored.
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

    /// The passwords must not be reachable through the path that fills a run's environment.
    /// A global secret would be — this is the whole reason for the reserved scope.
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
