//! What a node runs, asked of the node itself.
//!
//! The list of a machine's dashboards does **not** come off the wire protocol, and cannot: §5's
//! default-deny refuses an unauthorized peer *before* the route table is consulted, precisely so
//! nobody can enumerate a machine's services by watching `ServiceUnknown` and `NotAuthorized`
//! differ. So the question is put to the node's control panel, which is a service like any other,
//! is what pairing grants, sits behind the node's Basic gate, and already publishes
//! `GET /api/dashboards`. This is `docs/fleet.md` §11 — the desktop panel's fleet rail — asked
//! from a phone, and it needs no new wire format, no new grant kind and no version bump.
//!
//! Two of §11's rules are the whole reason this module is more than one request:
//!
//! * **Listing may also grant.** Pairing hands out `http:app` and nothing else, so a bare listing
//!   is a list of rows that would all refuse to open. What each row may reach is read from the
//!   node's own fleet page — by key, never by name — and a row that is not yet granted asks for
//!   `http:<service>` when it is opened rather than opening onto *not authorized*.
//! * **The grant is not usable the instant it is written.** The node's gateway serves from an
//!   in-memory snapshot of `fleet.toml` and re-reads it every five seconds, so [`allow`] pays that
//!   wait itself — the same reason pairing does ([`crate::invite::wait_until_admitted`]).
//!
//! What this browser may reach is still the node's opinion and only the node's: every field below
//! is read to be *shown*, and nothing here is ever consulted to decide that a request may proceed.

use serde::Deserialize;

use crate::bridge::PANEL_SERVICE;
use crate::http::{Body, Head, Request};
use crate::mesh::Mesh;
use crate::store::NodeRecord;

/// The zone a node's own services answer on locally, so `nosh.adi` is the dashboard the mesh
/// reaches as service `nosh`.
///
/// `adi_mesh::gateway::LOCAL_ZONE` said again, for the reason [`crate::mesh::HOME_RELAY`] is: that
/// constant lives in a crate this one cannot depend on, and the value is load-bearing rather than
/// a default — it is how a host in a listing becomes the name the node resolves off the wire.
const LOCAL_ZONE: &str = "adi";

/// The most of a panel's answer this client will hold in memory.
///
/// A listing is a few kilobytes; this is the cap that keeps a node which answers with something
/// else — a page, a log, a mistake — from making a phone allocate for it.
const MAX_BODY: usize = 512 * 1024;

/// One dashboard on a node, as a row on this screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// The name the mesh reaches it by: its local host with the zone taken off (`nosh.adi` →
    /// `nosh`). Also the scope of the `http:` grant that opens it.
    pub service: String,
    /// What its operator calls it.
    pub name: String,
    /// Whether the node says its frontend is up. Shown, not enforced — a dashboard that is down
    /// is still a row, because "it is off over there" is the answer somebody is looking for.
    pub running: bool,
    /// Whether this browser already holds a grant for it. `false` means opening it asks the node
    /// first ([`allow`]).
    pub granted: bool,
}

/// What a node answered when asked what it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Its dashboards, by name.
    pub boards: Vec<Board>,
    /// What the **node** calls this browser, out of its own fleet page — the petname a grant has
    /// to be asked for under. `None` when the node's fleet could not be read, which is why
    /// [`allow`] takes it as an argument rather than looking it up again.
    pub me: Option<String>,
}

/// Ask `record`'s node what it runs, and what this browser may reach there.
///
/// # Errors
/// The node's own sentence when the dial is refused, a 401 when the stored password no longer
/// works, or the status it answered with.
pub async fn list(mesh: &Mesh, record: &NodeRecord) -> Result<Listing, String> {
    let listing: DashboardsState = ask(mesh, record, Request::get("/api/dashboards")).await?;

    // Best-effort, and deliberately not fatal: the dashboards are already in hand, and the only
    // thing the fleet page adds is *whose* grants they are checked against. A node that answers
    // the listing but not this one yields rows that all offer to ask for their grant, which is a
    // worse first tap than it could be and still opens.
    let mine = ask::<FleetState>(mesh, record, Request::get("/api/fleet"))
        .await
        .ok()
        .and_then(|fleet| {
            let us = mesh.id().to_string();
            fleet.nodes.into_iter().find(|peer| peer.key == us)
        });
    let (me, grants) = match mine {
        Some(peer) => (Some(peer.petname), peer.grants),
        None => (None, Vec::new()),
    };

    let mut boards: Vec<Board> = listing
        .dashboards
        .into_iter()
        // Archiving takes both of a dashboard's services out of the node's supervisor, so its host
        // resolves to nothing over there and a row for it could only ever fail.
        .filter(|d| d.archived_at.is_none())
        .filter_map(|d| {
            let service = service_name(d.host.as_deref())?;
            Some(Board {
                granted: grants.iter().any(|grant| covers(grant, &service)),
                service,
                name: d.name,
                running: d.frontend_running,
            })
        })
        .collect();
    boards.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.service.cmp(&b.service)));
    Ok(Listing { boards, me })
}

/// Ask a node to let this browser reach one more of its services, and wait until it does.
///
/// `me` is what the node calls this browser — [`Listing::me`], read from the node's own fleet page
/// by key. It cannot be guessed from this side: §2 makes the petname the *namer's* to choose, so
/// the nickname this browser offered at pairing is a suggestion the node was free to ignore.
///
/// # Errors
/// If the node has never named this browser, if it refuses the grant, or if the service is still
/// not admitted once the node's own reload window has passed.
pub async fn allow(
    mesh: &Mesh,
    record: &NodeRecord,
    me: Option<&str>,
    service: &str,
) -> Result<(), String> {
    let me = me.ok_or(
        "this node has not said what it calls this browser, so there is nothing to ask under — \
         open its control panel and add the grant there",
    )?;
    let body = serde_json::json!({ "petname": me, "grant": format!("http:{service}") });
    let request = Request {
        method: "POST".into(),
        target: "/api/fleet/grants/add".into(),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
    };
    // The reply is the node's whole fleet page; nothing here reads it, and a 200 is the answer.
    let _: serde::de::IgnoredAny = ask(mesh, record, request).await?;
    crate::invite::wait_until_admitted(mesh, &record.addr()?, service).await
}

/// One request to a node's control panel, with its JSON answer parsed.
///
/// One stream per call, and the stream is dropped with the response: these are short questions
/// with an answer that ends, unlike the panel's own traffic ([`crate::bridge`]) where the stream
/// is the connection and outlives the request.
async fn ask<T: for<'de> Deserialize<'de>>(
    mesh: &Mesh,
    record: &NodeRecord,
    request: Request,
) -> Result<T, String> {
    let mut stream = mesh.open(&record.addr()?, PANEL_SERVICE).await?;
    let request = request.with_basic_auth(&record.username, &record.password);
    stream.write(&request.encode()).await?;

    let head = Head::parse(&stream.read_head().await?)?;
    let mut reader = Body::new(&head);
    let mut body = Vec::new();
    while !reader.is_done() {
        let chunk = reader.next(stream.reader()).await?;
        if chunk.is_empty() {
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(format!(
                "{} answered with more than {MAX_BODY} bytes",
                record.petname
            ));
        }
    }

    if head.status == 401 {
        // Distinguished from every other refusal because it has its own fix, and it is not "try
        // again": the node minted this password at pairing and no longer accepts it.
        return Err(format!(
            "{} no longer accepts this browser's password — pair with it again",
            record.petname
        ));
    }
    if head.status != 200 {
        return Err(format!(
            "{} answered {} {}",
            record.petname,
            head.status,
            head.reason.trim()
        ));
    }
    serde_json::from_slice(&body)
        .map_err(|e| format!("{} answered something unreadable: {e}", record.petname))
}

/// A node's local hostname with its zone taken off — `nosh.adi` → `nosh`, `app.nosh.adi` →
/// `app.nosh`. That name is both what the mesh dials and what a grant scopes.
///
/// Everything left of `.adi` is kept, because a node's own hosts are not all one label: a project
/// at `app.nosh.adi` sits beside the `nosh.adi` it belongs to, and truncating to the first label
/// would name a *different* service, or none. A host outside the local zone — a dashboard
/// published under a real domain — yields nothing rather than a guess: it answers where it is
/// published and no mesh name reaches it, so there is nothing here to open.
fn service_name(host: Option<&str>) -> Option<String> {
    let host = host?.trim().trim_end_matches('.').to_ascii_lowercase();
    let name = host.strip_suffix(&format!(".{LOCAL_ZONE}"))?;
    crate::protocol::is_service_name(name).then(|| name.to_string())
}

/// Does one of the node's grants, in the string form it stores, cover `service`?
///
/// `adi_mesh::fleet::Grant::allows` for the `http:` family, restated for the same reason the rest
/// of this file restates things: `Grant` lives beside a registry and a `Config` that do not exist
/// in a browser. An unparseable or unknown grant covers nothing, which is the same fail-closed
/// answer the node itself would give.
fn covers(grant: &str, service: &str) -> bool {
    match grant.trim().split_once(':') {
        Some(("http", scope)) => scope == "*" || scope == service,
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// The panel's own payloads, as much of them as a row needs
// ---------------------------------------------------------------------------------------

/// `GET /api/dashboards` — `adi_webapp_api::types::DashboardsState`, read rather than shared: that
/// crate reaches the store and the filesystem, and every field this client does not name is one
/// more thing a node running a newer panel could break by changing.
#[derive(Debug, Deserialize)]
struct DashboardsState {
    dashboards: Vec<Dashboard>,
}

#[derive(Debug, Deserialize)]
struct Dashboard {
    name: String,
    /// The hostname its services declare (`nosh.adi`). Absent when the dashboard's hive file names
    /// none — it is then running with no routable name, and only the node's own loopback reaches
    /// it.
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    frontend_running: bool,
    #[serde(default)]
    archived_at: Option<u64>,
}

/// `GET /api/fleet` — enough of `adi_webapp_api::types::FleetState` to find this browser in it.
#[derive(Debug, Deserialize)]
struct FleetState {
    nodes: Vec<FleetPeer>,
}

#[derive(Debug, Deserialize)]
struct FleetPeer {
    /// The peer's `EndpointId`. Matched against this tab's own key, because the key is the only
    /// identity of record — the petname beside it is the node's name for us and may be anything.
    key: String,
    petname: String,
    #[serde(default)]
    grants: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{covers, service_name};

    #[test]
    fn a_host_becomes_the_name_the_mesh_dials() {
        assert_eq!(service_name(Some("nosh.adi")).as_deref(), Some("nosh"));
        // A deep name is kept whole: `app.nosh` is a different service from `nosh`.
        assert_eq!(
            service_name(Some("app.nosh.adi")).as_deref(),
            Some("app.nosh")
        );
        assert_eq!(service_name(Some("NOSH.ADI.")).as_deref(), Some("nosh"));

        // Published somewhere real, or named nothing at all: no mesh name reaches either.
        assert_eq!(service_name(Some("nosh.example.com")), None);
        assert_eq!(service_name(Some("adi")), None);
        assert_eq!(service_name(None), None);
    }

    #[test]
    fn a_grant_covers_its_own_scope_and_nothing_else() {
        assert!(covers("http:nosh", "nosh"));
        assert!(covers("http:*", "nosh"));
        assert!(covers(" http:app.nosh ", "app.nosh"));

        assert!(!covers("http:nosh", "app.nosh"));
        assert!(!covers("http:app.nosh", "nosh"));
        // Neither family is enforced on the node, so neither may be read as reach here.
        assert!(!covers("tcp:127.0.0.1:22", "nosh"));
        assert!(!covers("ctl:*", "nosh"));
        assert!(!covers("nosh", "nosh"));
    }
}
