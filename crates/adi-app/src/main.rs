//! Control-panel webapp and JSON API behind `app.adi`.
//!
//! The Trunk build in `adi-webapp/dist` is embedded at compile time. Set `ADI_WEBAPP_DIST`
//! to serve a build from disk. API handlers and shared DTOs live in `adi_webapp_api`.

mod awaits;
mod channels;
mod event_log;
mod live;
mod node;
mod prober;
mod projects;
mod scan;
mod shared_assets;
mod transfer;
mod viewer;
mod ws;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use adi_agents::Agents;
use adi_db::Db;
use adi_events::Events;
use adi_knowledge::KnowledgeStore;
use adi_mesh::{Daemon, join};
use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_secrets::Secrets;
use adi_tasks::Tasks;
use adi_tools::Tools;
use adi_triggers::{EventDispatcher, Supervisor, Triggers};
use adi_usage::Usage;
use adi_webapp_api::handlers::Response;
use adi_webapp_api::{handlers, http, origin};
use include_dir::{Dir, include_dir};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, info, warn};

/// Shared stores and runtime state for all connections.
struct App {
    ports: Ports,
    projects: Projects,
    secrets: Secrets,
    db: Db,
    tasks: Tasks,
    tools: Tools,
    agents: Agents,
    /// Keep the lazily loaded embedding model alive across requests.
    knowledge: KnowledgeStore,
    triggers: Triggers,
    trigger_supervisor: Arc<Supervisor>,
    events: Events,
    workflows: adi_workflows::Engine,
    mesh: MeshCtl,
    /// Overrides the embedded webapp when [`DIST_ENV`] is set.
    dist: Option<PathBuf>,
    start: Instant,
    reads: Reads,
    live: live::Hub,
}

/// Owns the in-process mesh daemon; the mutex serializes start, stop, and join.
#[derive(Debug, Default)]
struct MeshCtl {
    daemon: Mutex<Option<Daemon>>,
}

impl MeshCtl {
    async fn running(&self) -> bool {
        self.daemon.lock().await.is_some()
    }

    async fn start(&self) -> anyhow::Result<()> {
        let mut slot = self.daemon.lock().await;
        if slot.is_none() {
            // Reuse stored node passwords for links opened through the mesh gateway.
            *slot = Some(Daemon::start_with(Some(Arc::new(viewer::HeldCredentials))).await?);
        }
        Ok(())
    }

    async fn stop(&self) {
        if let Some(daemon) = self.daemon.lock().await.take() {
            daemon.stop().await;
        }
    }

    /// Reuse the running endpoint: a second endpoint with the same identity races for its relay.
    /// Hold the lock across the handshake so a concurrent start cannot bind another endpoint.
    ///
    /// `grant_back` is ADI-MONO-171 G4's one-way-join choice — see
    /// `adi_mesh::join::record_viewer`.
    async fn join(&self, token: &str, grant_back: bool) -> anyhow::Result<join::Joined> {
        match self.daemon.lock().await.as_ref() {
            Some(daemon) => daemon.join(token, grant_back).await,
            None => join::join_with(token, grant_back).await,
        }
    }
}

/// Coalesce identical in-flight reads. Only [`shared_read_key`] routes participate;
/// mutations must never share a response.
#[derive(Debug, Default)]
struct Reads {
    inflight: std::sync::Mutex<HashMap<String, broadcast::Sender<Arc<Response>>>>,
}

impl Reads {
    async fn shared<F>(&self, key: String, compute: F) -> Arc<Response>
    where
        F: FnOnce() -> Response + Send + 'static,
    {
        // Release the map lock before computing or waiting for the response.
        let joined = {
            let mut inflight = self.inflight();
            match inflight.entry(key.clone()) {
                Entry::Occupied(leader) => Some(leader.get().subscribe()),
                Entry::Vacant(slot) => {
                    slot.insert(broadcast::channel(1).0);
                    None
                }
            }
        };

        if let Some(mut answer) = joined {
            return answer.recv().await.unwrap_or_else(|_| {
                // Handler panics become 500s in `blocking`; this covers a dropped leader.
                Arc::new(handlers::error(500, "the shared read was dropped"))
            });
        }

        let response = Arc::new(blocking(compute).await);
        // Remove before publishing so new callers start a fresh read.
        if let Some(waiting) = self.inflight().remove(&key) {
            let _ = waiting.send(Arc::clone(&response));
        }
        response
    }

    /// Recover a poisoned lock so later requests can still use the map.
    fn inflight(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, broadcast::Sender<Arc<Response>>>> {
        self.inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Keep file I/O and subprocess work off the async runtime workers.
async fn blocking<F>(work: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|e| handlers::error(500, &format!("the request handler failed: {e}")))
}

/// Embedded Trunk output; [`serve_asset`] uses a placeholder if `index.html` is absent.
static WEBAPP: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../adi-webapp/dist");

const SERVICE: &str = "adi-app";
/// Prefer the release tag from `build.rs` to the workspace version.
const VERSION: &str = match option_env!("ADI_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

const DEFAULT_PORT: u16 = 8090;

const DIST_ENV: &str = "ADI_WEBAPP_DIST";

/// Bound shutdown time for triggers that ignore SIGTERM.
const TRIGGER_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

/// `rustls-no-provider` requires an explicit provider. Call at client construction so tests
/// that bypass `main` also work; an already-installed provider is harmless.
pub(crate) fn ensure_tls_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let addr = listen_addr();
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr().unwrap_or(addr);
    let ports = Ports::new();
    let projects = Projects::open();
    let secrets = Secrets::open();
    let tasks = Tasks::open();
    let tools = Tools::open();
    // Tool seeding is best-effort so a store error cannot prevent startup.
    if let Err(e) = tools
        .seed_system()
        .and_then(|_| tools.sync_bin().map(|_| ()))
    {
        warn!(error = %e, "seeding system tools failed");
    }
    // Upgrade existing installs to the Hive-managed channel service. Provisioning failure
    // leaves the control panel usable; Hive owns the service's process and assigned port.
    if let Err(e) = adi_core::channels::ensure() {
        warn!(error = %e, "provisioning the Hive channel service failed");
    }
    // Initialize WAL before other users open the database; seed the shared Bun client.
    let db = Db::open();
    if let Err(e) = db.bootstrap() {
        warn!(error = %e, "bootstrapping the shared database failed");
    }
    let agents = Agents::open();
    // Migrate before consumers read definitions; log failures so the panel remains usable.
    match adi_agents::migrations::on_boot(&agents) {
        Ok(applied) => {
            if applied.agents > 0 {
                info!(
                    agents = applied.agents,
                    steps = %applied.steps.join(", "),
                    "migrated agent definitions"
                );
                for note in &applied.notes {
                    info!(%note, "migration");
                }
            }
            for (agent, why) in &applied.held {
                warn!(%agent, %why, "migration held an agent back");
            }
            if !applied.ahead.is_empty() {
                warn!(
                    agents = applied.ahead.len(),
                    writes = adi_agents::MANIFEST_VERSION,
                    claims = applied.ahead.values().copied().max().unwrap_or_default(),
                    "agent definitions are from a NEWER adi and were left alone — upgrade this binary"
                );
            }
        }
        Err(e) => warn!(error = %e, "migrating agent definitions failed"),
    }
    let knowledge = KnowledgeStore::open();
    let triggers = Triggers::open();
    let events = Events::open();
    let workflows = adi_workflows::Engine::open();
    if let Err(e) = workflows.bootstrap() {
        warn!(error = %e, "seeding the @adi/workflows Bun client failed");
    }
    let trigger_supervisor = Supervisor::start(triggers.clone());
    // Each consumer of the shared event log owns its own cursor, so this dispatcher and anything
    // else reading the log (the Hive channel service observes conversation state independently)
    // never race each other for the same rows.
    let event_dispatcher = EventDispatcher::start_watched(
        triggers.clone(),
        awaits::start(agents.clone(), workflows.clone()),
    );
    // Its own cursor on the same log, exactly like the dispatcher above — see that field's own
    // comment. On by default (ADI-MONO-157; `adi_usage::DEFAULT_ENABLED`); switched off, it still
    // catches up and acks, but queues and sends nothing.
    let usage = Usage::start_watched(
        projects.config().clone(),
        events.clone(),
        agents.clone(),
        projects.clone(),
        VERSION,
    );
    event_log::start(tasks.clone(), events.clone());
    prober::start(agents.clone());
    let dist = webapp_dist_override();
    if let Some(dir) = dist.as_ref() {
        info!(dist = %dir.display(), "serving webapp from disk (dev mode)");
    }
    info!(%local, registry = %ports.config().registry_path.display(), "adi-app listening");

    let app = Arc::new(App {
        ports,
        projects,
        secrets,
        db,
        tasks,
        tools,
        agents,
        knowledge,
        triggers,
        trigger_supervisor,
        events,
        workflows,
        mesh: MeshCtl::default(),
        dist,
        start: Instant::now(),
        reads: Reads::default(),
        live: live::Hub::default(),
    });

    live::start(Arc::clone(&app));

    // Respect the persisted mesh choice; a fresh install starts with the mesh disabled.
    {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            match adi_mesh::config::MeshConfig::load() {
                Ok(cfg) if cfg.enabled() => match app.mesh.start().await {
                    Ok(()) => info!("mesh autostarted"),
                    Err(e) => warn!(error = %e, "mesh autostart failed"),
                },
                Ok(_) => info!(
                    "mesh is off; turn it on from the Mesh page, `adi-mono mesh enable`, or by \
                     joining a fleet"
                ),
                Err(e) => warn!(error = %e, "could not read mesh config; mesh not autostarted"),
            }
        });
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let app = Arc::clone(&app);
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, &app).await {
                            debug!(%peer, error = %e, "connection error");
                        }
                    });
                }
                Err(e) => warn!(error = %e, "accept failed"),
            },
            () = adi_osext::shutdown_signal() => {
                info!("shutdown signal received; stopping");
                break;
            }
        }
    }
    // Trigger process groups outlive this process unless explicitly stopped.
    app.trigger_supervisor.stop(TRIGGER_STOP_GRACE).await;
    event_dispatcher.stop(TRIGGER_STOP_GRACE).await;
    usage.stop(TRIGGER_STOP_GRACE).await;
    app.mesh.stop().await;
    Ok(())
}

/// An explicit address/port argument takes precedence over `$PORT`, then [`DEFAULT_PORT`].
fn listen_addr() -> SocketAddr {
    if let Some(arg) = std::env::args().nth(1) {
        if let Ok(addr) = arg.parse::<SocketAddr>() {
            return addr;
        }
        if let Ok(port) = arg.parse::<u16>() {
            return SocketAddr::from(([127, 0, 0, 1], port));
        }
        warn!(arg = %arg, "ignoring unparseable listen argument");
    }
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    SocketAddr::from(([127, 0, 0, 1], port))
}

async fn handle(mut stream: TcpStream, app: &Arc<App>) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };
    debug!(method = %req.method, path = %req.path, "request");

    // This panel has no login. Reject cross-site API requests before routing, including
    // websocket upgrades, whose handshake must pass the same origin check.
    if req.route_path().starts_with("/api")
        && let Err(refusal) = origin::check(&req)
    {
        warn!(
            path = %req.path,
            origin = req.header("origin").unwrap_or("-"),
            status = refusal.status,
            "refused a request from another site",
        );
        let response = handlers::error(refusal.status, &refusal.message);
        return http::write_json(&mut stream, response.status, &response.body).await;
    }

    if req.method == "GET" && req.route_path() == "/api/ws" && req.is_websocket_upgrade() {
        return live::serve(stream, &req, app).await;
    }

    // Binary responses bypass the JSON router.
    if req.method == "GET"
        && let Some(id) = req.route_path().strip_prefix("/api/agents/attachment/")
    {
        return serve_attachment(&mut stream, &app.agents, id).await;
    }

    if req.method == "GET"
        && let Some(name) = req
            .route_path()
            .strip_prefix("/api/system/diagnose/download/")
    {
        return serve_diagnose_download(&mut stream, name).await;
    }

    // Unknown API paths must return a 404, not the SPA shell.
    if req.method == "GET" && !req.route_path().starts_with("/api") {
        return serve_asset(
            &mut stream,
            req.route_path(),
            app.dist.as_deref(),
            req.header("host"),
            req.header("accept-encoding"),
        )
        .await;
    }

    let response = answer(app, req).await;
    http::write_json(&mut stream, response.status, &response.body).await
}

/// Common routing for HTTP requests and live subscriptions.
///
/// The member-role gate (ADI-MONO-171 G3) runs first, ahead of both routers below: a blocked
/// route is blocked whether it would otherwise have been answered on the async path (most of
/// `/api/fleet/*`, `/api/mesh*`) or the blocking one (`/api/system/*`, `/api/secrets/*`, …), and
/// computing it once here is what lets neither router repeat the check.
async fn answer(app: &Arc<App>, req: http::Request) -> Arc<Response> {
    if let Some(refusal) = member_refusal(&app.projects, &req) {
        return Arc::new(refusal);
    }
    match async_route(app, &req).await {
        Some(response) => Arc::new(response),
        None => app.answer(req).await,
    }
}

/// Handle async routes, or return `None` for dispatch on the blocking pool.
async fn async_route(app: &App, req: &http::Request) -> Option<Response> {
    // Preserve the query string for the remote router.
    if let Some((node, path)) = viewer::split_node_path(&req.path) {
        return Some(
            viewer::proxy(
                &app.secrets,
                &req.method,
                node,
                path,
                req.header("content-type"),
                req.header("x-adi-filename"),
                &req.body,
            )
            .await,
        );
    }
    if channels::matches(req.route_path()) {
        return Some(channels::forward(req).await);
    }
    let response = match (req.method.as_str(), req.route_path()) {
        ("POST", "/api/secrets/refresh") => refresh_secret(&app.secrets, &req.body).await,
        ("POST", "/api/dashboards/transfer") => {
            transfer::transfer_dashboard(&app.projects, &app.ports, &req.body).await
        }
        ("GET", "/api/fleet/dashboards") => viewer::fleet_dashboards(&app.secrets).await,
        ("GET", "/api/fleet/reach") => viewer::reach(&app.secrets).await,
        ("POST", "/api/fleet/dashboards/unlock") => viewer::unlock(&app.secrets, &req.body).await,
        ("POST", "/api/fleet/dashboards/forget") => viewer::forget(&app.secrets, &req.body).await,
        ("POST", "/api/fleet/dashboards/allow") => viewer::allow(&app.secrets, &req.body).await,
        ("POST", "/api/fleet/join") => fleet_join(app, &req.body).await,
        ("GET", "/api/mesh") => handlers::mesh(app.mesh.running().await),
        ("POST", "/api/mesh/start") => mesh_start(&app.mesh).await,
        ("POST", "/api/mesh/stop") => mesh_stop(&app.mesh).await,
        ("POST", "/api/mesh/allow") => handlers::mesh_allow(app.mesh.running().await, &req.body),
        ("POST", "/api/mesh/deny") => handlers::mesh_deny(app.mesh.running().await, &req.body),
        ("POST", "/api/mesh/peers/allow") => {
            handlers::mesh_allow_peer(app.mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/peers/deny") => {
            handlers::mesh_deny_peer(app.mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/forwards/add") => {
            handlers::mesh_add_forward(app.mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/forwards/remove") => {
            handlers::mesh_remove_forward(app.mesh.running().await, &req.body)
        }
        _ => return None,
    };
    Some(response)
}

impl App {
    /// Coalesce eligible reads and dispatch synchronous work on the blocking pool.
    async fn answer(self: &Arc<Self>, req: http::Request) -> Arc<Response> {
        let key = shared_read_key(&req);
        let app = Arc::clone(self);
        let work = move || dispatch(&app, &req);
        match key {
            Some(key) => self.reads.shared(key, work).await,
            None => Arc::new(blocking(work).await),
        }
    }
}

/// Read-only GET routes eligible for coalescing.
///
/// `/api/secrets/mine` deliberately never goes here: [`shared_read_key`] keys on
/// `method + path + body` alone, with no identity in it, so two different users' `GET`s (same
/// empty body) would coalesce into one answer and leak one user's own names to the other's poll.
/// `/api/secrets/policy` has no such hazard (it names nobody), but is left off too rather than
/// drawing the one-route-is-safe-and-its-sibling-isn't line for a read this rare.
const SHARED_GETS: &[&str] = &[
    "/api/agents",
    "/api/agents/runs/all",
    "/api/dashboards",
    "/api/db",
    "/api/fleet",
    "/api/fleet/nodes",
    "/api/fleet/reach",
    "/api/embeddings/backends",
    "/api/hive",
    "/api/knowledge",
    "/api/llm/backends",
    "/api/meta",
    "/api/ports",
    "/api/ports/used",
    "/api/projects",
    "/api/secrets",
    "/api/settings/shared-assets",
    "/api/system",
    "/api/tasks",
    "/api/tools",
    "/api/triggers",
    "/api/update",
    "/api/usage-events",
    "/api/voice",
    "/api/workflows",
];

/// Read-only POST routes; their bodies identify the subject.
const SHARED_POSTS: &[&str] = &[
    "/api/agents/peek",
    "/api/agents/run/peek",
    "/api/agents/runs",
    "/api/llm/calls",
    "/api/llm/summary",
    "/api/projects/hook/log",
    "/api/projects/workspaces",
    "/api/triggers/log",
    "/api/workflows/runs",
    "/api/workflows/run/detail",
];

/// Larger bodies bypass coalescing to bound the in-flight map.
const MAX_SHARED_KEY_BODY: usize = 1024;

fn shared_read_key(req: &http::Request) -> Option<String> {
    let path = req.route_path();
    let shared = match req.method.as_str() {
        "GET" => SHARED_GETS.contains(&path) || path.starts_with("/api/projects/"),
        "POST" => SHARED_POSTS.contains(&path),
        _ => false,
    };
    if !shared || req.body.len() > MAX_SHARED_KEY_BODY {
        return None;
    }
    // Queries distinguish responses even though the allowlist matches only the route.
    Some(format!(
        "{} {}\n{}",
        req.method,
        req.path,
        String::from_utf8_lossy(&req.body)
    ))
}

/// Identity attached by the mesh gateway; absent on local panel requests.
///
/// Keyed on `X-Adi-Fleet-Petname`, not `X-Adi-Fleet-Node`: the petname is this machine's own,
/// pinned registry key for the connection (`adi_mesh::auth::FLEET_PETNAME_HEADER`), while the
/// node header carries the peer's self-declared nickname — a presentation-only value a name
/// clash can resolve differently next time (`docs/fleet.md` §2 rule 3). ADI-MONO-171 G3's role
/// check has to name the peer by something that cannot move out from under it, so it reads the
/// petname; `nickname` stays around for the conversation marker, which only ever wanted a label.
fn fleet_sender(req: &http::Request) -> Option<handlers::FleetSender<'_>> {
    let petname = req.header("x-adi-fleet-petname")?;
    Some(handlers::FleetSender {
        petname,
        nickname: req.header("x-adi-fleet-node").unwrap_or_default(),
        user: req.header("x-adi-fleet-user").unwrap_or_default(),
    })
}

/// This peer's role (`adi_mesh::fleet::Role`, ADI-MONO-171 G3): [`adi_mesh::fleet::Role::Admin`]
/// for a local caller (`sender: None`), which is every request this panel answered before roles
/// existed; [`adi_mesh::fleet::Role::Member`] — the safe default, not an open one — when the
/// registry cannot be read or no longer carries this petname, since both are reasons to trust a
/// peer with *less*, never more; otherwise whatever role the registry has on file for it.
fn peer_role(projects: &Projects, sender: Option<handlers::FleetSender<'_>>) -> adi_mesh::fleet::Role {
    let Some(sender) = sender else {
        return adi_mesh::fleet::Role::Admin;
    };
    adi_mesh::fleet::FleetRegistry::load_from(projects.config())
        .ok()
        .and_then(|registry| registry.get(sender.petname).map(|record| record.role))
        .unwrap_or(adi_mesh::fleet::Role::Member)
}

/// This peer's user scope (`docs/byo-credentials.md` §3): `Some(<id>)` — the **id**, never the
/// username — only for a peer whose registry record carries a [`user`](adi_mesh::fleet::NodeRecord::user),
/// minted or resolved for it by [`adi_mesh::join::decide`], never one copied in by
/// [`adi_mesh::join::record_viewer`] — and whose `auth.user` still matches the header's user.
/// `None` for a local caller (`sender: None`), an `adi` peer paired before this shipped, an
/// unknown or unreadable petname, or a stale header that no longer names the record's current
/// credential. On doubt it trusts the peer with less, exactly as [`peer_role`] does when it
/// defaults to [`adi_mesh::fleet::Role::Member`]: "no user layer, the server's values" rather than
/// guessing at whose values they are.
fn user_scope(projects: &Projects, sender: Option<handlers::FleetSender<'_>>) -> Option<String> {
    let sender = sender?;
    let registry = adi_mesh::fleet::FleetRegistry::load_from(projects.config()).ok()?;
    let record = registry.get(sender.petname)?;
    record.user.clone().filter(|_| record.auth.user == sender.user)
}

/// Refuse a member peer's request to a route ADI-MONO-171 G3 reserves for an admin — computed
/// once, ahead of both the async and blocking routers (`answer`), so neither has to repeat it.
/// `None` for a local caller, an admin peer, or a route the role does not restrict; everyone can
/// still read and run everything, because that part of the panel is collaborative by design.
fn member_refusal(projects: &Projects, req: &http::Request) -> Option<Response> {
    let sender = fleet_sender(req)?;
    if peer_role(projects, Some(sender)) != adi_mesh::fleet::Role::Member {
        return None;
    }
    member_blocked(req.method.as_str(), req.route_path(), &req.body, sender.petname)
}

/// The routes a member may never reach — `/api/system/*`, `/api/update/*`, `/api/licence*`,
/// `/api/mesh*`; every `/api/fleet/*` write except adding an `http:<label>` grant to its own
/// record; every secrets action that can read, write or refresh a secret (there is only one
/// scope today, global, but the gate is written to cover every scope there ever is), plus the
/// server's own `[user_secrets]` policy (`docs/byo-credentials.md` §4.1, ADI-MONO-178) — the
/// `/api/secrets/mine/*` routes are a member's own scope and are deliberately not named here
/// (§7: they are separate paths, not a carved-out exception); the run-limit setting; and creating
/// a hive service. Everything else (including every `GET`) is unrestricted.
fn member_blocked(method: &str, path: &str, body: &[u8], petname: &str) -> Option<Response> {
    let blocked = path.starts_with("/api/system")
        || path.starts_with("/api/update")
        || path.starts_with("/api/licence")
        || path.starts_with("/api/mesh")
        || (method == "POST"
            && path.starts_with("/api/fleet/")
            && !is_own_http_grant(path, body, petname))
        || (method == "POST"
            && matches!(
                path,
                "/api/secrets/reveal"
                    | "/api/secrets/set"
                    | "/api/secrets/set-oauth"
                    | "/api/secrets/remove"
                    | "/api/secrets/refresh"
                    | "/api/secrets/policy"
            ))
        || (method == "POST" && path == "/api/agents/limit")
        || (method == "POST" && path == "/api/hive/create");
    blocked.then(|| handlers::error(403, "a member peer may not reach this"))
}

/// The one `/api/fleet/*` write a member keeps: granting itself an `http:<label>` scratch
/// dashboard grant, which the panel already allows ("listing may also grant" — the fleet page's
/// own rule, not a new exception).
fn is_own_http_grant(path: &str, body: &[u8], petname: &str) -> bool {
    path == "/api/fleet/grants/add"
        && serde_json::from_slice::<adi_webapp_api::types::FleetGrantRef>(body).is_ok_and(|req| {
            req.petname.trim() == petname && req.grant.trim_start().starts_with("http:")
        })
}

/// Runs on the blocking pool because handlers perform file I/O and spawn subprocesses.
// Keep exact routes ahead of guarded prefix matches.
#[allow(clippy::too_many_lines)]
fn dispatch(app: &App, req: &http::Request) -> Response {
    let App {
        ports,
        projects,
        secrets,
        db,
        tasks,
        tools,
        agents,
        knowledge: knowledge_store,
        triggers,
        trigger_supervisor,
        events,
        workflows,
        start,
        ..
    } = app;
    let start = *start;
    let path = req.route_path();
    match (req.method.as_str(), path) {
        ("GET", "/api/health") => handlers::health(SERVICE, VERSION, start),
        ("GET", "/api/update") => handlers::update_state(),
        ("POST", "/api/update/check") => handlers::check_update(),
        ("POST", "/api/update/run") => handlers::run_update(),
        ("GET", "/api/system") => handlers::system_status(agents),
        ("POST", "/api/system/action") => handlers::run_system_action(agents, &req.body),
        ("POST", "/api/system/power") => handlers::run_system_power(&req.body),
        ("POST", "/api/system/restart") => handlers::restart_system(),
        ("POST", "/api/system/diagnose") => handlers::diagnose_system(),
        ("GET", "/api/settings/shared-assets") => handlers::shared_assets_state(),
        ("POST", "/api/settings/shared-assets") => handlers::set_shared_assets(&req.body),
        ("GET", "/api/usage-events") => handlers::usage_events_state(projects.config()),
        ("POST", "/api/usage-events/toggle") => {
            handlers::set_usage_events(projects.config(), &req.body)
        }
        ("POST", "/api/usage-events/forget") => handlers::forget_usage_events(projects.config()),
        ("GET", "/api/ports") => handlers::ports(ports),
        ("GET", "/api/ports/used") => handlers::used_ports(scan::listening_ports()),
        ("POST", "/api/ports/reserve") => handlers::reserve(ports, &req.body),
        ("POST", "/api/ports/release") => handlers::release(ports, &req.body),
        ("GET", "/api/projects") => handlers::projects(projects),
        ("POST", "/api/projects/create") => handlers::create_project(projects, &req.body),
        ("POST", "/api/projects/archive") => handlers::archive_project(projects, &req.body),
        ("POST", "/api/projects/unarchive") => handlers::unarchive_project(projects, &req.body),
        ("POST", "/api/projects/remove") => handlers::remove_project(projects, &req.body),
        // Renaming spans stores outside `adi_webapp_api`; see [`projects`].
        ("POST", "/api/projects/rename") => projects::rename_project(projects, &req.body),
        ("POST", "/api/projects/files") => handlers::list_files(projects, &req.body),
        ("POST", "/api/projects/file/read") => handlers::read_file(projects, &req.body),
        ("POST", "/api/projects/file/write") => handlers::write_file(projects, &req.body),
        ("POST", "/api/fs/list") => handlers::fs_list(projects, &req.body),
        ("POST", "/api/fs/read") => handlers::fs_read(projects, &req.body),
        ("POST", "/api/fs/write") => handlers::fs_write(projects, &req.body),
        ("POST", "/api/fs/create") => handlers::fs_create(projects, &req.body),
        ("POST", "/api/projects/workspaces") => handlers::workspaces_state(projects, &req.body),
        ("POST", "/api/projects/workspaces/create") => {
            handlers::create_workspace(projects, &req.body)
        }
        ("POST", "/api/projects/workspaces/remove") => {
            handlers::remove_workspace(projects, &req.body)
        }
        ("POST", "/api/projects/workspaces/terminal/open") => {
            handlers::open_workspace_terminal(projects, &req.body)
        }
        ("POST", "/api/projects/workspaces/terminal/peek") => {
            handlers::peek_workspace_terminal(projects, &req.body)
        }
        ("POST", "/api/projects/workspaces/terminal/send") => {
            handlers::send_workspace_terminal_keys(projects, &req.body)
        }
        ("POST", "/api/projects/workspaces/terminal/kill") => {
            handlers::kill_workspace_terminal(projects, &req.body)
        }
        ("POST", "/api/projects/hook/run") => handlers::run_project_hook(projects, &req.body),
        ("POST", "/api/projects/hook/log") => handlers::project_hook_log(projects, &req.body),
        ("POST", "/api/projects/hook/create") => handlers::create_project_hook(projects, &req.body),
        ("GET", p) if p.starts_with("/api/projects/") => {
            let live = scan::listening_ports();
            handlers::project_detail(projects, &p["/api/projects/".len()..], &live)
        }
        // Embedding-model loading must stay on the blocking pool.
        ("GET", "/api/knowledge") => handlers::knowledge(knowledge_store),
        ("POST", "/api/knowledge/search") => handlers::search_knowledge(knowledge_store, &req.body),
        ("POST", "/api/knowledge/notes") => handlers::knowledge_notes(knowledge_store, &req.body),
        ("POST", "/api/knowledge/note/get") => handlers::knowledge_note(knowledge_store, &req.body),
        ("POST", "/api/knowledge/note/add") => {
            handlers::add_knowledge_note(knowledge_store, &req.body)
        }
        ("POST", "/api/knowledge/note/edit") => {
            handlers::edit_knowledge_note(knowledge_store, &req.body)
        }
        ("POST", "/api/knowledge/note/remove") => {
            handlers::remove_knowledge_note(knowledge_store, &req.body)
        }
        ("POST", "/api/knowledge/base/create") => {
            handlers::create_knowledge_base(knowledge_store, &req.body)
        }
        ("POST", "/api/knowledge/base/remove") => {
            handlers::remove_knowledge_base(knowledge_store, &req.body)
        }
        ("POST", "/api/knowledge/reembed") => {
            handlers::reembed_knowledge(knowledge_store, &req.body)
        }
        ("GET", "/api/fleet") => handlers::fleet(projects.config()),
        ("GET", "/api/fleet/nodes") => viewer::nodes(secrets),
        // Minting creates a new nonce, so it must not be treated as a shareable read.
        ("POST", "/api/fleet/invite") => handlers::fleet_invite(projects.config(), &req.body),
        ("GET", "/api/fleet/invites") => handlers::fleet_invites(projects.config()),
        ("POST", "/api/fleet/invites/cancel") => {
            handlers::fleet_invite_cancel(projects.config(), &req.body)
        }
        ("POST", "/api/fleet/rename") => handlers::fleet_rename(projects.config(), &req.body),
        ("POST", "/api/fleet/unpair") => handlers::fleet_unpair(projects.config(), &req.body),
        // Unpair only after both directions have been dropped.
        ("POST", "/api/fleet/sources/drop") => viewer::drop_source(projects, secrets, &req.body),
        ("POST", "/api/fleet/viewers/drop") => viewer::drop_viewer(projects, secrets, &req.body),
        ("POST", "/api/fleet/grants/add") => handlers::fleet_grant(projects.config(), &req.body),
        ("POST", "/api/fleet/grants/remove") => {
            handlers::fleet_revoke(projects.config(), &req.body)
        }
        ("POST", "/api/fleet/instructions") => {
            handlers::fleet_instructions(projects.config(), &req.body)
        }
        ("POST", "/api/fleet/nickname/accept") => {
            handlers::fleet_accept_nickname(projects.config(), &req.body)
        }
        ("POST", "/api/fleet/nickname/dismiss") => {
            handlers::fleet_dismiss_nickname(projects.config(), &req.body)
        }
        ("GET", "/api/tasks") => handlers::tasks(tasks),
        ("POST", "/api/tasks/create") => handlers::create_task(tasks, &req.body),
        ("POST", "/api/tasks/archive") => handlers::archive_task(tasks, &req.body),
        ("POST", "/api/tasks/reopen") => handlers::reopen_task(tasks, &req.body),
        ("POST", "/api/tasks/delete") => handlers::delete_task(tasks, &req.body),
        ("POST", "/api/tasks/complete") => handlers::complete_task(tasks, &req.body),
        ("POST", "/api/tasks/give-up") => handlers::give_up_task(tasks, &req.body),
        ("GET", "/api/tools") => handlers::tools(tools),
        ("POST", "/api/tools/create") => handlers::create_tool(tools, &req.body),
        ("POST", "/api/tools/link") => handlers::link_tool(tools, &req.body),
        ("POST", "/api/tools/archive") => handlers::archive_tool(tools, &req.body),
        ("POST", "/api/tools/unarchive") => handlers::unarchive_tool(tools, &req.body),
        ("POST", "/api/tools/remove") => handlers::remove_tool(tools, &req.body),
        ("POST", "/api/tools/script/read") => handlers::read_tool_script(tools, &req.body),
        ("POST", "/api/tools/script/write") => handlers::write_tool_script(tools, &req.body),
        ("POST", "/api/tools/run") => handlers::run_tool(tools, projects, &req.body),

        // `query` uses a read-only connection; writes go through `exec`.
        ("GET", "/api/db") => handlers::db_state(db),
        ("POST", "/api/db/tables") => handlers::db_tables(db, &req.body),
        ("POST", "/api/db/schema") => handlers::db_schema(db, &req.body),
        ("POST", "/api/db/query") => handlers::db_query(db, &req.body),
        ("POST", "/api/db/exec") => handlers::db_exec(db, &req.body),

        ("POST", "/api/llm/summary") => handlers::llm_summary(db, &req.body),
        ("POST", "/api/llm/calls") => handlers::llm_calls(db, &req.body),
        ("POST", "/api/llm/call") => handlers::llm_call(db, &req.body),

        ("GET", "/api/llm/backends") => handlers::llm_backends(agents),
        ("POST", "/api/llm/backends/save") => handlers::save_llm_backend(agents, &req.body),
        ("POST", "/api/llm/backends/delete") => handlers::delete_llm_backend(agents, &req.body),
        // Sends a billed request without changing holds.
        ("POST", "/api/llm/backends/test") => handlers::test_llm_backend(
            agents,
            &req.body,
            user_scope(projects, fleet_sender(req)).as_deref(),
        ),
        ("POST", "/api/llm/holds/release") => handlers::release_llm_hold(agents, &req.body),
        ("POST", "/api/llm/settings") => handlers::save_llm_settings(agents, &req.body),

        // Not watchable: embedding settings change only through the panel; see `live::watchable`.
        ("GET", "/api/embeddings/backends") => handlers::embedding_backends(agents),
        ("POST", "/api/embeddings/backends/save") => {
            handlers::save_embedding_backend(agents, &req.body)
        }
        ("POST", "/api/embeddings/backends/delete") => {
            handlers::delete_embedding_backend(agents, &req.body)
        }
        ("POST", "/api/embeddings/backends/test") => {
            handlers::test_embedding_backend(agents, &req.body)
        }
        ("POST", "/api/embeddings/settings") => {
            handlers::save_embedding_settings(agents, &req.body)
        }

        ("GET", "/api/secrets") => handlers::secrets(secrets),
        ("POST", "/api/secrets/set") => handlers::set_secret(secrets, &req.body),
        ("POST", "/api/secrets/set-oauth") => handlers::set_oauth_secret(secrets, &req.body),
        ("POST", "/api/secrets/remove") => handlers::remove_secret(secrets, &req.body),
        ("POST", "/api/secrets/reveal") => handlers::reveal_secret(secrets, &req.body),
        ("GET", "/api/secrets/mine") => {
            handlers::secrets_mine(secrets, user_scope(projects, fleet_sender(req)).as_deref())
        }
        ("POST", "/api/secrets/mine/set") => handlers::set_my_secret(
            secrets,
            &req.body,
            user_scope(projects, fleet_sender(req)).as_deref(),
        ),
        ("POST", "/api/secrets/mine/remove") => handlers::remove_my_secret(
            secrets,
            &req.body,
            user_scope(projects, fleet_sender(req)).as_deref(),
        ),
        ("POST", "/api/secrets/mine/settings") => handlers::set_my_settings(
            secrets,
            &req.body,
            user_scope(projects, fleet_sender(req)).as_deref(),
        ),
        ("GET", "/api/secrets/policy") => handlers::secrets_policy(secrets),
        ("POST", "/api/secrets/policy") => handlers::set_secrets_policy(secrets, &req.body),
        ("GET", "/api/voice") => handlers::voice(secrets),
        ("POST", "/api/voice/transcribe") => handlers::transcribe(
            secrets,
            req.query_param("engine")
                .unwrap_or(handlers::BROWSER_ENGINE),
            req.header("content-type").unwrap_or("audio/webm"),
            &req.body,
        ),
        // Mints a fresh ephemeral key every call, so it is deliberately absent from SHARED_POSTS:
        // coalescing two of these would hand two browsers the same one-call key.
        ("POST", "/api/voice/realtime/session") => {
            handlers::realtime_session(secrets, &req.body)
        }
        ("GET", "/api/meta") => handlers::meta(agents, tools),
        ("GET", "/api/agents") => handlers::agents(agents),
        ("POST", "/api/agents/save") => {
            handlers::save_agent(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/delete") => {
            handlers::delete_agent(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/run") => {
            let sender = fleet_sender(req);
            handlers::run_agent(agents, &req.body, sender, user_scope(projects, sender))
        }
        ("POST", "/api/agents/limit") => handlers::set_run_limit(agents, &req.body),
        ("POST", "/api/agents/spawn-policy") => handlers::set_spawn_policy(agents, &req.body),
        ("POST", "/api/agents/spawn-rule") => handlers::set_spawn_rule(agents, &req.body),
        ("POST", "/api/agents/auto-title") => handlers::set_auto_title(agents, &req.body),
        ("POST", "/api/agents/runs") => handlers::agent_runs(agents, &req.body),
        ("GET", "/api/agents/runs/all") => handlers::all_agent_runs(
            agents,
            req.query_param("limit").and_then(|n| n.parse().ok()),
            req.query_param("hidden").and_then(|v| v.parse().ok()),
            req.query_param("filter")
                .and_then(handlers::RunFilter::from_query),
            fleet_sender(req),
        ),
        ("POST", "/api/agents/run/peek") => handlers::peek_run(agents, &req.body),
        // Fetch on demand; completed steps do not need a live subscription.
        ("POST", "/api/agents/run/steps") => handlers::run_steps(agents, &req.body),
        ("POST", "/api/agents/run/reply") => {
            handlers::reply_run(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/attachment") => handlers::store_attachment(
            agents,
            req.header("content-type").unwrap_or_default(),
            req.header("x-adi-filename").unwrap_or_default(),
            &req.body,
        ),
        // The question ID prevents an old tab from answering a replacement question.
        ("POST", "/api/agents/run/answer") => handlers::answer_run(agents, &req.body),
        ("GET", "/api/agents/questions") => handlers::pending_questions(agents),
        ("POST", "/api/agents/goals") => handlers::agent_goals(agents, &req.body),
        ("POST", "/api/agents/prompts") => handlers::agent_prompts(agents, &req.body),
        ("POST", "/api/agents/goal/set") => handlers::set_agent_goal(agents, &req.body),
        ("POST", "/api/agents/goal/close") => handlers::close_agent_goal(agents, &req.body),
        ("POST", "/api/agents/await/ignore") => handlers::ignore_await(agents, &req.body),
        // Retokenizing the transcript is too expensive for the polled peek response.
        ("POST", "/api/agents/run/tokens") => handlers::run_tokens(agents, &req.body),
        ("POST", "/api/agents/run/review") => handlers::review_run(agents, &req.body),
        ("POST", "/api/agents/simulate") => handlers::simulate_agent(agents, &req.body),
        ("POST", "/api/agents/simulate/prompt") => handlers::simulate_prompt(agents, &req.body),
        ("POST", "/api/agents/simulate/turn") => handlers::simulate_turn(agents, &req.body),
        ("POST", "/api/agents/simulate/reply") => handlers::simulate_reply(agents, &req.body),
        ("POST", "/api/agents/run/unqueue") => handlers::unqueue_run(agents, &req.body),
        ("POST", "/api/agents/run/stop") => {
            handlers::stop_run(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/run/delete") => {
            handlers::delete_run(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/run/hide") => {
            handlers::hide_run(agents, &req.body, fleet_sender(req))
        }
        ("POST", "/api/agents/run/star") => handlers::star_run(agents, &req.body),
        ("POST", "/api/agents/run/rename") => handlers::rename_run(agents, &req.body),
        ("POST", "/api/agents/stop") => handlers::stop_agent(agents, &req.body),
        ("POST", "/api/agents/peek") => handlers::peek_agent(agents, &req.body),
        ("POST", "/api/agents/send-keys") => handlers::send_agent_keys(agents, &req.body),
        ("GET", "/api/triggers") => handlers::triggers(triggers),
        ("POST", "/api/triggers/save") => {
            handlers::save_trigger(triggers, trigger_supervisor, &req.body)
        }
        ("POST", "/api/triggers/delete") => {
            handlers::delete_trigger(triggers, trigger_supervisor, &req.body)
        }
        ("POST", "/api/triggers/fire") => handlers::fire_trigger(triggers, &req.body),
        ("POST", "/api/triggers/restart") => {
            handlers::restart_trigger(triggers, trigger_supervisor, &req.body)
        }
        ("POST", "/api/triggers/log") => handlers::trigger_log(triggers, &req.body),
        ("POST", "/api/events/emit") => handlers::emit_event(events, &req.body),
        ("GET", "/api/workflows") => handlers::workflows(workflows),
        ("POST", "/api/workflows/enable") => handlers::enable_workflow(workflows, &req.body),
        ("POST", "/api/workflows/disable") => handlers::disable_workflow(workflows, &req.body),
        ("POST", "/api/workflows/run") => handlers::run_workflow(workflows, &req.body),
        ("POST", "/api/workflows/runs") => {
            handlers::list_runs(workflows, tasks, agents, &req.body)
        }
        ("POST", "/api/workflows/run/detail") => {
            handlers::run_detail(workflows, tasks, agents, &req.body)
        }
        ("POST", "/api/workflows/run/rewind") => {
            handlers::rewind_run(workflows, tasks, agents, &req.body)
        }
        // Preserve the query for webhook secrets; GET also supports provider pings.
        (m, p) if p.starts_with("/api/hooks/") && matches!(m, "POST" | "GET") => {
            let name = &p["/api/hooks/".len()..];
            let query = req.path.split_once('?').map_or("", |(_, q)| q);
            handlers::hook_trigger(triggers, name, query, &req.body)
        }
        ("GET", "/api/hive") => {
            let live = scan::listening_ports();
            handlers::hive(projects, ports, &live)
        }
        ("GET", "/api/dashboards") => {
            let live = scan::listening_ports();
            handlers::dashboards(projects.config(), ports, &live)
        }
        ("POST", "/api/dashboards/create") => {
            handlers::create_dashboard(projects.config(), ports, &req.body)
        }
        ("POST", "/api/dashboards/archive") => {
            let live = scan::listening_ports();
            handlers::archive_dashboard(projects.config(), ports, &live, &req.body)
        }
        ("POST", "/api/dashboards/unarchive") => {
            let live = scan::listening_ports();
            handlers::unarchive_dashboard(projects.config(), ports, &live, &req.body)
        }
        ("POST", "/api/dashboards/project") => {
            let live = scan::listening_ports();
            handlers::set_dashboard_project(projects.config(), ports, &live, &req.body)
        }
        ("POST", "/api/dashboards/delete") => {
            let live = scan::listening_ports();
            handlers::delete_dashboard(projects.config(), ports, &live, &req.body)
        }
        // Mesh imports have already passed the node password gate (`docs/fleet.md` §5).
        ("POST", "/api/dashboards/import") => {
            let live = scan::listening_ports();
            handlers::import_dashboard(projects, ports, &live, &req.body)
        }
        ("GET", "/api/marketplace") => handlers::marketplace(projects.config()),
        ("POST", "/api/marketplace/sync") => handlers::sync_marketplace(projects.config()),
        ("POST", "/api/marketplace/install") => {
            handlers::install_marketplace_app(projects.config(), &req.body)
        }
        ("POST", "/api/marketplace/uninstall") => {
            handlers::uninstall_marketplace_element(projects.config(), &req.body)
        }
        ("POST", "/api/marketplace/start") => {
            handlers::start_marketplace_app(projects.config(), &req.body)
        }
        ("POST", "/api/marketplace/start-service") => {
            handlers::start_marketplace_service(projects.config(), &req.body)
        }
        ("POST", "/api/marketplace/update") => {
            handlers::update_marketplace_app(projects.config(), &req.body)
        }
        ("POST", "/api/marketplace/bundle/update") => {
            handlers::update_marketplace_bundle(projects.config(), &req.body)
        }
        // Service changes invalidate the cached port scan.
        ("POST", "/api/hive/start") => {
            let response = handlers::start_service(projects, &req.body);
            scan::invalidate();
            response
        }
        ("POST", "/api/hive/stop") => {
            let response = handlers::stop_service(projects, &req.body);
            scan::invalidate();
            response
        }
        ("POST", "/api/hive/create") => {
            let live = scan::listening_ports();
            handlers::create_service(projects, &req.body, &live)
        }
        (_, p) if p.starts_with("/api") => handlers::error(404, "no such API endpoint"),
        _ => handlers::error(405, "method not allowed"),
    }
}

/// Persist the enabled choice even if startup fails, so the next restart retries.
async fn mesh_start(mesh: &MeshCtl) -> Response {
    if let Err(e) = persist_mesh_enabled(true) {
        return handlers::error(500, &format!("saving mesh config: {e}"));
    }
    match mesh.start().await {
        Ok(()) => handlers::mesh(true),
        Err(e) => handlers::error(500, &format!("starting mesh: {e}")),
    }
}

/// Persist the disabled choice before stopping the daemon.
async fn mesh_stop(mesh: &MeshCtl) -> Response {
    if let Err(e) = persist_mesh_enabled(false) {
        return handlers::error(500, &format!("saving mesh config: {e}"));
    }
    mesh.stop().await;
    handlers::mesh(false)
}

/// Persist an operator choice only; process shutdown must not change this setting.
fn persist_mesh_enabled(enabled: bool) -> anyhow::Result<()> {
    let mut cfg = adi_mesh::config::MeshConfig::load()?;
    cfg.set_enabled(enabled);
    cfg.save()
}

/// Join a fleet and return the minted credential once. Joining persists `enabled=true`
/// and starts the mesh immediately (`docs/fleet.md` §8).
async fn fleet_join(app: &App, body: &[u8]) -> Response {
    let (token, grant_back) = match handlers::fleet_join_token(body, now_secs()) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    match app.mesh.join(&token, grant_back).await {
        Ok(joined) => {
            record_front_door(&joined.viewer);
            // Idempotent when the join reused the running daemon.
            if let Err(e) = app.mesh.start().await {
                warn!(error = %e, "mesh daemon did not come up after a join");
            }
            handlers::fleet_joined(app.projects.config(), &joined)
        }
        Err(e) => handlers::error(502, &format!("{e:#}")),
    }
}

/// Update certificate names after the pairing is saved. Failure is advisory: HTTP routing
/// still works, but HTTPS may warn until the front-door certificate is refreshed.
fn record_front_door(petname: &str) {
    if adi_core::dns::Dns.add_mesh_node(petname) == adi_core::dns::MeshNodeChange::Failed {
        warn!(
            petname,
            "could not record the new peer in the front door's certificate list; \
             http://<service>.<peer>.n.adi works now, https:// will warn until it is refreshed"
        );
    }
}

fn oauth_router_url() -> String {
    std::env::var("ADI_OAUTH_ROUTER_URL")
        .unwrap_or_else(|_| "https://oauth-router.withadi.dev".to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Refresh and re-store an OAuth token without exposing the refresh token to the browser.
async fn refresh_secret(secrets: &Secrets, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<adi_webapp_api::types::SecretRef>(body) else {
        return handlers::error(
            400,
            "expected JSON body { \"name\": \"…\", \"project\"?: \"…\" }",
        );
    };
    let name = req.name.trim();
    if name.is_empty() {
        return handlers::error(400, "a secret name is required");
    }
    let project = req
        .project
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());

    let secret = match secrets.get(project, name) {
        Ok(Some(s)) => s,
        Ok(None) => return handlers::error(404, &format!("no such secret: {name}")),
        Err(e) => return Response::from(&e),
    };
    let Some(oauth) = secret.oauth else {
        return handlers::error(400, "not an OAuth secret — nothing to refresh");
    };
    if !oauth.has_refresh {
        return handlers::error(
            409,
            "no refresh token stored — re-authorize to get a new one",
        );
    }
    let refresh_token = match secrets.reveal_refresh(project, name) {
        Ok(Some(rt)) => rt,
        Ok(None) => return handlers::error(409, "no refresh token stored"),
        Err(e) => return Response::from(&e),
    };

    let url = format!(
        "{}/refresh/{}",
        oauth_router_url().trim_end_matches('/'),
        oauth.provider
    );
    ensure_tls_provider();
    let resp = match reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return handlers::error(502, &format!("could not reach the OAuth router: {e}")),
    };
    let status = resp.status();
    let payload: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return handlers::error(502, &format!("bad response from the OAuth router: {e}")),
    };
    if !status.is_success() {
        let msg = payload
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("refresh failed");
        return handlers::error(502, &format!("OAuth refresh failed: {msg}"));
    }
    let Some(access_token) = payload
        .get("access_token")
        .and_then(serde_json::Value::as_str)
    else {
        return handlers::error(502, "the OAuth router returned no access_token");
    };

    let rotated = payload
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let expires_at = payload
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .map(|s| now_secs().saturating_add(s));
    let scope = payload
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or(oauth.scope);
    let token = adi_secrets::OAuthToken {
        provider: oauth.provider,
        access_token: access_token.to_string(),
        refresh_token: rotated.or(Some(refresh_token)),
        expires_at,
        scope,
    };
    match secrets.set_oauth(project, name, &token, secret.description.as_deref()) {
        Ok(_) => handlers::secrets(secrets),
        Err(e) => Response::from(&e),
    }
}

/// Attachments are immutable; their random IDs also serve as cache keys.
async fn serve_attachment(stream: &mut TcpStream, agents: &Agents, id: &str) -> anyhow::Result<()> {
    let Some((media_type, bytes)) = handlers::attachment_bytes(agents, id) else {
        return http::write_json(stream, 404, r#"{"ok":false,"error":"no such attachment"}"#).await;
    };
    let (media_type, disposition) = if RENDERABLE.contains(&media_type.as_str()) {
        (media_type, "inline")
    } else {
        ("application/octet-stream".to_string(), "attachment")
    };
    http::write_cached(stream, &media_type, disposition, &bytes).await
}

/// Resolve downloads within the reports directory, never from a raw request path.
async fn serve_diagnose_download(stream: &mut TcpStream, name: &str) -> anyhow::Result<()> {
    let Some(path) = handlers::diagnose_download_path(name) else {
        return http::write_json(stream, 404, r#"{"ok":false,"error":"no such report"}"#).await;
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return http::write_json(stream, 404, r#"{"ok":false,"error":"no such report"}"#).await;
    };
    http::write_download(stream, "application/octet-stream", name, &bytes).await
}

/// Types safe to serve inline on the panel origin. Other types, including HTML and SVG,
/// download as `application/octet-stream`; `nosniff` prevents content-type promotion.
const RENDERABLE: [&str; 6] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "application/pdf",
    "text/plain",
];

/// Unknown asset paths fall back to the SPA shell, or a placeholder if it is not built.
/// Disk overrides bypass the CDN; embedded assets use `host` for `cdn-when-remote`.
async fn serve_asset(
    stream: &mut TcpStream,
    path: &str,
    dist: Option<&Path>,
    host: Option<&str>,
    accept_encoding: Option<&str>,
) -> anyhow::Result<()> {
    let rel = match path.trim_start_matches('/') {
        "" => "index.html",
        other => other,
    };
    match dist {
        Some(dir) => serve_from_disk(stream, dir, rel).await,
        None => serve_embedded(stream, rel, host, accept_encoding).await,
    }
}

/// Rewrite only the embedded shell for CDN mode; direct asset requests keep working locally.
///
/// Every embedded asset was precompressed at build time (`build.rs`), so negotiating a
/// coding is a lookup of the matching `.br`/`.gz` sibling, never a per-request compression.
/// The one exception is a CDN-mode shell rewrite: its bytes depend on `host`, so they can't
/// have been precompressed in advance, and that one response is sent as identity.
async fn serve_embedded(
    stream: &mut TcpStream,
    rel: &str,
    host: Option<&str>,
    accept_encoding: Option<&str>,
) -> anyhow::Result<()> {
    if rel != "index.html"
        && let Some(hit) = pick_embedded(rel, accept_encoding)
    {
        return write_build_file(stream, rel, hit.encoding, hit.bytes).await;
    }
    if let Some(index) = WEBAPP.get_file("index.html") {
        let html = "text/html; charset=utf-8";
        return match shared_assets::SharedAssets::active(VERSION, host) {
            Some(shared) => {
                let bytes = index.contents();
                let rewritten = shared_assets::rewrite(&String::from_utf8_lossy(bytes), &shared);
                http::write_response(stream, 200, "OK", html, rewritten.as_bytes()).await
            }
            None => {
                let hit = pick_embedded("index.html", accept_encoding)
                    .expect("index.html is embedded, so at least its identity bytes are");
                http::write_asset(stream, html, hit.encoding, hit.bytes).await
            }
        };
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

/// A resolved embedded asset: `encoding` is `Some("br"|"gzip")` when `bytes` is that
/// precompressed sibling, `None` for the identity file.
struct EmbeddedHit {
    encoding: Option<&'static str>,
    bytes: &'static [u8],
}

/// Pick the best precompressed sibling of `rel` the request accepts, falling back to the
/// identity file. `None` only when `rel` itself isn't embedded at all.
fn pick_embedded(rel: &str, accept_encoding: Option<&str>) -> Option<EmbeddedHit> {
    if accepts_coding(accept_encoding, "br")
        && let Some(file) = WEBAPP.get_file(format!("{rel}.br"))
    {
        return Some(EmbeddedHit {
            encoding: Some("br"),
            bytes: file.contents(),
        });
    }
    if accepts_coding(accept_encoding, "gzip")
        && let Some(file) = WEBAPP.get_file(format!("{rel}.gz"))
    {
        return Some(EmbeddedHit {
            encoding: Some("gzip"),
            bytes: file.contents(),
        });
    }
    WEBAPP.get_file(rel).map(|file| EmbeddedHit {
        encoding: None,
        bytes: file.contents(),
    })
}

/// Whether `Accept-Encoding` lists `coding` (or `*`) with a nonzero `q`. No header, or a
/// `q=0`/unparsable `q`, both mean "not accepted" — same as the coding being absent.
fn accepts_coding(accept_encoding: Option<&str>, coding: &str) -> bool {
    let Some(header) = accept_encoding else {
        return false;
    };
    header.split(',').any(|part| {
        let mut tokens = part.split(';');
        let name = tokens.next().unwrap_or("").trim();
        if !name.eq_ignore_ascii_case(coding) && name != "*" {
            return false;
        }
        let q = tokens
            .find_map(|p| p.trim().strip_prefix("q="))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        q > 0.0
    })
}

async fn serve_from_disk(stream: &mut TcpStream, dir: &Path, rel: &str) -> anyhow::Result<()> {
    if is_safe_rel(rel)
        && let Ok(bytes) = tokio::fs::read(dir.join(rel)).await
    {
        return write_build_file(stream, rel, None, &bytes).await;
    }
    if let Ok(bytes) = tokio::fs::read(dir.join("index.html")).await {
        let html = "text/html; charset=utf-8";
        return http::write_response(stream, 200, "OK", html, &bytes).await;
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

async fn write_build_file(
    stream: &mut TcpStream,
    rel: &str,
    encoding: Option<&str>,
    body: &[u8],
) -> anyhow::Result<()> {
    if is_content_addressed(rel) {
        http::write_asset_immutable(stream, content_type(rel), encoding, body).await
    } else {
        http::write_asset(stream, content_type(rel), encoding, body).await
    }
}

/// Trunk hashes the file name (`<name>-<hash>[_bg].<ext>`), with at least 12 hex digits.
/// A hash in a snippets directory names the crate, not the file contents, and is not cacheable.
fn is_content_addressed(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let stem = name.split('.').next().unwrap_or(name);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() >= 12 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `rel` has already had its leading slash stripped; reject parent-directory components.
fn is_safe_rel(rel: &str) -> bool {
    !rel.split('/').any(|c| c == "..")
}

fn webapp_dist_override() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(DIST_ENV)?);
    if dir.is_dir() {
        Some(dir)
    } else {
        warn!(dist = %dir.display(), "{DIST_ENV} is set but not a directory; ignoring");
        None
    }
}

fn placeholder_html() -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"dark\">\
         <title>adi</title>{style}</head>\
         <body><main class=\"adi-container\">\
         <p class=\"adi-logo\">adi</p>\
         <h1 class=\"adi-bar__title\" style=\"margin-top:48px\">The control panel is not built yet</h1>\
         <p class=\"adi-hint\">adi-app is running and serving the API, but there is no web UI in \
         <code>dist/</code> to hand you. Build it once:</p>\
         <pre class=\"adi-term\">scripts/build-app.sh</pre>\
         <p class=\"adi-hint\">or run <code>trunk build</code> in <code>crates/adi-webapp</code>, \
         then <code>cargo build -p adi-app</code>, and reload.</p>\
         </main></body></html>",
        style = adi_css::style_tag(),
    )
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn request(method: &str, path: &str, body: &str) -> http::Request {
        http::Request {
            method: method.to_string(),
            path: path.to_string(),
            headers: HashMap::new(),
            body: body.as_bytes().to_vec(),
            rest: Vec::new(),
        }
    }

    /// A store rooted in a scratch dir, like `awaits.rs`'s own `scratch` — the one other place
    /// this crate names the config layer directly (`adi-config` is a dev-dependency precisely
    /// for tests like these).
    fn scratch(tag: &str) -> Projects {
        let root = std::env::temp_dir().join(format!(
            "adi-app-main-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Projects::with_config(adi_config::Config::with_root(root))
    }

    /// File a fleet record under `petname` with the given role — enough to drive [`peer_role`]
    /// and [`member_refusal`] without pairing a real peer.
    fn pair_with_role(projects: &Projects, petname: &str, role: adi_mesh::fleet::Role) {
        let mut registry =
            adi_mesh::fleet::FleetRegistry::load_from(projects.config()).expect("load");
        registry.nodes.insert(
            petname.to_string(),
            adi_mesh::fleet::NodeRecord {
                key: std::iter::repeat_n('a', 52).collect(),
                nickname: petname.to_string(),
                role,
                ..adi_mesh::fleet::NodeRecord::default()
            },
        );
        registry.save_to(projects.config()).expect("save");
    }

    /// File a fleet record under `petname` with the given username and user id — enough to drive
    /// [`user_scope`] without pairing a real peer. `user: None` is what a record with no user
    /// scope (a legacy `adi` pairing, or one `record_viewer` filed) looks like.
    fn pair_with_user(projects: &Projects, petname: &str, username: &str, user: Option<&str>) {
        let mut registry =
            adi_mesh::fleet::FleetRegistry::load_from(projects.config()).expect("load");
        let mut record = adi_mesh::fleet::NodeRecord {
            key: std::iter::repeat_n('a', 52).collect(),
            nickname: petname.to_string(),
            ..adi_mesh::fleet::NodeRecord::default()
        };
        record.set_password(username, "pw");
        record.user = user.map(ToString::to_string);
        registry.nodes.insert(petname.to_string(), record);
        registry.save_to(projects.config()).expect("save");
    }

    fn with_petname(mut req: http::Request, petname: &str) -> http::Request {
        req.headers
            .insert("x-adi-fleet-petname".to_string(), petname.to_string());
        req
    }

    #[test]
    fn peer_role_is_admin_for_a_local_caller_and_for_an_unknown_or_unreadable_petname() {
        let projects = scratch("peer-role-defaults");
        assert_eq!(
            peer_role(&projects, None),
            adi_mesh::fleet::Role::Admin,
            "a local caller is unrestricted"
        );

        let sender = handlers::FleetSender {
            petname: "ghost",
            nickname: "ghost",
            user: "igor",
        };
        assert_eq!(
            peer_role(&projects, Some(sender)),
            adi_mesh::fleet::Role::Member,
            "an unknown petname fails safe, not open"
        );
    }

    #[test]
    fn peer_role_reads_whatever_the_registry_has_on_file() {
        let projects = scratch("peer-role-on-file");
        pair_with_role(&projects, "laptop-b", adi_mesh::fleet::Role::Admin);
        pair_with_role(&projects, "p42-device", adi_mesh::fleet::Role::Member);

        let admin = handlers::FleetSender {
            petname: "laptop-b",
            nickname: "laptop-b",
            user: "igor",
        };
        let member = handlers::FleetSender {
            petname: "p42-device",
            nickname: "p42-device",
            user: "p42",
        };
        assert_eq!(peer_role(&projects, Some(admin)), adi_mesh::fleet::Role::Admin);
        assert_eq!(
            peer_role(&projects, Some(member)),
            adi_mesh::fleet::Role::Member
        );
    }

    #[test]
    fn user_scope_is_none_for_a_local_caller_an_adi_peer_and_an_unknown_petname() {
        let projects = scratch("user-scope-none");
        assert_eq!(
            user_scope(&projects, None),
            None,
            "a local caller is the owner, on global secrets"
        );

        pair_with_user(&projects, "legacy", "adi", None);
        let legacy = handlers::FleetSender {
            petname: "legacy",
            nickname: "legacy",
            user: "adi",
        };
        assert_eq!(
            user_scope(&projects, Some(legacy)),
            None,
            "an adi peer has no scope until it is re-paired"
        );

        let ghost = handlers::FleetSender {
            petname: "ghost",
            nickname: "ghost",
            user: "u-whatever000000000000000",
        };
        assert_eq!(
            user_scope(&projects, Some(ghost)),
            None,
            "an unknown petname fails safe, not open"
        );
    }

    #[test]
    fn user_scope_is_none_for_an_unreadable_registry() {
        let projects = scratch("user-scope-unreadable");
        let module = projects.config().module(adi_mesh::config::MODULE);
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join("fleet.toml"), "not = [ toml").expect("write garbage");

        let sender = handlers::FleetSender {
            petname: "laptop-b",
            nickname: "laptop-b",
            user: "u-aaa",
        };
        assert_eq!(
            user_scope(&projects, Some(sender)),
            None,
            "an unreadable registry fails safe, not open"
        );
    }

    #[test]
    fn user_scope_is_the_id_only_when_the_header_matches_the_records_credential() {
        let projects = scratch("user-scope-minted");
        pair_with_user(&projects, "laptop-b", "u-abc123", Some("u-abc123"));

        let matching = handlers::FleetSender {
            petname: "laptop-b",
            nickname: "laptop-b",
            user: "u-abc123",
        };
        assert_eq!(
            user_scope(&projects, Some(matching)),
            Some("u-abc123".to_string())
        );

        let stale = handlers::FleetSender {
            petname: "laptop-b",
            nickname: "laptop-b",
            user: "u-stale00000000000000000",
        };
        assert_eq!(
            user_scope(&projects, Some(stale)),
            None,
            "a header that no longer matches the record's credential is no scope"
        );
    }

    #[test]
    fn user_scope_answers_the_id_never_the_pinned_username() {
        // A pinned username (ADI-MONO-179: the partner sandbox's `p<uid>`) is not the id —
        // `user_scope` must answer `NodeRecord.user`, never `auth.user`.
        let projects = scratch("user-scope-pinned");
        pair_with_user(&projects, "p42-device", "p42", Some("u-shared0000000000000000"));

        let sender = handlers::FleetSender {
            petname: "p42-device",
            nickname: "p42-device",
            user: "p42",
        };
        assert_eq!(
            user_scope(&projects, Some(sender)),
            Some("u-shared0000000000000000".to_string()),
            "the id, not the username the peer authenticated as"
        );
    }

    /// The exact route list ADI-MONO-171 G3 names: system/update/licence/mesh outright, every
    /// fleet write except granting itself an `http:<label>`, every secrets action — including
    /// `set-oauth` and `refresh`, both missing from this gate until ADI-MONO-174 — the run-limit
    /// setting, and creating a hive service. Everything else — including every `GET` — is
    /// unrestricted.
    #[test]
    fn member_blocked_matches_the_documented_route_list() {
        let petname = "p42-device";
        for (method, path) in [
            ("GET", "/api/system"),
            ("POST", "/api/system/restart"),
            ("GET", "/api/update"),
            ("POST", "/api/update/run"),
            ("GET", "/api/licence"),
            ("GET", "/api/mesh"),
            ("POST", "/api/mesh/start"),
            ("POST", "/api/secrets/reveal"),
            ("POST", "/api/secrets/set"),
            ("POST", "/api/secrets/set-oauth"),
            ("POST", "/api/secrets/remove"),
            ("POST", "/api/secrets/refresh"),
            ("POST", "/api/agents/limit"),
            ("POST", "/api/hive/create"),
            ("POST", "/api/fleet/unpair"),
            ("POST", "/api/fleet/invite"),
            ("POST", "/api/fleet/rename"),
        ] {
            assert!(
                member_blocked(method, path, b"", petname).is_some(),
                "{method} {path} should be blocked for a member"
            );
        }

        for (method, path, body) in [
            ("GET", "/api/fleet", b"".as_slice()),
            ("GET", "/api/agents", b""),
            ("POST", "/api/agents/run", b"{}"),
            ("POST", "/api/agents/run/stop", b"{}"),
            ("POST", "/api/agents/save", b"{}"),
            (
                "POST",
                "/api/fleet/grants/add",
                br#"{"petname":"p42-device","grant":"http:nosh"}"#,
            ),
        ] {
            assert!(
                member_blocked(method, path, body, petname).is_none(),
                "{method} {path} should be open to a member"
            );
        }
    }

    /// ADI-MONO-174: `/api/secrets/set-oauth` and `/api/secrets/refresh` let a member overwrite
    /// or refresh a global OAuth secret (e.g. `CLAUDE_CODE_OAUTH_TOKEN`) because they were absent
    /// from `member_blocked`, unlike the sibling `set`/`remove`/`reveal` routes.
    #[test]
    fn member_blocked_covers_set_oauth_and_refresh_like_the_other_secrets_routes() {
        let petname = "p42-device";
        assert!(
            member_blocked("POST", "/api/secrets/set-oauth", b"{}", petname).is_some(),
            "a member must not overwrite a global OAuth secret"
        );
        assert!(
            member_blocked("POST", "/api/secrets/refresh", b"{}", petname).is_some(),
            "a member must not refresh a global secret"
        );
    }

    /// ADI-MONO-178 §4.1: the server's own `[user_secrets]` policy is admin-only, exactly like
    /// the global-scope secrets actions — but the `/mine` routes need no exception carved into
    /// this list at all, because they are separate paths this list never names (§7).
    #[test]
    fn member_blocked_covers_the_policy_route_but_never_the_mine_routes() {
        let petname = "p42-device";
        assert!(
            member_blocked("POST", "/api/secrets/policy", b"{}", petname).is_some(),
            "a member must not change the server's own user-secrets policy"
        );
        for path in [
            "/api/secrets/mine",
            "/api/secrets/mine/set",
            "/api/secrets/mine/remove",
            "/api/secrets/mine/settings",
        ] {
            assert!(
                member_blocked("POST", path, b"{}", petname).is_none(),
                "{path} is a member's own scope, never blocked"
            );
        }
    }

    /// The one `/api/fleet/*` write a member keeps: granting itself an `http:<label>` on its own
    /// record. The same path refuses a grant for anybody else's record, a non-`http:` grant, or
    /// the one other fleet write (`grants/remove`) the task names as still restricted.
    #[test]
    fn a_member_may_only_grant_itself_an_http_label_on_its_own_record() {
        let petname = "p42-device";
        assert!(
            member_blocked(
                "POST",
                "/api/fleet/grants/add",
                br#"{"petname":"p42-device","grant":"http:nosh"}"#,
                petname
            )
            .is_none(),
            "granting itself an http label is the one exception"
        );
        assert!(
            member_blocked(
                "POST",
                "/api/fleet/grants/add",
                br#"{"petname":"someone-else","grant":"http:nosh"}"#,
                petname
            )
            .is_some(),
            "granting another record anything is still blocked"
        );
        assert!(
            member_blocked(
                "POST",
                "/api/fleet/grants/add",
                br#"{"petname":"p42-device","grant":"tcp:127.0.0.1:22"}"#,
                petname
            )
            .is_some(),
            "a non-http grant is still blocked"
        );
        assert!(
            member_blocked(
                "POST",
                "/api/fleet/grants/remove",
                br#"{"petname":"p42-device","grant":"http:nosh"}"#,
                petname
            )
            .is_some(),
            "removing a grant is not the exception, only adding one is"
        );
    }

    #[test]
    fn member_refusal_refuses_a_member_peer_and_lets_everyone_else_through() {
        let projects = scratch("member-refusal");
        pair_with_role(&projects, "p42-device", adi_mesh::fleet::Role::Member);
        pair_with_role(&projects, "laptop-b", adi_mesh::fleet::Role::Admin);

        let blocked = with_petname(request("GET", "/api/system", ""), "p42-device");
        let refusal = member_refusal(&projects, &blocked).expect("a member is refused");
        assert_eq!(refusal.status, 403);

        let admin_same_route = with_petname(request("GET", "/api/system", ""), "laptop-b");
        assert!(
            member_refusal(&projects, &admin_same_route).is_none(),
            "an admin peer is unrestricted"
        );

        let local = request("GET", "/api/system", "");
        assert!(
            member_refusal(&projects, &local).is_none(),
            "a local caller is unrestricted"
        );

        let open_route = with_petname(request("GET", "/api/fleet", ""), "p42-device");
        assert!(
            member_refusal(&projects, &open_route).is_none(),
            "a route the role does not restrict is unaffected"
        );
    }

    #[test]
    fn fleet_sender_reads_the_mesh_gateways_identity_headers() {
        let mut forwarded = request("POST", "/api/agents/run", "{}");
        forwarded
            .headers
            .insert("x-adi-fleet-node".to_string(), "laptop-b".to_string());
        forwarded
            .headers
            .insert("x-adi-fleet-user".to_string(), "igor".to_string());
        forwarded
            .headers
            .insert("x-adi-fleet-petname".to_string(), "laptop-b".to_string());
        let sender = fleet_sender(&forwarded).expect("a sender");
        assert_eq!(sender.nickname, "laptop-b");
        assert_eq!(sender.user, "igor");
        assert_eq!(sender.petname, "laptop-b");

        assert!(
            fleet_sender(&request("POST", "/api/agents/run", "{}")).is_none(),
            "a local request carries no fleet identity"
        );
        // The petname is what identity hinges on now, never the dialler-chosen nickname —
        // ADI-MONO-171 G1 (`docs/fleet.md` §2 rule 3: a nickname is only ever a suggestion).
        let mut no_petname = request("POST", "/api/agents/run", "{}");
        no_petname
            .headers
            .insert("x-adi-fleet-node".to_string(), "laptop-b".to_string());
        no_petname
            .headers
            .insert("x-adi-fleet-user".to_string(), "igor".to_string());
        assert!(
            fleet_sender(&no_petname).is_none(),
            "a node header with no petname is not a fleet sender"
        );
    }

    #[test]
    fn only_a_build_file_named_by_its_hash_is_cached() {
        assert!(is_content_addressed("adi-webapp-7fde788d5e37b14c_bg.wasm"));
        assert!(is_content_addressed("adi-webapp-7fde788d5e37b14c.js"));
        assert!(is_content_addressed("adi-webapp-81b3a4fdd05fb69_bg.wasm"));
        assert!(is_content_addressed("main-42001df2ba433658.css"));
        assert!(!is_content_addressed("index.html"));
        assert!(!is_content_addressed("sw.js"));
        assert!(!is_content_addressed("rerenders.js"));
        assert!(!is_content_addressed("elements/adi-elements.js"));
        assert!(!is_content_addressed("manifest.webmanifest"));
        assert!(!is_content_addressed(
            "snippets/adi-webapp-723bd312eb4d940f/inline0.js"
        ));
        assert!(!is_content_addressed("icon-192.png"));
        assert!(!is_content_addressed("mark-maskable.svg"));
    }

    #[test]
    fn accepts_coding_picks_br_over_gzip_over_identity() {
        assert!(accepts_coding(Some("br"), "br"));
        assert!(accepts_coding(Some("gzip, br"), "br"));
        assert!(accepts_coding(Some("gzip, br"), "gzip"));
        assert!(!accepts_coding(Some("gzip"), "br"));
        assert!(!accepts_coding(None, "br"));
    }

    #[test]
    fn accepts_coding_is_case_insensitive_and_reads_star() {
        assert!(accepts_coding(Some("BR"), "br"));
        assert!(accepts_coding(Some("*"), "br"));
        assert!(accepts_coding(Some("*"), "gzip"));
    }

    #[test]
    fn accepts_coding_honours_a_zero_quality() {
        // Firefox and curl both send q-values this way; a ruled-out coding must be rejected
        // exactly like an absent one, not treated as "mentioned, so fine".
        assert!(!accepts_coding(Some("br;q=0"), "br"));
        assert!(!accepts_coding(Some("br;q=0.0, gzip"), "br"));
        assert!(accepts_coding(Some("br;q=0.3"), "br"));
        // An unparsable q is treated as the default (1.0), not as a reason to reject.
        assert!(accepts_coding(Some("br;q=nonsense"), "br"));
    }

    #[tokio::test]
    async fn write_build_file_sets_content_encoding_and_vary_only_when_compressed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            write_build_file(&mut stream, "app.js", Some("br"), b"brotli-bytes")
                .await
                .unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut raw = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut client, &mut raw)
            .await
            .unwrap();
        server.await.unwrap();

        let text = String::from_utf8(raw).unwrap();
        assert!(text.contains("Content-Encoding: br\r\n"), "{text}");
        assert!(text.contains("Vary: Accept-Encoding\r\n"), "{text}");
        assert!(text.contains("Content-Type: text/javascript"), "{text}");
        assert!(text.ends_with("brotli-bytes"), "{text}");
    }

    #[tokio::test]
    async fn write_build_file_sends_no_content_encoding_for_identity() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            write_build_file(&mut stream, "app.js", None, b"plain-bytes")
                .await
                .unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut raw = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut client, &mut raw)
            .await
            .unwrap();
        server.await.unwrap();

        let text = String::from_utf8(raw).unwrap();
        assert!(!text.contains("Content-Encoding:"), "{text}");
        // A cache still needs to know identity was a negotiated outcome, not the only one.
        assert!(text.contains("Vary: Accept-Encoding\r\n"), "{text}");
        assert!(text.ends_with("plain-bytes"), "{text}");
    }

    #[test]
    fn only_reads_are_shareable() {
        assert!(shared_read_key(&request("GET", "/api/agents", "")).is_some());
        assert!(shared_read_key(&request("GET", "/api/projects/abc", "")).is_some());
        assert!(shared_read_key(&request("POST", "/api/agents/run/peek", "{}")).is_some());

        assert!(
            shared_read_key(&request("POST", "/api/agents/run", "{}")).is_none(),
            "launching a run is not a read"
        );
        assert!(
            shared_read_key(&request("POST", "/api/agents/run/reply", "{}")).is_none(),
            "saying something into a chat is not a read"
        );
        assert!(
            shared_read_key(&request("GET", "/api/nope", "")).is_none(),
            "an unrouted path is never shared"
        );
    }

    #[test]
    fn the_key_separates_subjects_and_bounds_itself() {
        let one = shared_read_key(&request("POST", "/api/agents/runs", r#"{"name":"a"}"#));
        let other = shared_read_key(&request("POST", "/api/agents/runs", r#"{"name":"b"}"#));
        assert!(
            one.is_some() && one != other,
            "different agents, different keys"
        );

        let page = shared_read_key(&request("GET", "/api/agents/runs/all?limit=100", ""));
        assert!(
            page.is_some(),
            "the route is still the one on the allowlist"
        );
        assert_ne!(
            page,
            shared_read_key(&request("GET", "/api/agents/runs/all?limit=200", "")),
        );
        assert_ne!(
            page,
            shared_read_key(&request("GET", "/api/agents/runs/all", "")),
            "a page and the whole index are not the same answer"
        );

        let huge = "x".repeat(MAX_SHARED_KEY_BODY + 1);
        assert!(
            shared_read_key(&request("POST", "/api/agents/runs", &huge)).is_none(),
            "an oversized body is answered on its own rather than growing the map"
        );
    }

    #[tokio::test]
    async fn concurrent_readers_share_one_answer() {
        let reads = Reads::default();
        let runs = Arc::new(AtomicUsize::new(0));

        let compute = |runs: Arc<AtomicUsize>| {
            move || {
                let nth = runs.fetch_add(1, Ordering::SeqCst);
                // Give the other readers time to join the in-flight read.
                std::thread::sleep(std::time::Duration::from_millis(120));
                handlers::error(200, &format!("read #{nth}"))
            }
        };

        let key = "GET /api/agents".to_string();
        let (first, second, third) = tokio::join!(
            reads.shared(key.clone(), compute(Arc::clone(&runs))),
            reads.shared(key.clone(), compute(Arc::clone(&runs))),
            reads.shared(key.clone(), compute(Arc::clone(&runs))),
        );

        assert_eq!(runs.load(Ordering::SeqCst), 1, "one read, not three");
        assert_eq!(first.body, second.body);
        assert_eq!(second.body, third.body);
        assert!(
            reads.inflight().is_empty(),
            "the slot is freed once the read is answered"
        );
    }

    #[tokio::test]
    async fn different_reads_are_not_shared() {
        let reads = Reads::default();
        let runs = Arc::new(AtomicUsize::new(0));

        let compute = |runs: Arc<AtomicUsize>| {
            move || {
                runs.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(60));
                handlers::error(200, "ok")
            }
        };
        let (_, _) = tokio::join!(
            reads.shared("GET /api/agents".into(), compute(Arc::clone(&runs))),
            reads.shared("GET /api/tasks".into(), compute(Arc::clone(&runs))),
        );
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_panicking_read_answers_its_followers() {
        let reads = Reads::default();
        let key = "GET /api/agents".to_string();
        let (leader, follower) = tokio::join!(
            reads.shared(key.clone(), || {
                std::thread::sleep(std::time::Duration::from_millis(80));
                panic!("handler exploded");
            }),
            reads.shared(key.clone(), || handlers::error(200, "never runs")),
        );

        assert_eq!(leader.status, 500);
        assert_eq!(follower.status, 500);
        assert!(reads.inflight().is_empty(), "the slot is released");
    }
}
