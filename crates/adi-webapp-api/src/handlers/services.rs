use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use adi_projects::Projects;
use serde::Deserialize;

use crate::types::{HiveService, HiveState, NewService, ProjectService, ServicePort, StartResult, StartService, StopResult};

use super::response::{error, ok_json, Response};
use super::projects::project_detail;

// A read-only view of the subset of adi-hive's `hive.yaml` schema the detail page shows.
// adi-hive owns the authoritative schema (`crates/adi-hive/src/config.rs`); it's a binary
// crate with no lib target, so this mirrors just the fields surfaced here rather than
// pulling its full dependency tree (tokio, …) into this library. Unknown fields are ignored.
#[derive(Deserialize)]
struct HiveDoc {
    #[serde(default)]
    services: BTreeMap<String, YamlService>,
}

#[derive(Deserialize)]
struct YamlService {
    #[serde(default)]
    proxy: Option<HiveProxy>,
    #[serde(default)]
    rollout: Option<HiveRollout>,
    #[serde(default)]
    runner: Option<HiveRunner>,
    #[serde(default)]
    restart: Option<String>,
}

#[derive(Deserialize)]
struct HiveProxy {
    host: String,
}

#[derive(Deserialize)]
struct HiveRollout {
    #[serde(default)]
    recreate: Option<HiveRecreate>,
}

#[derive(Deserialize)]
struct HiveRecreate {
    /// Values may be literals or `` bash`ports-manager.get('name')` `` commands (preprocessed into
    /// `datacommand:<hash>` placeholders), executed to reserve ports when this view reads the
    /// config (see `adi_ports_manager::preprocess` / `ports_map`).
    #[serde(default, deserialize_with = "adi_ports_manager::ports_map")]
    ports: BTreeMap<String, u16>,
}

#[derive(Deserialize)]
struct HiveRunner {
    #[serde(default)]
    script: Option<HiveScript>,
}

#[derive(Deserialize)]
struct HiveScript {
    run: String,
    #[serde(default)]
    working_dir: Option<String>,
}

/// Read a project's `.adi/hive.yaml` into `(has_hive, services)`, tagging each service with a live
/// running flag (its primary port is in `listening`). A missing file is `(false, [])`; a
/// present-but-unparseable file is `(true, [])` — the project has a hive config, just not one we
/// can summarize.
pub(crate) fn read_hive_services(path: &Path, listening: &[u16]) -> (bool, Vec<ProjectService>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (false, Vec::new());
    };
    // Rewrite `bash`…`` port commands into valid YAML, then parse with the command table
    // installed so port fields resolve and run their commands on read.
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return (true, Vec::new());
    };
    let services = doc
        .services
        .into_iter()
        .map(|(name, svc)| {
            let ports: Vec<ServicePort> = svc
                .rollout
                .and_then(|r| r.recreate)
                .map(|r| {
                    r.ports
                        .into_iter()
                        .map(|(key, port)| ServicePort { key, port })
                        .collect()
                })
                .unwrap_or_default();
            let running = primary_port(&ports).is_some_and(|p| listening.contains(&p));
            ProjectService {
                name,
                host: svc.proxy.map(|p| p.host),
                ports,
                run: svc.runner.and_then(|r| r.script).map(|s| s.run),
                restart: svc.restart,
                running,
            }
        })
        .collect();
    (true, services)
}

// MARK: files — a project's own directory, browsed/edited through an isolated jail

/// The port key that names a service's HTTP port (mirrors adi-hive's `HTTP_PORT_KEY`).
const HTTP_PORT_KEY: &str = "http";

/// The port a service's liveness is judged on: the `http` port, else the sole port, else `None`.
fn primary_port(ports: &[ServicePort]) -> Option<u16> {
    if let Some(p) = ports.iter().find(|p| p.key == HTTP_PORT_KEY) {
        return Some(p.port);
    }
    match ports {
        [only] => Some(only.port),
        _ => None,
    }
}

/// `GET /api/hive` — aggregate every service declared across all projects' `.adi/hive.yaml`
/// plus the global `~/.adi/mono/hive/hive.yaml`, tagged with a live running flag. `listening`
/// is the set of currently-listening TCP ports (the host does the platform scan and passes it).
#[must_use]
pub fn hive(store: &Projects, listening: &[u16]) -> Response {
    let mut services = Vec::new();

    // The global front-door hive lives in the `hive` module of the same store the projects use.
    let global = store.config().module("hive").raw_path("hive.yaml");
    collect_hive_services(None, &global, listening, &mut services);

    match store.list() {
        Ok(projects) => {
            for project in projects {
                if let Ok(path) = store.hive_path(&project.id) {
                    collect_hive_services(Some(&project.id), &path, listening, &mut services);
                }
            }
        }
        Err(e) => return Response::from(&e),
    }

    ok_json(&HiveState { services })
}

/// Parse one hive.yaml and append its services to `out`, tagged with `project` and a running
/// flag (its primary port is in `listening`).
fn collect_hive_services(
    project: Option<&str>,
    path: &Path,
    listening: &[u16],
    out: &mut Vec<HiveService>,
) {
    let (_has_hive, parsed) = read_hive_services(path, listening);
    for svc in parsed {
        let port = primary_port(&svc.ports);
        let running = port.is_some_and(|p| listening.contains(&p));
        out.push(HiveService {
            project: project.map(str::to_string),
            name: svc.name,
            host: svc.host,
            ports: svc.ports,
            run: svc.run,
            restart: svc.restart,
            primary_port: port,
            running,
        });
    }
}

/// `POST /api/hive/start` — launch a hive service's runner (its `run` command) with the
/// ports-manager-allocated `PORT` injected, in its working directory. The child is detached (its
/// own process group) and its output goes to `<workdir>/server.log`; status then reflects the
/// service's primary port listening.
#[must_use]
pub fn start_service(store: &Projects, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<StartService>(body) else {
        return error(400, "expected JSON body { project?, service }");
    };

    let (hive_path, default_dir) = match &req.project {
        Some(id) => match (store.hive_path(id), store.project_dir(id)) {
            (Ok(hive), Ok(dir)) => (hive, Some(dir)),
            (Err(e), _) | (_, Err(e)) => return Response::from(&e),
        },
        None => (store.config().module("hive").raw_path("hive.yaml"), None),
    };

    let Ok(raw) = std::fs::read_to_string(&hive_path) else {
        return error(404, "no hive.yaml for that target");
    };
    // Preprocess so `bash`…`` port commands resolve (and reserve their port) on read.
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return error(422, "could not parse the hive.yaml");
    };

    let Some(svc) = doc.services.get(&req.service) else {
        return error(404, &format!("no service `{}` in the hive", req.service));
    };
    let Some(script) = svc.runner.as_ref().and_then(|r| r.script.as_ref()) else {
        return error(
            400,
            &format!("service `{}` has no script runner to start", req.service),
        );
    };

    let port = service_http_port(svc);
    // Don't spawn a doomed process if the port is already taken — the service looks up already.
    if let Some(p) = port
        && !adi_ports_manager::is_bindable(p)
    {
        return error(
            409,
            &format!("service `{}` looks already running on :{p}", req.service),
        );
    }
    let workdir = resolve_workdir(script.working_dir.as_deref(), default_dir.as_deref());
    match spawn_runner(&script.run, &workdir, port) {
        Ok(pid) => ok_json(&StartResult {
            service: req.service,
            port,
            pid,
        }),
        Err(e) => error(500, &format!("starting `{}`: {e}", req.service)),
    }
}

/// `POST /api/hive/stop` {project?, service} — stop a running service by killing whatever listens
/// on its resolved port (the runner was spawned in its own process group; a plain kill on the
/// listener stops it).
#[must_use]
pub fn stop_service(store: &Projects, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<StartService>(body) else {
        return error(400, "expected JSON body { project?, service }");
    };
    let hive_path = match &req.project {
        Some(id) => match store.hive_path(id) {
            Ok(hive) => hive,
            Err(e) => return Response::from(&e),
        },
        None => store.config().module("hive").raw_path("hive.yaml"),
    };
    let Ok(raw) = std::fs::read_to_string(&hive_path) else {
        return error(404, "no hive.yaml for that target");
    };
    let (yaml, commands) = adi_ports_manager::preprocess(&raw);
    let Ok(doc) =
        adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str::<HiveDoc>(&yaml))
    else {
        return error(422, "could not parse the hive.yaml");
    };
    let Some(svc) = doc.services.get(&req.service) else {
        return error(404, &format!("no service `{}` in the hive", req.service));
    };
    let Some(port) = service_http_port(svc) else {
        return error(
            400,
            &format!("service `{}` has no port to stop", req.service),
        );
    };
    match kill_listener(port) {
        Ok(()) => ok_json(&StopResult {
            service: req.service,
            port: Some(port),
        }),
        Err(e) => error(500, &format!("stopping `{}`: {e}", req.service)),
    }
}

/// `POST /api/hive/create` — add a service to a project's `.adi/hive.yaml` (creating the file
/// if needed), then return the fresh [`ProjectDetail`]. The existing YAML is patched as a
/// value tree so fields this view doesn't model survive, and `` bash`…` `` port commands
/// round-trip through their preprocessed placeholders (comments don't survive a rewrite).
/// Without an explicit `port`, the `http` port is written as a
/// `` ports-manager.get('<project>/<name>', 'http') `` command — the same lease the hive
/// daemon resolves for the imported service, so both sides agree on the port.
#[must_use]
pub fn create_service(store: &Projects, body: &[u8], listening: &[u16]) -> Response {
    use serde_yaml_ng::{Mapping, Value as Yaml};

    fn ystr(s: &str) -> Yaml {
        Yaml::String(s.to_string())
    }
    /// A trimmed, non-empty optional field.
    fn given(field: Option<&str>) -> Option<&str> {
        field.map(str::trim).filter(|s| !s.is_empty())
    }

    let Ok(req) = serde_json::from_slice::<NewService>(body) else {
        return error(
            400,
            "expected JSON body { project, name, run, host?, port?, working_dir?, restart? }",
        );
    };
    let project = req.project.trim();
    let name = req.name.trim();
    let run = req.run.trim();
    if !valid_service_name(name) {
        return error(
            400,
            "a service name is letters, digits, `.`, `-`, `_` (and not `.`/`..`)",
        );
    }
    if run.is_empty() {
        return error(400, "a run command is required");
    }
    match store.get(project) {
        Ok(Some(_)) => {}
        Ok(None) => return error(404, &format!("no such project: {project}")),
        Err(e) => return Response::from(&e),
    }
    let path = match store.hive_path(project) {
        Ok(path) => path,
        Err(e) => return Response::from(&e),
    };

    // Load the existing document (a missing/empty file is an empty mapping). A file we can't
    // parse is refused rather than clobbered — it can be fixed in the project's Files tab.
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let (yaml, mut commands) = adi_ports_manager::preprocess(&raw);
    let mut doc = match serde_yaml_ng::from_str::<Yaml>(&yaml) {
        Ok(Yaml::Null) => Yaml::Mapping(Mapping::new()),
        Ok(doc @ Yaml::Mapping(_)) => doc,
        Ok(_) => return error(422, "the existing hive.yaml is not a YAML mapping"),
        Err(_) => return error(
            422,
            "could not parse the existing hive.yaml — fix it in the project's files first",
        ),
    };

    let mut svc = Mapping::new();
    if let Some(host) = given(req.host.as_deref()) {
        let mut proxy = Mapping::new();
        proxy.insert(ystr("host"), ystr(host));
        svc.insert(ystr("proxy"), Yaml::Mapping(proxy));
    }
    let http_port = match req.port {
        Some(p) => Yaml::Number(p.into()),
        None => ystr(&commands.placeholder(&format!("ports-manager.get('{project}/{name}', 'http')"))),
    };
    let mut ports = Mapping::new();
    ports.insert(ystr("http"), http_port);
    let mut recreate = Mapping::new();
    recreate.insert(ystr("ports"), Yaml::Mapping(ports));
    let mut rollout = Mapping::new();
    rollout.insert(ystr("recreate"), Yaml::Mapping(recreate));
    svc.insert(ystr("rollout"), Yaml::Mapping(rollout));
    let mut script = Mapping::new();
    script.insert(ystr("run"), ystr(run));
    if let Some(dir) = given(req.working_dir.as_deref()) {
        script.insert(ystr("working_dir"), ystr(dir));
    }
    let mut runner = Mapping::new();
    runner.insert(ystr("script"), Yaml::Mapping(script));
    svc.insert(ystr("runner"), Yaml::Mapping(runner));
    if let Some(restart) = given(req.restart.as_deref()) {
        svc.insert(ystr("restart"), ystr(restart));
    }

    let Yaml::Mapping(root) = &mut doc else {
        unreachable!("doc was matched to a mapping above");
    };
    let services = root
        .entry(ystr("services"))
        .or_insert_with(|| Yaml::Mapping(Mapping::new()));
    let Yaml::Mapping(services) = services else {
        return error(422, "the existing hive.yaml `services` is not a mapping");
    };
    let key = ystr(name);
    if services.contains_key(&key) {
        return error(409, &format!("service `{name}` already exists in this project"));
    }
    services.insert(key, Yaml::Mapping(svc));

    let text = match serde_yaml_ng::to_string(&doc) {
        Ok(text) => commands.restore(&text),
        Err(e) => return error(500, &format!("re-serializing hive.yaml: {e}")),
    };
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        return error(500, &format!("creating {}: {e}", dir.display()));
    }
    if let Err(e) = std::fs::write(&path, text) {
        return error(500, &format!("writing {}: {e}", path.display()));
    }
    project_detail(store, project, listening)
}

/// Validate a service name: a single YAML key that is also safe as a ports-manager lease
/// segment and a filesystem-adjacent token — mirrors the trigger-name rule.
fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// SIGTERM whatever process is listening on `port` (best-effort, via `lsof` + `kill`).
fn kill_listener(port: u16) -> std::io::Result<()> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "pids=$(lsof -ti tcp:{port} -sTCP:LISTEN 2>/dev/null); [ -n \"$pids\" ] && kill $pids || true"
        ))
        .env("PATH", augmented_path())
        .status()?;
    Ok(())
}

/// The service's proxied port: the `http` slot, else the sole declared port, else `None`.
fn service_http_port(svc: &YamlService) -> Option<u16> {
    let ports = svc
        .rollout
        .as_ref()
        .and_then(|r| r.recreate.as_ref())
        .map(|r| &r.ports)?;
    ports
        .get("http")
        .copied()
        .or_else(|| (ports.len() == 1).then(|| *ports.values().next().unwrap()))
}

/// Resolve a runner's working directory: an explicit absolute path as-is, a relative one against
/// the project dir, else the project dir (or the current dir when there is none).
fn resolve_workdir(explicit: Option<&str>, default_dir: Option<&Path>) -> PathBuf {
    match explicit {
        Some(dir) => {
            let p = Path::new(dir);
            if p.is_absolute() {
                p.to_path_buf()
            } else if let Some(base) = default_dir {
                base.join(dir)
            } else {
                p.to_path_buf()
            }
        }
        None => default_dir.map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    }
}

/// Spawn `sh -c "<run>"` detached (its own process group, so it survives an app restart), in
/// `workdir`, with `PORT`/`PORT_HTTP` and an augmented `PATH` so user tools (bun, node, …)
/// resolve under a minimal launchd environment. Output is redirected to `<workdir>/server.log`.
fn spawn_runner(run: &str, workdir: &Path, port: Option<u16>) -> std::io::Result<u32> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(run)
        .current_dir(workdir)
        .env("PATH", augmented_path())
        .process_group(0)
        .stdin(Stdio::null());
    if let Some(p) = port {
        cmd.env("PORT", p.to_string())
            .env("PORT_HTTP", p.to_string());
    }
    match std::fs::File::create(workdir.join("server.log")) {
        Ok(log) => {
            let errlog = log.try_clone()?;
            cmd.stdout(Stdio::from(log)).stderr(Stdio::from(errlog));
        }
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    Ok(cmd.spawn()?.id())
}

/// A `PATH` that includes the user's common tool directories, so a runner launched under a
/// minimal launchd environment can still find `bun`, `node`, and Homebrew binaries.
fn augmented_path() -> String {
    let mut parts = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.local/bin"));
    }
    parts.push("/opt/homebrew/bin".to_string());
    parts.push("/usr/local/bin".to_string());
    parts.push("/usr/bin".to_string());
    parts.push("/bin".to_string());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}
