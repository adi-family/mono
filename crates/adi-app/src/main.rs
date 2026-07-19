//! adi-app — the adi application server behind `app.adi`: one process serving the
//! control-panel webapp at `GET /` and a JSON `/api/*` backend over [`adi_ports_manager`].
//! Listens on `$PORT` or an explicit `addr` argument, on loopback.
//!
//! The UI is the Leptos app [`adi-webapp`](../adi-webapp), compiled to wasm by Trunk. Its
//! `dist/` output is embedded here at build time; set `ADI_WEBAPP_DIST=/path/to/dist` to
//! serve those files from disk instead (a dev mode — rebuild the UI with `trunk build` and
//! refresh, no re-embed). The API handlers live in [`adi_webapp_api::handlers`] and share
//! their DTO types with that frontend.

mod http;
mod scan;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use adi_agents::Agents;
use adi_mesh::Daemon;
use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_tasks::Tasks;
use adi_triggers::{Supervisor, Triggers};
use adi_webapp_api::handlers;
use adi_webapp_api::handlers::Response;
use include_dir::{Dir, include_dir};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

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
            *slot = Some(Daemon::start().await?);
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

/// The webapp's Trunk build output, embedded so the binary is self-contained. Empty until
/// `trunk build` runs in `crates/adi-webapp`; [`serve_asset`] serves a placeholder when
/// `index.html` is absent.
static WEBAPP: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../adi-webapp/dist");

/// Service identity reported at `/api/health`.
const SERVICE: &str = "adi-app";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fallback listen port when `$PORT` is unset and no `addr` argument is given.
const DEFAULT_PORT: u16 = 8090;

/// Env var pointing at a webapp `dist/` to serve from disk instead of the embedded copy.
const DIST_ENV: &str = "ADI_WEBAPP_DIST";

/// How long to wait for background triggers to exit at shutdown before giving up on them — a
/// code block that ignores SIGTERM must not hold the whole app open.
const TRIGGER_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

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
    let ports = Arc::new(Ports::new());
    let projects = Arc::new(Projects::open());
    let tasks = Arc::new(Tasks::open());
    let agents = Arc::new(Agents::open());
    let triggers = Arc::new(Triggers::open());
    // Background triggers are long-lived processes owned by this app: the supervisor keeps
    // every enabled one running for as long as the app is up, and stops them on the way out.
    let trigger_supervisor = Supervisor::start((*triggers).clone());
    let webapp_dist = Arc::new(webapp_dist_override());
    // The mesh daemon runs in-process, so it lives only as long as this app. Autostart it
    // (non-blocking, best-effort) so the whole stack is up once the app is — the control
    // panel's Stop button still stops it for the session.
    let mesh = Arc::new(MeshCtl::default());
    {
        let mesh = Arc::clone(&mesh);
        tokio::spawn(async move {
            match mesh.start().await {
                Ok(()) => info!("mesh autostarted"),
                Err(e) => warn!(error = %e, "mesh autostart failed"),
            }
        });
    }
    let start = Instant::now();
    info!(%local, registry = %ports.config().registry_path.display(), "adi-app listening");
    if let Some(dir) = webapp_dist.as_ref() {
        info!(dist = %dir.display(), "serving webapp from disk (dev mode)");
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let ports = Arc::clone(&ports);
                    let projects = Arc::clone(&projects);
                    let tasks = Arc::clone(&tasks);
                    let agents = Arc::clone(&agents);
                    let triggers = Arc::clone(&triggers);
                    let trigger_supervisor = Arc::clone(&trigger_supervisor);
                    let webapp_dist = Arc::clone(&webapp_dist);
                    let mesh = Arc::clone(&mesh);
                    tokio::spawn(async move {
                        if let Err(e) = handle(
                            stream,
                            &ports,
                            &projects,
                            &tasks,
                            &agents,
                            &triggers,
                            &trigger_supervisor,
                            &mesh,
                            start,
                            webapp_dist.as_deref(),
                        )
                        .await
                        {
                            debug!(%peer, error = %e, "connection error");
                        }
                    });
                }
                Err(e) => warn!(error = %e, "accept failed"),
            },
            () = shutdown_signal() => {
                info!("shutdown signal received; stopping");
                break;
            }
        }
    }
    // Background triggers run in their own process groups so the supervisor can signal their
    // whole tree — which also means they outlive this process unless they are stopped first.
    // Waiting here is what keeps a restart from leaking a copy of every background trigger.
    trigger_supervisor.stop(TRIGGER_STOP_GRACE).await;
    mesh.stop().await;
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
// The dispatcher threads each store handle through by reference; grouping them into a context
// struct would be churn for no gain in a single hand-rolled router.
#[allow(clippy::too_many_arguments)]
async fn handle(
    mut stream: TcpStream,
    ports: &Ports,
    projects: &Projects,
    tasks: &Tasks,
    agents: &Agents,
    triggers: &Triggers,
    trigger_supervisor: &Supervisor,
    mesh: &MeshCtl,
    start: Instant,
    dist: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };
    debug!(method = %req.method, path = %req.path, "request");

    let path = req.route_path();
    let Response { status, body } = match (req.method.as_str(), path) {
        ("GET", "/api/health") => handlers::health(SERVICE, VERSION, start),
        ("GET", "/api/ports") => handlers::ports(ports),
        ("GET", "/api/ports/used") => handlers::used_ports(scan::listening_ports()),
        ("POST", "/api/ports/reserve") => handlers::reserve(ports, &req.body),
        ("POST", "/api/ports/release") => handlers::release(ports, &req.body),
        ("GET", "/api/projects") => handlers::projects(projects),
        ("POST", "/api/projects/create") => handlers::create_project(projects, &req.body),
        ("POST", "/api/projects/archive") => handlers::archive_project(projects, &req.body),
        ("POST", "/api/projects/unarchive") => handlers::unarchive_project(projects, &req.body),
        ("POST", "/api/projects/remove") => handlers::remove_project(projects, &req.body),
        ("POST", "/api/projects/files") => handlers::list_files(projects, &req.body),
        ("POST", "/api/projects/file/read") => handlers::read_file(projects, &req.body),
        ("POST", "/api/projects/file/write") => handlers::write_file(projects, &req.body),
        // The store browser: the whole ~/.adi/mono tree, jailed to it (see handlers::fs).
        ("POST", "/api/fs/list") => handlers::fs_list(projects, &req.body),
        ("POST", "/api/fs/read") => handlers::fs_read(projects, &req.body),
        ("POST", "/api/fs/write") => handlers::fs_write(projects, &req.body),
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
        ("POST", "/api/projects/hook/create") => {
            handlers::create_project_hook(projects, &req.body)
        }
        // A single project's detail (manifest + its .adi/hive.yaml services). The id is the
        // trailing path segment; the exact routes above (all POST, or the bare GET) win first.
        ("GET", p) if p.starts_with("/api/projects/") => {
            let listening: Vec<u16> = scan::listening_ports()
                .into_iter()
                .map(|u| u.port)
                .collect();
            handlers::project_detail(projects, &p["/api/projects/".len()..], &listening)
        }
        ("GET", "/api/tasks") => handlers::tasks(tasks),
        ("POST", "/api/tasks/create") => handlers::create_task(tasks, &req.body),
        ("GET", "/api/agents") => handlers::agents(agents),
        ("POST", "/api/agents/save") => handlers::save_agent(agents, &req.body),
        ("POST", "/api/agents/delete") => handlers::delete_agent(agents, &req.body),
        ("POST", "/api/agents/run") => handlers::run_agent(agents, &req.body),
        ("POST", "/api/agents/stop") => handlers::stop_agent(agents, &req.body),
        ("POST", "/api/agents/code") => handlers::agent_code(agents, &req.body),
        ("POST", "/api/agents/code/save") => handlers::save_agent_code(agents, &req.body),
        ("POST", "/api/agents/build") => handlers::build_agent(agents, &req.body),
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
        // The public webhook endpoint: fire an enabled `webhook` trigger with the request body
        // as its payload. GET is accepted too — some webhook providers ping with it. The secret
        // (when the trigger requires one) rides in the query, which route_path() strips.
        (m, p) if p.starts_with("/api/hooks/") && matches!(m, "POST" | "GET") => {
            let name = &p["/api/hooks/".len()..];
            let query = req.path.split_once('?').map_or("", |(_, q)| q);
            handlers::hook_trigger(triggers, name, query, &req.body)
        }
        ("GET", "/api/hive") => {
            let listening: Vec<u16> = scan::listening_ports()
                .into_iter()
                .map(|u| u.port)
                .collect();
            handlers::hive(projects, ports, &listening)
        }
        ("GET", "/api/dashboards") => {
            let listening: Vec<u16> = scan::listening_ports()
                .into_iter()
                .map(|u| u.port)
                .collect();
            handlers::dashboards(projects.config(), ports, &listening)
        }
        ("POST", "/api/dashboards/create") => {
            handlers::create_dashboard(projects.config(), ports, &req.body)
        }
        ("POST", "/api/hive/start") => handlers::start_service(projects, &req.body),
        ("POST", "/api/hive/stop") => handlers::stop_service(projects, &req.body),
        ("POST", "/api/hive/create") => {
            let listening: Vec<u16> = scan::listening_ports()
                .into_iter()
                .map(|u| u.port)
                .collect();
            handlers::create_service(projects, &req.body, &listening)
        }
        ("GET", "/api/mesh") => handlers::mesh(mesh.running().await),
        ("POST", "/api/mesh/start") => mesh_start(mesh).await,
        ("POST", "/api/mesh/stop") => mesh_stop(mesh).await,
        ("POST", "/api/mesh/allow") => handlers::mesh_allow(mesh.running().await, &req.body),
        ("POST", "/api/mesh/deny") => handlers::mesh_deny(mesh.running().await, &req.body),
        ("POST", "/api/mesh/peers/allow") => {
            handlers::mesh_allow_peer(mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/peers/deny") => {
            handlers::mesh_deny_peer(mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/forwards/add") => {
            handlers::mesh_add_forward(mesh.running().await, &req.body)
        }
        ("POST", "/api/mesh/forwards/remove") => {
            handlers::mesh_remove_forward(mesh.running().await, &req.body)
        }
        (_, p) if p.starts_with("/api") => handlers::error(404, "no such API endpoint"),
        // Any other GET serves a webapp asset, or the app shell for client-side routing.
        ("GET", p) => return serve_asset(&mut stream, p, dist).await,
        _ => handlers::error(405, "method not allowed"),
    };
    http::write_json(&mut stream, status, &body).await
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
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return futures_pending().await;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Never resolves — keeps the accept loop alive if the SIGTERM handler can't be installed.
#[cfg(unix)]
async fn futures_pending() {
    std::future::pending::<()>().await;
}
