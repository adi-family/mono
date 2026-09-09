use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use adi_projects::Projects;
use serde::Deserialize;

use crate::types::{
    HiveService, HiveState, NewService, ProcessUsage, ProjectService, ServicePort, ServiceState,
    StartResult, StartService, StopResult, UsedPort,
};

use super::projects::project_detail;
use super::response::{Response, error, ok_json};

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
    /// `on-demand` (the default for a service with a `proxy.host`) or `always` — adi-hive's
    /// `StartPolicy`, mirrored as the raw string so this view reports what the file says rather
    /// than a second opinion about it.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    idle_stop: Option<String>,
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
    #[serde(default)]
    docker: Option<HiveDocker>,
}

#[derive(Deserialize)]
struct HiveScript {
    run: String,
    #[serde(default)]
    working_dir: Option<String>,
}

/// The docker runner, mirrored just enough for this crate's needs — adi-hive owns the full schema.
/// `image` is surfaced as the service's displayed command; `name` is the container to start/stop
/// (the start/stop endpoints act on the container by name, never on a script or the host port).
#[derive(Deserialize)]
struct HiveDocker {
    image: String,
    #[serde(default)]
    name: Option<String>,
}

impl HiveRunner {
    /// The command to show for this runner: a script's shell command, or `docker: <image>` for a
    /// container. `docker` wins when both are present (as it does in adi-hive's `runners()`).
    fn display_command(self) -> Option<String> {
        if let Some(docker) = self.docker {
            return Some(format!("docker: {}", docker.image));
        }
        self.script.map(|s| s.run)
    }
}

/// Whether `port` is one the host's scan found listening.
pub(crate) fn is_listening(live: &[UsedPort], port: u16) -> bool {
    live.iter().any(|u| u.port == port)
}

/// The value of `start:` that means "started by a visit, stopped when nobody visits" (adi-hive's
/// `StartPolicy::OnDemand`) — and, for a service with a `proxy.host`, what saying nothing means.
const ON_DEMAND: &str = "on-demand";

/// The other policy: started with the hive and kept alive. What `start:` has to say for a service
/// that must be up whether or not anybody is looking at it.
const ALWAYS: &str = "always";

/// Where a hive publishes what its on-demand services are doing, beside its config (adi-hive's
/// `DEMAND_FILE`). Only a hive that actually supervises one writes it.
const DEMAND_FILE: &str = "demand.json";

/// What the hive daemon says its on-demand services are doing, keyed exactly as the hive keys them:
/// `<project>/<service>` for an imported service, the bare name for one declared in the front-door
/// hive itself.
///
/// It exists because a port scan cannot tell *starting* from *stopped*, and those are the two
/// states an on-demand service spends its interesting moments in. Absent (no hive here supervises
/// anything on demand, or it is not running) the file simply isn't there, and every service falls
/// back to what the scan alone can say.
#[derive(Debug, Default)]
pub(crate) struct DemandPhases(BTreeMap<String, String>);

impl DemandPhases {
    /// Read the file, or nothing at all — a missing or unreadable report is never an error here.
    pub(crate) fn read(cfg: &adi_config::Config) -> Self {
        let path = cfg.module("hive").raw_path(DEMAND_FILE);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BTreeMap<String, String>>(&raw).ok())
            .map_or_else(Self::default, Self)
    }

    /// The phase published for a service, under the key its hive uses.
    fn phase(&self, namespace: Option<&str>, name: &str) -> Option<&str> {
        let key = namespace.map_or_else(|| name.to_string(), |ns| format!("{ns}/{name}"));
        self.0.get(&key).map(String::as_str)
    }
}

/// Resolve a service's live state from the three things that can be known about it: whether its
/// port answers, whether it is on demand, and what its hive last published.
///
/// The port scan outranks the report — a service that is answering is running, whoever started it
/// and whatever anyone says. The report is only consulted for the states a scan cannot see, and a
/// published "running" we cannot reach reads as `starting`: the hive has a process, it is just not
/// answering yet.
fn service_state(running: bool, on_demand: bool, published: Option<&str>) -> ServiceState {
    if running {
        return ServiceState::Running;
    }
    match published {
        Some("starting" | "running") => ServiceState::Starting,
        Some("idle-stopped") => ServiceState::IdleStopped,
        // Nothing published: an on-demand service that is not up is waiting to be visited, which is
        // the policy working rather than a service that is down.
        _ if on_demand => ServiceState::IdleStopped,
        _ => ServiceState::Stopped,
    }
}

/// Whether a `start:` value *names* the on-demand policy, in the spellings adi-hive accepts.
/// Anything else it does not recognise — a typo included — is `always` there, and so here.
fn names_on_demand(start: &str) -> bool {
    matches!(
        start.trim().to_ascii_lowercase().as_str(),
        "on-demand" | "on_demand" | "ondemand" | "demand" | "lazy"
    )
}

/// Whether a service is on demand, read the way adi-hive reads it: what its `start:` says, and —
/// when it says nothing — the default for its shape. `has_host` is whether it declares a
/// `proxy.host`, because a service with none can never be woken by a request and is therefore
/// `always` (adi-hive's `StartPolicy::default_for`).
fn is_on_demand(start: Option<&str>, has_host: bool) -> bool {
    given(start).map_or(has_host, names_on_demand)
}

/// What the process holding `port` costs, when the host sampled it.
fn usage_at(live: &[UsedPort], port: Option<u16>) -> Option<ProcessUsage> {
    let port = port?;
    live.iter().find(|u| u.port == port)?.usage.clone()
}

/// Read a project's `.adi/hive.yaml` into `(has_hive, services)`, tagging each service with a live
/// running flag (its primary port is one of the `live` listeners), that listener's CPU/memory, and
/// its start policy and state. A missing file is `(false, [])`; a present-but-unparseable file is
/// `(true, [])` — the project has a hive config, just not one we can summarize.
///
/// `namespace` is how the supervising hive keys these services (a project or dashboard id, `None`
/// for the front-door hive's own), which is what the on-demand phases in `demand` are looked up by.
pub(crate) fn read_hive_services(
    path: &Path,
    live: &[UsedPort],
    demand: &DemandPhases,
    namespace: Option<&str>,
) -> (bool, Vec<ProjectService>) {
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
            let port = primary_port(&ports);
            let running = port.is_some_and(|p| is_listening(live, p));
            let host = svc.proxy.map(|p| p.host);
            let on_demand = is_on_demand(
                svc.start.as_deref(),
                host.as_deref().is_some_and(|h| !h.trim().is_empty()),
            );
            ProjectService {
                host,
                run: svc.runner.and_then(HiveRunner::display_command),
                restart: svc.restart,
                // Normalised so a panel never has to guess: a file that says nothing about `start`
                // is reported under the policy it is actually running, not as a blank.
                start: Some(if on_demand { ON_DEMAND } else { ALWAYS }.to_string()),
                idle_stop: svc.idle_stop,
                running,
                state: service_state(running, on_demand, demand.phase(namespace, &name)),
                usage: usage_at(live, port),
                ports,
                name,
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
/// plus the global `~/.adi/mono/hive/hive.yaml`, tagged with a live running flag and what the
/// running ones cost. `live` is the machine's listening TCP ports with their sampled process
/// usage (the host does the platform scan and passes it).
#[must_use]
pub fn hive(store: &Projects, ports: &adi_ports_manager::Ports, live: &[UsedPort]) -> Response {
    let mut services = Vec::new();
    // Read once for the whole aggregate: it is one small file, and every service in every project
    // is looked up in it.
    let demand = DemandPhases::read(store.config());

    // The global front-door hive lives in the `hive` module of the same store the projects use.
    let global = store.config().module("hive").raw_path("hive.yaml");
    collect_hive_services(None, &global, live, &demand, &mut services);

    match store.list() {
        Ok(projects) => {
            for project in projects {
                if let Ok(path) = store.hive_path(&project.id) {
                    collect_hive_services(Some(&project.id), &path, live, &demand, &mut services);
                }
            }
        }
        Err(e) => return Response::from(&e),
    }

    // Dashboards are supervised by their own (per-user) adi-hive, but they are hive services all
    // the same — this view is meant to be the one place every service is visible.
    collect_dashboard_services(store.config(), ports, live, &demand, &mut services);

    ok_json(&HiveState { services })
}

/// Append every dashboard's services, tagged with its dashboard id.
///
/// Dashboards declare no ports in their `hive.yaml` — adi-hive leases one per service from the
/// ports manager, keyed `<dashboard-id>/<service>` — so unlike the project path, the ports are
/// resolved from that registry rather than read out of the YAML.
fn collect_dashboard_services(
    cfg: &adi_config::Config,
    ports: &adi_ports_manager::Ports,
    live: &[UsedPort],
    demand: &DemandPhases,
    out: &mut Vec<HiveService>,
) {
    let root = cfg.module("dashboards").dir().to_path_buf();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let Some(id) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let (_has_hive, parsed) =
            read_hive_services(&dir.join(".adi").join("hive.yaml"), live, demand, Some(&id));
        for svc in parsed {
            let port = ports
                .get(&format!("{id}/{}", svc.name), "http")
                .ok()
                .flatten();
            let running = port.is_some_and(|p| is_listening(live, p));
            // `read_hive_services` already settled the policy for this service, so this reads its
            // answer rather than defaulting a second time off a field it has since normalised.
            let on_demand = svc.start.as_deref() == Some(ON_DEMAND);
            out.push(HiveService {
                project: None,
                dashboard: Some(id.clone()),
                host: svc.host,
                ports: port
                    .map(|p| {
                        vec![ServicePort {
                            key: "http".to_string(),
                            port: p,
                        }]
                    })
                    .unwrap_or_default(),
                run: svc.run,
                restart: svc.restart,
                start: svc.start,
                idle_stop: svc.idle_stop,
                primary_port: port,
                running,
                // Decided on the leased port, not the YAML's — see the port above.
                state: service_state(running, on_demand, demand.phase(Some(&id), &svc.name)),
                // A dashboard's port comes from the registry, not its YAML, so `read_hive_services`
                // could not have charged it — resolve the usage against the leased port here.
                usage: usage_at(live, port),
                name: svc.name,
            });
        }
    }
}

/// Parse one hive.yaml and append its services to `out`, tagged with `project`, a running flag
/// (its primary port is one of the `live` listeners), and that listener's usage.
fn collect_hive_services(
    project: Option<&str>,
    path: &Path,
    live: &[UsedPort],
    demand: &DemandPhases,
    out: &mut Vec<HiveService>,
) {
    let (_has_hive, parsed) = read_hive_services(path, live, demand, project);
    for svc in parsed {
        let port = primary_port(&svc.ports);
        out.push(HiveService {
            project: project.map(str::to_string),
            dashboard: None,
            name: svc.name,
            host: svc.host,
            ports: svc.ports,
            run: svc.run,
            restart: svc.restart,
            start: svc.start,
            idle_stop: svc.idle_stop,
            primary_port: port,
            running: svc.running,
            state: svc.state,
            usage: svc.usage,
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
    let Some(runner) = svc.runner.as_ref() else {
        return error(
            400,
            &format!("service `{}` has no runner to start", req.service),
        );
    };
    // Exactly one runner kind is allowed — the same rule adi-hive enforces. A service declaring
    // both is ambiguous, so refuse rather than guess (adi-hive would skip it too).
    if runner.docker.is_some() && runner.script.is_some() {
        return error(
            400,
            &format!(
                "service `{}` declares both a `docker` and a `script` runner — declare exactly one",
                req.service
            ),
        );
    }

    // A docker runner: start just its container (`docker start <name>`), never a script and never
    // the host port.
    if let Some(docker) = runner.docker.as_ref() {
        let name = docker_container_name(&req.service, docker);
        return match docker_container("start", &name) {
            // The container runs inside the docker daemon, so there is no local pid to report.
            Ok(()) => ok_json(&StartResult {
                service: req.service,
                port: service_http_port(svc),
                pid: 0,
            }),
            Err(e) => error(500, &format!("starting container `{name}`: {e}")),
        };
    }

    let Some(script) = runner.script.as_ref() else {
        return error(
            400,
            &format!(
                "service `{}` has no script or docker runner to start",
                req.service
            ),
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
    // The same one-kind rule as start (and adi-hive): a service declaring both runner kinds is
    // ambiguous, so refuse — which also keeps such a service from ever reaching `kill_listener`.
    if let Some(runner) = svc.runner.as_ref()
        && runner.docker.is_some()
        && runner.script.is_some()
    {
        return error(
            400,
            &format!(
                "service `{}` declares both a `docker` and a `script` runner — declare exactly one",
                req.service
            ),
        );
    }

    // A docker runner: stop just its container (`docker stop <name>`). NEVER kill the port's
    // listener — for a published container that listener is Docker's own host proxy, so a
    // `kill` on it takes down the Docker daemon (and every other container) instead of this one.
    if let Some(docker) = svc.runner.as_ref().and_then(|r| r.docker.as_ref()) {
        let name = docker_container_name(&req.service, docker);
        return match docker_container("stop", &name) {
            Ok(()) => ok_json(&StopResult {
                service: req.service,
                port: service_http_port(svc),
            }),
            Err(e) => error(500, &format!("stopping container `{name}`: {e}")),
        };
    }

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
pub fn create_service(store: &Projects, body: &[u8], live: &[UsedPort]) -> Response {
    use serde_yaml_ng::{Mapping, Value as Yaml};

    fn ystr(s: &str) -> Yaml {
        Yaml::String(s.to_string())
    }

    /// A YAML sequence of strings, dropping blank entries.
    fn yseq(items: &[String]) -> Yaml {
        Yaml::Sequence(
            items
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(ystr)
                .collect(),
        )
    }

    let Ok(req) = serde_json::from_slice::<NewService>(body) else {
        return error(
            400,
            "expected JSON body { project, name, run|docker, host?, port?, working_dir?, \
             restart?, start?, idle_stop?, stop_grace? }",
        );
    };
    if let Some(refusal) = refuse_bad_start_policy(&req) {
        return refusal;
    }
    let project = req.project.trim();
    let name = req.name.trim();
    let run = req.run.trim();
    if !valid_service_name(name) {
        return error(
            400,
            "a service name is letters, digits, `.`, `-`, `_` (and not `.`/`..`)",
        );
    }
    // A service is one runner kind: a docker container (when `docker` is set) or a script.
    let docker = req.docker.as_ref();
    if let Some(d) = docker {
        if given(Some(&d.image)).is_none() {
            return error(400, "a docker image is required");
        }
    } else if run.is_empty() {
        return error(400, "a run command is required (or provide a docker image)");
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
        Err(_) => {
            return error(
                422,
                "could not parse the existing hive.yaml — fix it in the project's files first",
            );
        }
    };

    let mut svc = Mapping::new();
    if let Some(host) = given(req.host.as_deref()) {
        let mut proxy = Mapping::new();
        proxy.insert(ystr("host"), ystr(host));
        svc.insert(ystr("proxy"), Yaml::Mapping(proxy));
    }
    let http_port = match req.port {
        Some(p) => Yaml::Number(p.into()),
        None => {
            ystr(&commands.placeholder(&format!("ports-manager.get('{project}/{name}', 'http')")))
        }
    };
    let mut ports = Mapping::new();
    ports.insert(ystr("http"), http_port);
    let mut recreate = Mapping::new();
    recreate.insert(ystr("ports"), Yaml::Mapping(ports));
    let mut rollout = Mapping::new();
    rollout.insert(ystr("recreate"), Yaml::Mapping(recreate));
    svc.insert(ystr("rollout"), Yaml::Mapping(rollout));
    let mut runner = Mapping::new();
    if let Some(d) = docker {
        // A docker runner: `runner.docker` with the leased host `http` port mapped to the
        // container port. adi-hive compiles this to a supervised `docker run` (see its config.rs).
        let mut dk = Mapping::new();
        dk.insert(ystr("image"), ystr(d.image.trim()));
        if let Some(cp) = d.container_port {
            let mut dports = Mapping::new();
            dports.insert(ystr("http"), Yaml::Number(cp.into()));
            dk.insert(ystr("ports"), Yaml::Mapping(dports));
        }
        if let Some(pull) = given(d.pull.as_deref()) {
            dk.insert(ystr("pull"), ystr(pull));
        }
        if !d.environment.is_empty() {
            let mut env = Mapping::new();
            for (k, v) in &d.environment {
                env.insert(ystr(k.trim()), ystr(v));
            }
            dk.insert(ystr("environment"), Yaml::Mapping(env));
        }
        let volumes = yseq(&d.volumes);
        if volumes.as_sequence().is_some_and(|s| !s.is_empty()) {
            dk.insert(ystr("volumes"), volumes);
        }
        let args = yseq(&d.args);
        if args.as_sequence().is_some_and(|s| !s.is_empty()) {
            dk.insert(ystr("args"), args);
        }
        let command = yseq(&d.command);
        if command.as_sequence().is_some_and(|s| !s.is_empty()) {
            dk.insert(ystr("command"), command);
        }
        runner.insert(ystr("docker"), Yaml::Mapping(dk));
    } else {
        let mut script = Mapping::new();
        script.insert(ystr("run"), ystr(run));
        if let Some(dir) = given(req.working_dir.as_deref()) {
            script.insert(ystr("working_dir"), ystr(dir));
        }
        runner.insert(ystr("script"), Yaml::Mapping(script));
    }
    svc.insert(ystr("runner"), Yaml::Mapping(runner));
    if let Some(restart) = given(req.restart.as_deref()) {
        svc.insert(ystr("restart"), ystr(restart));
    }
    for (key, value) in start_policy_keys(&req) {
        svc.insert(ystr(key), ystr(&value));
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
        return error(
            409,
            &format!("service `{name}` already exists in this project"),
        );
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
    project_detail(store, project, live)
}

/// A trimmed, non-empty optional field.
fn given(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|s| !s.is_empty())
}

/// Refuse a start policy that would not mean what it says, before anything reaches the file.
///
/// An unreadable window is refused rather than written: a service that quietly ignores the half
/// hour somebody asked for and takes the daemon's hour instead is worse than one that was not
/// created, because nothing about it looks wrong afterwards.
fn refuse_bad_start_policy(req: &NewService) -> Option<Response> {
    if let Some(policy) = given(req.start.as_deref())
        && !names_on_demand(policy)
        && !policy.eq_ignore_ascii_case(ALWAYS)
    {
        return Some(error(400, "start must be `always` or `on-demand`"));
    }
    for (field, value) in [
        ("idle_stop", req.idle_stop.as_deref()),
        ("stop_grace", req.stop_grace.as_deref()),
    ] {
        if let Some(raw) = given(value)
            && adi_config::parse_duration(raw).is_none()
        {
            return Some(error(
                400,
                &format!("{field} must be a duration like `1h`, `30m`, `90s`, or seconds"),
            ));
        }
    }
    None
}

/// The `start` / `idle_stop` / `stop_grace` keys a new service should carry, in file order.
///
/// Only what was actually asked for: a service created without an opinion about starting carries no
/// key at all and takes adi-hive's default — on-demand once it has a host, `always` without one —
/// which is what keeps the create form from writing a policy into every hive.yaml it touches. The
/// two windows are on-demand's alone; on an `always` service they would describe a stop that never
/// happens.
fn start_policy_keys(req: &NewService) -> Vec<(&'static str, String)> {
    let Some(policy) = given(req.start.as_deref()) else {
        return Vec::new();
    };
    if !names_on_demand(policy) {
        return vec![("start", policy.to_string())];
    }
    let mut keys = vec![("start", ON_DEMAND.to_string())];
    if let Some(idle) = given(req.idle_stop.as_deref()) {
        keys.push(("idle_stop", idle.to_string()));
    }
    if let Some(grace) = given(req.stop_grace.as_deref()) {
        keys.push(("stop_grace", grace.to_string()));
    }
    keys
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

/// The container name for a docker service — an explicit `docker.name`, else `adi-<service>` with
/// characters Docker forbids (like the `/` in a project-scoped name) mapped to `-`. Mirrors
/// adi-hive's own `container_name`, so a container started here and one the hive supervisor runs
/// resolve to the same name.
fn docker_container_name(service: &str, docker: &HiveDocker) -> String {
    if let Some(name) = docker
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return name.to_string();
    }
    let mut out = String::from("adi-");
    out.extend(service.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            c
        } else {
            '-'
        }
    }));
    out
}

/// Whether `name` is a valid Docker container name — a leading alphanumeric then `[A-Za-z0-9_.-]`.
/// Also the safety gate before the name is spliced into the `docker …` shell command: a name that
/// passes holds no shell metacharacters, so it can't break out of the command.
fn valid_container_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Run `docker <action> <name>` (start / stop) on a single container, under an augmented `PATH` so
/// `docker` resolves in launchd's minimal environment (same pattern as [`kill_listener`]). The
/// name is charset-validated first, so acting on one container never touches Docker itself or any
/// other container.
fn docker_container(action: &str, name: &str) -> Result<(), String> {
    if !valid_container_name(name) {
        return Err(format!("unsafe container name `{name}`"));
    }
    let status = std::process::Command::new("sh")
        .arg("-c")
        // `name` is charset-validated above, so it carries no shell metacharacters.
        .arg(format!("docker {action} {name}"))
        .env("PATH", adi_config::augmented_path())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`docker {action} {name}` exited {status}"))
    }
}

/// SIGTERM whatever process is listening on `port` (best-effort, via `lsof` + `kill`).
fn kill_listener(port: u16) -> std::io::Result<()> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "pids=$(lsof -ti tcp:{port} -sTCP:LISTEN 2>/dev/null); [ -n \"$pids\" ] && kill $pids || true"
        ))
        .env("PATH", adi_config::augmented_path())
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
        .stdin(Stdio::null());
    // Detached so the runner survives an app restart (own process group / group-equivalent).
    adi_osext::detach_process_group(&mut cmd);
    // Env parity (shared in adi-config): augmented PATH (+ HOME) so a runner finds bun/node/docker
    // under launchd's bare environment — the same env adi-hive's supervisor applies.
    for (key, value) in adi_config::launch_env() {
        cmd.env(key, value);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn docker(name: Option<&str>) -> HiveDocker {
        HiveDocker {
            image: "pgvector/pgvector:pg16".to_string(),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn container_name_prefers_explicit_then_falls_back_to_sanitized_service() {
        // An explicit name is used verbatim (the DB's real container name here).
        assert_eq!(
            docker_container_name("db", &docker(Some("nosh-guide-postgres"))),
            "nosh-guide-postgres"
        );
        // Blank explicit name → fall back to the default.
        assert_eq!(docker_container_name("db", &docker(Some("  "))), "adi-db");
        // No name → `adi-<service>`, with unsafe characters mapped to `-` (matches adi-hive).
        assert_eq!(docker_container_name("db", &docker(None)), "adi-db");
        assert_eq!(
            docker_container_name("proj/api", &docker(None)),
            "adi-proj-api"
        );
    }

    /// The distinction the four states exist for: "nobody has asked for it" is not "it is down",
    /// and a port that answers outranks anything a report can say.
    #[test]
    fn an_on_demand_service_that_is_down_reads_as_idle_stopped_not_stopped() {
        assert_eq!(service_state(true, true, None), ServiceState::Running);
        assert_eq!(
            service_state(true, true, Some("idle-stopped")),
            ServiceState::Running,
            "the port scan outranks a stale report"
        );
        assert_eq!(
            service_state(false, true, None),
            ServiceState::IdleStopped,
            "an on-demand service that is down is waiting, not broken"
        );
        assert_eq!(service_state(false, false, None), ServiceState::Stopped);
        assert_eq!(
            service_state(false, true, Some("starting")),
            ServiceState::Starting
        );
        assert_eq!(
            service_state(false, true, Some("running")),
            ServiceState::Starting,
            "the hive has a process, but nothing is answering yet"
        );
    }

    /// A service created without an opinion about starting must carry no `start:` at all — the
    /// default lives in adi-hive, and writing it into every file would be a second place for it to
    /// be changed.
    #[test]
    fn only_the_start_keys_that_were_asked_for_are_written() {
        let base = NewService {
            project: "p".to_string(),
            name: "watch".to_string(),
            run: "bun run watch".to_string(),
            host: None,
            port: None,
            working_dir: None,
            restart: None,
            start: None,
            idle_stop: None,
            stop_grace: None,
            docker: None,
        };
        assert!(start_policy_keys(&base).is_empty(), "no opinion, no key");

        let demanded = NewService {
            start: Some("on-demand".to_string()),
            idle_stop: Some("30m".to_string()),
            stop_grace: Some("  ".to_string()),
            ..base.clone()
        };
        assert_eq!(
            start_policy_keys(&demanded),
            vec![
                ("start", "on-demand".to_string()),
                ("idle_stop", "30m".to_string()),
            ],
            "a blank window is not a window"
        );

        // The windows belong to on-demand; on an `always` service they describe nothing.
        let always = NewService {
            start: Some("always".to_string()),
            idle_stop: Some("30m".to_string()),
            ..base.clone()
        };
        assert_eq!(
            start_policy_keys(&always),
            vec![("start", "always".to_string())]
        );

        // And what cannot be honoured is refused rather than written and quietly defaulted.
        assert!(refuse_bad_start_policy(&base).is_none());
        assert!(
            refuse_bad_start_policy(&NewService {
                start: Some("whenever".to_string()),
                ..base.clone()
            })
            .is_some()
        );
        assert!(
            refuse_bad_start_policy(&NewService {
                start: Some("on-demand".to_string()),
                idle_stop: Some("soon".to_string()),
                ..base
            })
            .is_some()
        );
    }

    #[test]
    fn the_on_demand_policy_is_read_the_way_adi_hive_reads_it() {
        let routable = true;
        assert!(is_on_demand(Some("on-demand"), routable));
        assert!(is_on_demand(Some(" On-Demand "), routable));
        assert!(is_on_demand(Some("on_demand"), routable));
        assert!(!is_on_demand(Some("always"), routable));
        assert!(
            !is_on_demand(Some("on demand"), routable),
            "a typo is not the policy, and does not fall through to the default either"
        );

        // Nothing said: the default, which turns on whether anything could ever wake it.
        assert!(is_on_demand(None, true));
        assert!(!is_on_demand(None, false), "no host, nothing to arrive at");
        assert!(is_on_demand(Some("  "), true), "blank is not an opinion");
    }

    /// What `GET /api/hive` and the project detail page report for each of the three cases: a
    /// service that asked for the policy, a routable one that said nothing (the default), and one
    /// with no host, which nothing could wake and which therefore keeps starting itself.
    #[test]
    fn a_services_start_policy_is_read_out_of_its_hive_yaml() {
        let dir = std::env::temp_dir().join(format!(
            "adi-api-hive-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hive.yaml");
        std::fs::write(
            &path,
            "services:\n  \
               watch:\n    \
                 proxy: { host: watch.adi }\n    \
                 rollout: { recreate: { ports: { http: 8931 } } }\n    \
                 start: on-demand\n    \
                 idle_stop: 30m\n    \
                 runner: { script: { run: 'bun run watch' } }\n  \
               web:\n    \
                 proxy: { host: web.adi }\n    \
                 rollout: { recreate: { ports: { http: 8933 } } }\n    \
                 runner: { script: { run: 'bun run web' } }\n  \
               api:\n    \
                 rollout: { recreate: { ports: { http: 8932 } } }\n    \
                 runner: { script: { run: 'bun run api' } }\n",
        )
        .unwrap();

        let (has_hive, services) =
            read_hive_services(&path, &[], &DemandPhases::default(), Some("proj"));
        assert!(has_hive);
        let watch = services.iter().find(|s| s.name == "watch").expect("watch");
        assert_eq!(watch.start.as_deref(), Some("on-demand"));
        assert_eq!(watch.idle_stop.as_deref(), Some("30m"));
        assert_eq!(watch.state, ServiceState::IdleStopped);

        // A routable service that says nothing takes the default, and is reported as waiting rather
        // than as down — it is not running because nobody has asked for it.
        let web = services.iter().find(|s| s.name == "web").expect("web");
        assert_eq!(web.start.as_deref(), Some("on-demand"));
        assert_eq!(web.state, ServiceState::IdleStopped);
        assert_eq!(web.idle_stop, None, "the window is the daemon's, not a key");

        // No host, so no request could ever wake it: `always`, and down means down.
        let api = services.iter().find(|s| s.name == "api").expect("api");
        assert_eq!(api.start.as_deref(), Some("always"));
        assert_eq!(api.state, ServiceState::Stopped);
        assert_eq!(api.idle_stop, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The phases are keyed the way the hive keys its services, so a project's service is found
    /// under `<project>/<service>` and a front-door one under its bare name.
    #[test]
    fn published_phases_are_looked_up_under_the_hives_own_key() {
        let phases = DemandPhases(BTreeMap::from([
            ("proj/watch".to_string(), "starting".to_string()),
            ("frontend".to_string(), "idle-stopped".to_string()),
        ]));
        assert_eq!(phases.phase(Some("proj"), "watch"), Some("starting"));
        assert_eq!(phases.phase(None, "frontend"), Some("idle-stopped"));
        assert_eq!(phases.phase(None, "watch"), None);
        assert_eq!(phases.phase(Some("other"), "watch"), None);
    }

    #[test]
    fn valid_container_name_gates_shell_metacharacters() {
        for ok in ["adi-db", "nosh-guide-postgres", "a.b_c-1", "A1"] {
            assert!(valid_container_name(ok), "{ok} should be valid");
        }
        // A leading non-alphanumeric, or anything that could break out of the shell command.
        for bad in ["", "-x", ".x", "a b", "a;rm", "a$(x)", "a|b", "a`b`", "a/b"] {
            assert!(!valid_container_name(bad), "{bad:?} should be rejected");
        }
    }
}
