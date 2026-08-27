//! adi-app — the adi application server behind `app.adi`: one process serving the
//! control-panel webapp at `GET /` and a JSON `/api/*` backend over [`adi_ports_manager`].
//! Listens on `$PORT` or an explicit `addr` argument, on loopback.
//!
//! The UI is the Leptos app [`adi-webapp`](../adi-webapp), compiled to wasm by Trunk. Its
//! `dist/` output is embedded here at build time; set `ADI_WEBAPP_DIST=/path/to/dist` to
//! serve those files from disk instead (a dev mode — rebuild the UI with `trunk build` and
//! refresh, no re-embed). The API handlers live in [`adi_webapp_api::handlers`] and share
//! their DTO types with that frontend.

mod awaits;
mod http;
mod live;
mod node;
mod origin;
mod projects;
mod scan;
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
use adi_knowledge::KnowledgeStore;
use adi_db::Db;
use adi_events::Events;
use adi_mesh::Daemon;
use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_secrets::Secrets;
use adi_tasks::Tasks;
use adi_tools::Tools;
use adi_triggers::{EventDispatcher, Supervisor, Triggers};
use adi_webapp_api::handlers;
use adi_webapp_api::handlers::Response;
use include_dir::{Dir, include_dir};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, info, warn};

/// Everything a request may need, held once and shared by every connection.
///
/// One `Arc<App>` rather than a dozen: a synchronous handler runs on the blocking pool, which
/// needs an owned `'static` handle to what it touches (see [`App::answer`]). Grouping the stores
/// is what makes that a clone of one pointer instead of twelve.
struct App {
    ports: Ports,
    projects: Projects,
    secrets: Secrets,
    db: Db,
    tasks: Tasks,
    tools: Tools,
    agents: Agents,
    /// The knowledge store, held for the life of the process **on purpose**: the embedding model
    /// loads lazily into it and stays there, so the second search is instant. A handler that
    /// opened its own store per request would reload the weights every time.
    knowledge: KnowledgeStore,
    triggers: Triggers,
    trigger_supervisor: Arc<Supervisor>,
    events: Events,
    mesh: MeshCtl,
    /// A `dist/` to serve the webapp from instead of the embedded copy ([`DIST_ENV`]).
    dist: Option<PathBuf>,
    /// When the process started, for `/api/health`'s uptime.
    start: Instant,
    reads: Reads,
    /// The `/api/ws` live channel: what each connected control panel is watching, and the answers
    /// it has already been sent. See [`live`].
    live: live::Hub,
}

/// Owns the mesh [`Daemon`] the control panel starts/stops in-process, so it lives only as
/// long as this app. `None` when stopped. The async mutex serializes start/stop.
#[derive(Debug, Default)]
struct MeshCtl {
    daemon: Mutex<Option<Daemon>>,
}

impl MeshCtl {
    /// Whether the mesh daemon is currently running.
    async fn running(&self) -> bool {
        self.daemon.lock().await.is_some()
    }

    /// Start the daemon if it isn't already up.
    async fn start(&self) -> anyhow::Result<()> {
        let mut slot = self.daemon.lock().await;
        if slot.is_none() {
            // With this machine's stored node passwords, so a `*.n.adi` link opened here does not
            // ask for one the panel already holds — see [`crate::viewer::HeldCredentials`].
            *slot = Some(Daemon::start_with(Some(Arc::new(viewer::HeldCredentials))).await?);
        }
        Ok(())
    }

    /// Stop the daemon if it's running (a clean teardown: tasks joined, ticket cleared).
    async fn stop(&self) {
        if let Some(daemon) = self.daemon.lock().await.take() {
            daemon.stop().await;
        }
    }
}

/// Reads that are already in flight, so several askers share one answer.
///
/// The control panel polls: an open chat asks for its run list and its transcript once a second,
/// and the rails refetch the agent list, every agent's sessions and the dashboards every four —
/// per tab. When one of those reads takes longer than the interval, the next tick fires anyway and
/// the requests stack up, each redoing byte-for-byte the same work. This collapses that: the first
/// asker computes, everyone who asks the same thing while it runs waits on *its* result, and they
/// all get the same response.
///
/// Only routes named by [`shared_read_key`] take part — reads, where "the answer a moment ago" and
/// "the answer now" are the same answer. Nothing that mutates is ever shared.
#[derive(Debug, Default)]
struct Reads {
    inflight: std::sync::Mutex<HashMap<String, broadcast::Sender<Arc<Response>>>>,
}

impl Reads {
    /// The answer to `key`, computing it only if nobody else already is.
    async fn shared<F>(&self, key: String, compute: F) -> Arc<Response>
    where
        F: FnOnce() -> Response + Send + 'static,
    {
        // Claim the slot or join it, holding the lock for exactly that decision — never across
        // the read itself, which is the slow part everyone is waiting on.
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
                // Only reachable if the leader's connection task vanished between claiming the
                // slot and answering; it cannot happen through a handler panic, which [`blocking`]
                // turns into a 500 the followers receive like any other answer.
                Arc::new(handlers::error(500, "the shared read was dropped"))
            });
        }

        let response = Arc::new(blocking(compute).await);
        // Free the slot before publishing, so the next poll starts a fresh read rather than
        // joining one that has already finished.
        if let Some(waiting) = self.inflight().remove(&key) {
            let _ = waiting.send(Arc::clone(&response));
        }
        response
    }

    /// A previous panic while holding this lock says nothing about the map, so a poisoned lock is
    /// taken anyway rather than failing every later request.
    fn inflight(&self) -> std::sync::MutexGuard<'_, HashMap<String, broadcast::Sender<Arc<Response>>>>
    {
        self.inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Run a synchronous handler on tokio's blocking pool.
///
/// Every `/api` handler below reads files, stats pids and sometimes spawns a subprocess. Run
/// directly in the connection's async task — as they used to be — a handful of them occupy every
/// runtime worker at once, and the server stops answering *anything*: `/api/health`, which touches
/// nothing, was observed taking 22 seconds behind them. On the blocking pool they can take as long
/// as they take without a single async worker being held.
async fn blocking<F>(work: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|e| handlers::error(500, &format!("the request handler failed: {e}")))
}

/// The webapp's Trunk build output, embedded so the binary is self-contained. Empty until
/// `trunk build` runs in `crates/adi-webapp`; [`serve_asset`] serves a placeholder when
/// `index.html` is absent.
static WEBAPP: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../adi-webapp/dist");

/// Service identity reported at `/api/health`.
const SERVICE: &str = "adi-app";
/// The release tag this was built from when there is one (see `build.rs`), so `/api/health`
/// reports the same number as the bundle it shipped in rather than the workspace floor.
const VERSION: &str = match option_env!("ADI_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Fallback listen port when `$PORT` is unset and no `addr` argument is given.
const DEFAULT_PORT: u16 = 8090;

/// Env var pointing at a webapp `dist/` to serve from disk instead of the embedded copy.
const DIST_ENV: &str = "ADI_WEBAPP_DIST";

/// How long to wait for background triggers to exit at shutdown before giving up on them — a
/// code block that ignores SIGTERM must not hold the whole app open.
const TRIGGER_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

/// Install `ring` as the process-wide rustls provider, once.
///
/// reqwest is built with `rustls-no-provider` (see the workspace manifest), which means it picks
/// no crypto provider of its own and *panics* when a client is built without one. So this is
/// called at each client construction rather than once in `main`: the tests reach the request
/// paths directly, never through `main`, and a start-up-only install left them panicking inside
/// `Client::builder().build()`. `install_default` errors only if a provider is already set, which
/// is exactly the outcome wanted on the second and every later call.
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
    // Ensure the built-in system tools (the adi-ecosystem CLIs) exist, then rebuild the global
    // `.bin`. Best-effort — a store that can't be seeded shouldn't stop the app from starting.
    if let Err(e) = tools.seed_system().and_then(|_| tools.sync_bin().map(|_| ())) {
        warn!(error = %e, "seeding system tools failed");
    }
    // Create the global database (so it exists in WAL mode before anything races to make it) and
    // seed the `@adi/db` Bun client into the store's node_modules, so `import … from "@adi/db"`
    // resolves from every `.ts` the platform runs. Best-effort, like the tools above.
    let db = Db::open();
    if let Err(e) = db.bootstrap() {
        warn!(error = %e, "bootstrapping the shared database failed");
    }
    let agents = Agents::open();
    // Opening the store loads nothing — the embedding model is built on the first call that
    // genuinely needs one (a search, an add), and then stays for the life of the process.
    let knowledge = KnowledgeStore::open();
    let triggers = Triggers::open();
    let events = Events::open();
    // Background triggers are long-lived processes owned by this app: the supervisor keeps
    // every enabled one running for as long as the app is up, and stops them on the way out.
    let trigger_supervisor = Supervisor::start(triggers.clone());
    // Event triggers, in turn, are fired on demand: the dispatcher drains the shared event spool
    // (which task/agent mutations and the emit endpoint publish onto) and launches every enabled
    // event trigger whose patterns match a drained event.
    //
    // Triggers are not the only subscriber: a harness run can register an *await* — "wake me when
    // this is published" — and the await worker is what honors it. It watches from the dispatcher's
    // side rather than draining the spool itself, because two drainers would race for records.
    let event_dispatcher =
        EventDispatcher::start_watched(triggers.clone(), awaits::start(agents.clone()));
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
        mesh: MeshCtl::default(),
        dist,
        start: Instant::now(),
        reads: Reads::default(),
        live: live::Hub::default(),
    });

    // The live channel's clock: it recomputes only what some open page is watching, so until a
    // control panel connects this costs a wakeup every quarter second and nothing else.
    live::start(Arc::clone(&app));

    // The mesh daemon runs in-process, so it lives only as long as this app. Autostart it
    // (non-blocking, best-effort) so the whole stack is up once the app is — the control
    // panel's Stop button still stops it for the session.
    {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            match app.mesh.start().await {
                Ok(()) => info!("mesh autostarted"),
                Err(e) => warn!(error = %e, "mesh autostart failed"),
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
    // Background triggers run in their own process groups so the supervisor can signal their
    // whole tree — which also means they outlive this process unless they are stopped first.
    // Waiting here is what keeps a restart from leaking a copy of every background trigger.
    app.trigger_supervisor.stop(TRIGGER_STOP_GRACE).await;
    // The dispatcher owns no child processes (fired event triggers are detached one-offs), so
    // this just ends its poll loop cleanly.
    event_dispatcher.stop(TRIGGER_STOP_GRACE).await;
    app.mesh.stop().await;
    Ok(())
}

/// Resolve where to listen: an explicit `addr` argument wins, else `$PORT`, else `DEFAULT_PORT`.
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

/// Read one request, route it, and write the response.
async fn handle(mut stream: TcpStream, app: &Arc<App>) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };
    debug!(method = %req.method, path = %req.path, "request");

    // Before anything is routed: this panel has no login, so a page on another site must not be
    // able to drive it (see [`origin`]). Ahead of the websocket branch on purpose — the handshake
    // is the one route where this check is the only guard that can ever exist.
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

    // The live channel leaves HTTP behind entirely: past the handshake this connection is a
    // websocket for as long as the page is open, not a request and a response.
    if req.method == "GET" && req.route_path() == "/api/ws" && req.is_websocket_upgrade() {
        return live::serve(stream, &req, app).await;
    }

    // An image somebody attached to a message, served back to the page drawing the transcript. It
    // is handled here rather than in the JSON router because it is the one `/api` route whose answer
    // is bytes: the router's `Response` is a status and a JSON string by construction.
    if req.method == "GET"
        && let Some(id) = req.route_path().strip_prefix("/api/agents/attachment/")
    {
        return serve_attachment(&mut stream, &app.agents, id).await;
    }

    // Any GET outside `/api` is a webapp asset, streamed straight back from memory or disk.
    // Inside `/api` an unknown path is a 404 from the router, not the app shell.
    if req.method == "GET" && !req.route_path().starts_with("/api") {
        return serve_asset(&mut stream, req.route_path(), app.dist.as_deref()).await;
    }

    let response = answer(app, req).await;
    http::write_json(&mut stream, response.status, &response.body).await
}

/// Answer one read or mutation: the few genuinely asynchronous routes first, then the synchronous
/// dispatch — shared with anyone else asking the same thing, and off the async workers either way.
///
/// Shared with the live channel, which answers a subscription with the very same routing rather
/// than a parallel set of handlers that could drift from it.
async fn answer(app: &Arc<App>, req: http::Request) -> Arc<Response> {
    match async_route(app, &req).await {
        Some(response) => Arc::new(response),
        None => app.answer(req).await,
    }
}

/// The few routes that are genuinely asynchronous: an outbound HTTP call, and the in-process mesh
/// daemon behind its async mutex. `None` means "not one of these" — a synchronous route, which
/// [`App::answer`] takes off the runtime entirely.
async fn async_route(app: &App, req: &http::Request) -> Option<Response> {
    let response = match (req.method.as_str(), req.route_path()) {
        // Server-side: decrypt the refresh token, exchange it at the router, re-store. Async
        // because it makes an outbound call, so it can't be a plain sync handler.
        ("POST", "/api/secrets/refresh") => refresh_secret(&app.secrets, &req.body).await,
        // Send a dashboard to a paired node. Async for the same reason: it is an outbound call —
        // through this machine's mesh gateway, at the node's own control panel (see [`transfer`]).
        ("POST", "/api/dashboards/transfer") => {
            transfer::transfer_dashboard(&app.projects, &app.ports, &req.body).await
        }
        // What the *fleet* is running: one authenticated call to each paired node's own control
        // panel, over the same gateway (see [`viewer`]). Async for the same reason as a transfer —
        // every one of these leaves the machine.
        ("GET", "/api/fleet/dashboards") => viewer::fleet_dashboards(&app.secrets).await,
        ("POST", "/api/fleet/dashboards/unlock") => viewer::unlock(&app.secrets, &req.body).await,
        ("POST", "/api/fleet/dashboards/forget") => viewer::forget(&app.secrets, &req.body).await,
        ("POST", "/api/fleet/dashboards/allow") => viewer::allow(&app.secrets, &req.body).await,
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
    /// Answer a synchronous request without occupying an async worker: shared with whoever else is
    /// asking the same thing right now, and run on the blocking pool either way.
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

/// GET routes that only look at state, and so may be shared between concurrent askers.
const SHARED_GETS: &[&str] = &[
    "/api/agents",
    "/api/agents/runs/all",
    "/api/dashboards",
    "/api/db",
    "/api/fleet",
    "/api/hive",
    "/api/knowledge",
    "/api/meta",
    "/api/ports",
    "/api/ports/used",
    "/api/projects",
    "/api/secrets",
    "/api/tasks",
    "/api/tools",
    "/api/triggers",
    "/api/update",
    "/api/voice",
];

/// POST routes that are reads despite the method — the polled ones carry their subject (an agent
/// name, a run id) in the body, which is why they are POSTs at all.
const SHARED_POSTS: &[&str] = &[
    "/api/agents/peek",
    "/api/agents/run/peek",
    "/api/agents/runs",
    "/api/projects/hook/log",
    "/api/projects/workspaces",
    "/api/triggers/log",
];

/// The largest body a shared read may be keyed on. Every route above sends a handful of fields;
/// anything larger is answered on its own rather than growing the in-flight map.
const MAX_SHARED_KEY_BODY: usize = 1024;

/// How to identify a request that may be answered together with identical ones in flight, or
/// `None` for everything else — which is every mutation, and so every route not named above.
fn shared_read_key(req: &http::Request) -> Option<String> {
    let path = req.route_path();
    let shared = match req.method.as_str() {
        // `/api/projects/<id>` is one project's detail page, a read like the bare list above.
        "GET" => SHARED_GETS.contains(&path) || path.starts_with("/api/projects/"),
        "POST" => SHARED_POSTS.contains(&path),
        _ => false,
    };
    if !shared || req.body.len() > MAX_SHARED_KEY_BODY {
        return None;
    }
    // Keyed on the *full* path, query and all: the allowlist is matched without one, but
    // `?limit=100` and `?limit=200` are different answers to the same route, and sharing would
    // hand one asker the other's page.
    Some(format!(
        "{} {}\n{}",
        req.method,
        req.path,
        String::from_utf8_lossy(&req.body)
    ))
}

/// Route a synchronous request. Runs on the blocking pool ([`blocking`]), never on an async
/// worker: nearly every arm reads files, and several spawn a subprocess.
// One flat table of routes, deliberately: splitting it by prefix would hide the ordering the
// guarded arms depend on, and every arm is a single line of dispatch.
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
        start,
        ..
    } = app;
    let start = *start;
    let path = req.route_path();
    match (req.method.as_str(), path) {
        ("GET", "/api/health") => handlers::health(SERVICE, VERSION, start),
        // Auto-update (`docs/adi-update.md`). The `GET` reads two files and is what the top
        // bar's version pill polls; `check` fetches the release manifest; `run` hands the
        // install to the bundled CLI, which restarts this very process on its way through.
        ("GET", "/api/update") => handlers::update_state(),
        ("POST", "/api/update/check") => handlers::check_update(),
        ("POST", "/api/update/run") => handlers::run_update(),
        ("GET", "/api/ports") => handlers::ports(ports),
        ("GET", "/api/ports/used") => handlers::used_ports(scan::listening_ports()),
        ("POST", "/api/ports/reserve") => handlers::reserve(ports, &req.body),
        ("POST", "/api/ports/release") => handlers::release(ports, &req.body),
        ("GET", "/api/projects") => handlers::projects(projects),
        ("POST", "/api/projects/create") => handlers::create_project(projects, &req.body),
        ("POST", "/api/projects/archive") => handlers::archive_project(projects, &req.body),
        ("POST", "/api/projects/unarchive") => handlers::unarchive_project(projects, &req.body),
        ("POST", "/api/projects/remove") => handlers::remove_project(projects, &req.body),
        // Not a `handlers::` arm: a rename spans stores that crate does not reach (see [`projects`]).
        ("POST", "/api/projects/rename") => projects::rename_project(projects, &req.body),
        ("POST", "/api/projects/files") => handlers::list_files(projects, &req.body),
        ("POST", "/api/projects/file/read") => handlers::read_file(projects, &req.body),
        ("POST", "/api/projects/file/write") => handlers::write_file(projects, &req.body),
        // The store browser: the whole ~/.adi/mono tree, jailed to it (see handlers::fs).
        ("POST", "/api/fs/list") => handlers::fs_list(projects, &req.body),
        ("POST", "/api/fs/read") => handlers::fs_read(projects, &req.body),
        ("POST", "/api/fs/write") => handlers::fs_write(projects, &req.body),
        ("POST", "/api/fs/create") => handlers::fs_create(projects, &req.body),
        // Workspaces & project hooks: working copies created by the script files under a
        // project's .adi/hooks, registered in its .adi/workspaces.toml. All POST under
        // /api/projects/… — NOT /api/hooks/*, which is the triggers webhook URL space.
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
        // A single project's detail (manifest + its .adi/hive.yaml services). The id is the
        // trailing path segment; the exact routes above (all POST, or the bare GET) win first.
        ("GET", p) if p.starts_with("/api/projects/") => {
            let live = scan::listening_ports();
            handlers::project_detail(projects, &p["/api/projects/".len()..], &live)
        }
        // The knowledge base (`docs/knowledge.md`): scoped collections of text notes, searched by
        // meaning. These run here, on the blocking pool, and not as `async_route` arms — a search
        // may load the embedding model, which is seconds of CPU the async workers must not spend.
        ("GET", "/api/knowledge") => handlers::knowledge(knowledge_store),
        ("POST", "/api/knowledge/search") => {
            handlers::search_knowledge(knowledge_store, &req.body)
        }
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
        // The fleet: the remote adi nodes this machine is paired with (`docs/fleet.md`). The
        // registry is a file in the shared store, so these take the store the projects registry
        // already holds open rather than reaching for `~/.adi/mono` a second time — which is also
        // what lets the handlers be tested against a temp root.
        ("GET", "/api/fleet") => handlers::fleet(projects.config()),
        // Minting is a POST with no body: it takes nothing and it is not a read — every call
        // writes a fresh nonce into the invite book.
        ("POST", "/api/fleet/invite") => handlers::fleet_invite(projects.config()),
        ("POST", "/api/fleet/rename") => handlers::fleet_rename(projects.config(), &req.body),
        ("POST", "/api/fleet/unpair") => handlers::fleet_unpair(projects.config(), &req.body),
        ("POST", "/api/fleet/grants/add") => handlers::fleet_grant(projects.config(), &req.body),
        ("POST", "/api/fleet/grants/remove") => handlers::fleet_revoke(projects.config(), &req.body),
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
        // Tools: user CLIs (sh/ts) created in-store or linked by path, exposed as tools/.bin/<name>
        // shims agents run. Every mutation returns the fresh list for one-round-trip updates.
        ("GET", "/api/tools") => handlers::tools(tools),
        ("POST", "/api/tools/create") => handlers::create_tool(tools, &req.body),
        ("POST", "/api/tools/link") => handlers::link_tool(tools, &req.body),
        ("POST", "/api/tools/archive") => handlers::archive_tool(tools, &req.body),
        ("POST", "/api/tools/unarchive") => handlers::unarchive_tool(tools, &req.body),
        ("POST", "/api/tools/remove") => handlers::remove_tool(tools, &req.body),
        ("POST", "/api/tools/script/read") => handlers::read_tool_script(tools, &req.body),
        ("POST", "/api/tools/script/write") => handlers::write_tool_script(tools, &req.body),
        // A run resolves a project-scoped tool's cwd from the project registry.
        ("POST", "/api/tools/run") => handlers::run_tool(tools, projects, &req.body),

        // The shared SQLite store. `query` runs on a read-only connection and `exec` on a
        // read-write one, so browsing can never write — see handlers::db.
        ("GET", "/api/db") => handlers::db_state(db),
        ("POST", "/api/db/tables") => handlers::db_tables(db, &req.body),
        ("POST", "/api/db/schema") => handlers::db_schema(db, &req.body),
        ("POST", "/api/db/query") => handlers::db_query(db, &req.body),
        ("POST", "/api/db/exec") => handlers::db_exec(db, &req.body),

        ("GET", "/api/secrets") => handlers::secrets(secrets),
        ("POST", "/api/secrets/set") => handlers::set_secret(secrets, &req.body),
        ("POST", "/api/secrets/set-oauth") => handlers::set_oauth_secret(secrets, &req.body),
        ("POST", "/api/secrets/remove") => handlers::remove_secret(secrets, &req.body),
        ("POST", "/api/secrets/reveal") => handlers::reveal_secret(secrets, &req.body),
        // Dictation. The clip arrives as a raw body — it is already bytes with a content type
        // from `MediaRecorder`, and wrapping it in JSON would cost a base64 third for nothing.
        ("GET", "/api/voice") => handlers::voice(secrets),
        ("POST", "/api/voice/transcribe") => handlers::transcribe(
            secrets,
            req.query_param("engine").unwrap_or(handlers::BROWSER_ENGINE),
            // Chrome records WebM/Opus and Safari MP4; the fallback only matters for a caller
            // that sent no type at all, and webm is the likelier guess.
            req.header("content-type").unwrap_or("audio/webm"),
            &req.body,
        ),
        // The Meta page's state: the well-known `adi-agent` (if set up), the defaults to seed a
        // new one with (system prompt + every active tool), and the agent form schema. Reads the
        // same agents store; the tools store supplies the default tool set.
        ("GET", "/api/meta") => handlers::meta(agents, tools),
        ("GET", "/api/agents") => handlers::agents(agents),
        ("POST", "/api/agents/save") => handlers::save_agent(agents, &req.body),
        ("POST", "/api/agents/delete") => handlers::delete_agent(agents, &req.body),
        ("POST", "/api/agents/run") => handlers::run_agent(agents, &req.body),
        ("POST", "/api/agents/limit") => handlers::set_run_limit(agents, &req.body),
        ("POST", "/api/agents/runs") => handlers::agent_runs(agents, &req.body),
        // `?limit=N` is the chat rail's page — the newest N sessions across every agent. Absent
        // (or unparseable) means the whole index, which is what the pages that read all of it ask
        // for.
        ("GET", "/api/agents/runs/all") => handlers::all_agent_runs(
            agents,
            req.query_param("limit").and_then(|n| n.parse().ok()),
        ),
        ("POST", "/api/agents/run/peek") => handlers::peek_run(agents, &req.body),
        ("POST", "/api/agents/run/reply") => handlers::reply_run(agents, &req.body),
        // An image on its way into a message. Raw bytes with their type in the header, like the
        // dictation clip above — the page already holds both, and JSON would cost a base64 third.
        // The bytes are read back out at `GET /api/agents/attachment/<id>`, which is not routed
        // here: it answers with bytes rather than JSON, so it is handled before this dispatch.
        ("POST", "/api/agents/attachment") => handlers::store_attachment(
            agents,
            req.header("content-type").unwrap_or_default(),
            req.header("x-adi-filename").unwrap_or_default(),
            &req.body,
        ),
        // Settle the question a conversation stopped to ask. Distinct from a reply because it
        // names the ask it answers, so a card left open in another tab cannot answer the question
        // that replaced it.
        ("POST", "/api/agents/run/answer") => handlers::answer_run(agents, &req.body),
        // Every conversation waiting on a person, across every agent — the "needs you" inbox.
        ("GET", "/api/agents/questions") => handlers::pending_questions(agents),
        // What a conversation is for. `goals` reads (a conversation's, or every open one when the
        // body names nothing); the two writes are set-or-reword and the two ways of closing.
        ("POST", "/api/agents/goals") => handlers::agent_goals(agents, &req.body),
        ("POST", "/api/agents/goal/set") => handlers::set_agent_goal(agents, &req.body),
        ("POST", "/api/agents/goal/close") => handlers::close_agent_goal(agents, &req.body),
        // What a conversation is waiting on the *world* for. The list rides the run listing and the
        // conversation snapshot, so there is nothing to read here — only the one write, which drops
        // a wake that is never coming and would otherwise hold the chat open for a week.
        ("POST", "/api/agents/await/ignore") => handlers::ignore_await(agents, &req.body),
        // What a conversation spent its context on. Its own endpoint, not part of the peek: it
        // re-tokenizes the whole transcript, and the peek is polled once a second.
        ("POST", "/api/agents/run/tokens") => handlers::run_tokens(agents, &req.body),
        // Hand the same conversation to an agent and ask how it should have gone. Writes the
        // dossier, then launches the reviewer on it — the answer arrives as its own conversation.
        ("POST", "/api/agents/run/review") => handlers::review_run(agents, &req.body),
        // A run of the agent with a person in the model's seat: the agent's own environment, its
        // own composed prompt, and tools that really execute. All four answer the same state
        // object, so a page never renders a prompt one turn behind what it just did.
        ("POST", "/api/agents/simulate") => handlers::simulate_agent(agents, &req.body),
        ("POST", "/api/agents/simulate/prompt") => handlers::simulate_prompt(agents, &req.body),
        ("POST", "/api/agents/simulate/turn") => handlers::simulate_turn(agents, &req.body),
        ("POST", "/api/agents/simulate/reply") => handlers::simulate_reply(agents, &req.body),
        ("POST", "/api/agents/run/unqueue") => handlers::unqueue_run(agents, &req.body),
        ("POST", "/api/agents/run/stop") => handlers::stop_run(agents, &req.body),
        ("POST", "/api/agents/run/delete") => handlers::delete_run(agents, &req.body),
        ("POST", "/api/agents/run/hide") => handlers::hide_run(agents, &req.body),
        ("POST", "/api/agents/run/star") => handlers::star_run(agents, &req.body),
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
        // Replace a supervised background trigger's process without changing its definition.
        ("POST", "/api/triggers/restart") => {
            handlers::restart_trigger(triggers, trigger_supervisor, &req.body)
        }
        ("POST", "/api/triggers/log") => handlers::trigger_log(triggers, &req.body),
        // Publish a platform event by hand — the app's dispatcher fires matching event triggers.
        ("POST", "/api/events/emit") => handlers::emit_event(events, &req.body),
        // The public webhook endpoint: fire an enabled `webhook` trigger with the request body
        // as its payload. GET is accepted too — some webhook providers ping with it. The secret
        // (when the trigger requires one) rides in the query, which route_path() strips.
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
        // The receiving half of a transfer: another machine handing us a dashboard it packed.
        // Reached over the mesh, so it is gated by this node's own password before it ever gets
        // here (`docs/fleet.md` §5) — the same gate every other route on this panel sits behind.
        ("POST", "/api/dashboards/import") => {
            let live = scan::listening_ports();
            handlers::import_dashboard(projects, ports, &live, &req.body)
        }
        // Starting or stopping a service changes what is listening, and the page asks that next —
        // so drop the port-scan memo rather than answering it from a scan taken before the change.
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

/// `POST /api/mesh/start` — bring the in-process mesh daemon up, then report fresh state.
async fn mesh_start(mesh: &MeshCtl) -> Response {
    match mesh.start().await {
        Ok(()) => handlers::mesh(true),
        Err(e) => handlers::error(500, &format!("starting mesh: {e}")),
    }
}

/// `POST /api/mesh/stop` — stop the in-process mesh daemon, then report fresh state.
async fn mesh_stop(mesh: &MeshCtl) -> Response {
    mesh.stop().await;
    handlers::mesh(false)
}

/// The OAuth router base URL used for server-side secret refresh (override with
/// `ADI_OAUTH_ROUTER_URL`, e.g. for a self-hosted or local router).
fn oauth_router_url() -> String {
    std::env::var("ADI_OAUTH_ROUTER_URL")
        .unwrap_or_else(|_| "https://oauth-router.withadi.dev".to_string())
}

/// Seconds since the Unix epoch (for stamping a refreshed token's absolute expiry).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `POST /api/secrets/refresh` — renew an OAuth secret's access token using its stored refresh
/// token, entirely server-side: the refresh token is decrypted here, exchanged at the router
/// (which holds the provider client secret), and the fresh token is re-stored. The refresh token
/// never crosses to the browser. Returns the fresh secrets list on success.
async fn refresh_secret(secrets: &Secrets, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<adi_webapp_api::types::SecretRef>(body) else {
        return handlers::error(400, "expected JSON body { \"name\": \"…\", \"project\"?: \"…\" }");
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

    // Find the secret and confirm it's an OAuth secret with a refresh token.
    let secret = match secrets.get(project, name) {
        Ok(Some(s)) => s,
        Ok(None) => return handlers::error(404, &format!("no such secret: {name}")),
        Err(e) => return Response::from(&e),
    };
    let Some(oauth) = secret.oauth else {
        return handlers::error(400, "not an OAuth secret — nothing to refresh");
    };
    if !oauth.has_refresh {
        return handlers::error(409, "no refresh token stored — re-authorize to get a new one");
    }
    let refresh_token = match secrets.reveal_refresh(project, name) {
        Ok(Some(rt)) => rt,
        Ok(None) => return handlers::error(409, "no refresh token stored"),
        Err(e) => return Response::from(&e),
    };

    // Exchange it at the router (holds the client secret); the refresh token stays server-side.
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
    let Some(access_token) = payload.get("access_token").and_then(serde_json::Value::as_str) else {
        return handlers::error(502, "the OAuth router returned no access_token");
    };

    // Re-store: new access token; the provider's rotated refresh token if it issued one, else
    // keep the current one; the new expiry and scope.
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

/// Serve one attached image's bytes, or a 404 when the id names nothing.
///
/// Cached hard: an attachment is immutable and its id is minted from random bytes, so the page that
/// draws a chat every second must not re-fetch every screenshot in it. The id changing *is* the
/// invalidation.
async fn serve_attachment(
    stream: &mut TcpStream,
    agents: &Agents,
    id: &str,
) -> anyhow::Result<()> {
    let Some((media_type, bytes)) = handlers::attachment_bytes(agents, id) else {
        return http::write_json(stream, 404, r#"{"ok":false,"error":"no such attachment"}"#).await;
    };
    http::write_cached(stream, &media_type, &bytes).await
}

/// Serve a webapp asset. With a disk override ([`DIST_ENV`]) set, files come from that
/// directory; otherwise from the embedded copy. Either way, an unknown path falls back to
/// the app shell (`index.html`) for client-side routing, and the placeholder if the webapp
/// isn't built yet.
async fn serve_asset(
    stream: &mut TcpStream,
    path: &str,
    dist: Option<&Path>,
) -> anyhow::Result<()> {
    let rel = match path.trim_start_matches('/') {
        "" => "index.html",
        other => other,
    };
    match dist {
        Some(dir) => serve_from_disk(stream, dir, rel).await,
        None => serve_embedded(stream, rel).await,
    }
}

/// Serve `rel` from the embedded `dist/`, falling back to the shell / placeholder.
async fn serve_embedded(stream: &mut TcpStream, rel: &str) -> anyhow::Result<()> {
    if let Some(file) = WEBAPP.get_file(rel) {
        return http::write_response(stream, 200, "OK", content_type(rel), file.contents()).await;
    }
    if let Some(index) = WEBAPP.get_file("index.html") {
        let html = "text/html; charset=utf-8";
        return http::write_response(stream, 200, "OK", html, index.contents()).await;
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

/// Serve `rel` from a `dist/` directory on disk (the [`DIST_ENV`] dev mode), falling back
/// to the shell / placeholder.
async fn serve_from_disk(stream: &mut TcpStream, dir: &Path, rel: &str) -> anyhow::Result<()> {
    if is_safe_rel(rel)
        && let Ok(bytes) = tokio::fs::read(dir.join(rel)).await
    {
        return http::write_response(stream, 200, "OK", content_type(rel), &bytes).await;
    }
    if let Ok(bytes) = tokio::fs::read(dir.join("index.html")).await {
        let html = "text/html; charset=utf-8";
        return http::write_response(stream, 200, "OK", html, &bytes).await;
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

/// Reject path traversal: `rel` has its leading `/` stripped already, so joining it to the
/// dist dir stays inside as long as no component is `..`.
fn is_safe_rel(rel: &str) -> bool {
    !rel.split('/').any(|c| c == "..")
}

/// The `ADI_WEBAPP_DIST` override, if it points at an existing directory.
fn webapp_dist_override() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(DIST_ENV)?);
    if dir.is_dir() {
        Some(dir)
    } else {
        warn!(dist = %dir.display(), "{DIST_ENV} is set but not a directory; ignoring");
        None
    }
}

/// The page shown when the webapp isn't built into `dist/` yet, styled with the shared
/// [`adi_css`] design system.
fn placeholder_html() -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>adi-app</title>{style}</head>\
         <body><div class=\"adi-container\">\
         <h1>adi-app is running</h1>\
         <p class=\"adi-muted\">The web UI hasn't been built yet. Build it with:</p>\
         <pre class=\"adi-mono\">scripts/build-app.sh</pre>\
         <p class=\"adi-muted\">or run <code>trunk build</code> in <code>crates/adi-webapp</code>, \
         then <code>cargo build -p adi-app</code>.</p>\
         </div></body></html>",
        style = adi_css::style_tag(),
    )
}

/// Map a file name to a `Content-Type` by its extension; unknown types are served as bytes.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        // The PWA manifest: browsers accept `application/json` but only this type is spec'd.
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

    /// What may be shared and what may not. The distinction is the whole safety argument for
    /// coalescing: a read repeated is the same read, a mutation repeated is a second mutation.
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

    /// Two pollers watching *different* chats must not be handed each other's answer, so the body
    /// that names the subject is part of the key — and a body too large to key on opts out.
    #[test]
    fn the_key_separates_subjects_and_bounds_itself() {
        let one = shared_read_key(&request("POST", "/api/agents/runs", r#"{"name":"a"}"#));
        let other = shared_read_key(&request("POST", "/api/agents/runs", r#"{"name":"b"}"#));
        assert!(one.is_some() && one != other, "different agents, different keys");

        // A query is part of the subject, not decoration on the route: `?limit=100` and
        // `?limit=200` are two different pages of the session index, and sharing them would hand
        // one page's asker the other's answer.
        let page = shared_read_key(&request("GET", "/api/agents/runs/all?limit=100", ""));
        assert!(page.is_some(), "the route is still the one on the allowlist");
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

    /// The point of the coalescer: concurrent askers cost one read between them, and every one of
    /// them gets that read's answer.
    #[tokio::test]
    async fn concurrent_readers_share_one_answer() {
        let reads = Reads::default();
        let runs = Arc::new(AtomicUsize::new(0));

        let compute = |runs: Arc<AtomicUsize>| {
            move || {
                let nth = runs.fetch_add(1, Ordering::SeqCst);
                // Long enough that the followers below are certain to join while it is in flight.
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

    /// Sharing is per-question: two different reads in flight at once must not collapse into one.
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

    /// A panicking handler must not take the followers with it: everyone waiting on that read gets
    /// the same 500, and the slot is released so the next poll tries again.
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
