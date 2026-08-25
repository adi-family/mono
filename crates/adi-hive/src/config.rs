//! The single hive config, loaded from `~/.adi/mono/hive/hive.yaml`: the reverse-proxy
//! fields and the fields needed to run a service locally. Unknown hive.yaml fields are
//! accepted-but-ignored (no `deny_unknown_fields`).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use adi_ports_manager::Ports;
use anyhow::Context as _;
use serde::Deserialize;
use tracing::warn;

/// The store module the hive config lives under, and the raw config file within it.
const HIVE_MODULE: &str = "hive";
const HIVE_CONFIG_FILE: &str = "hive.yaml";

/// Upstreams are always local — a service's HTTP port on loopback.
const UPSTREAM_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The port-map key that names a service's HTTP port (what the proxy targets).
const HTTP_PORT_KEY: &str = "http";

/// The ports-manager lease for adi-hive's own front-door port (when no explicit `proxy.bind`).
const FRONT_DOOR_NAME: &str = "adi-hive";
const FRONT_DOOR_KEY: &str = "front-door";

/// The reserved namespace for remote nodes: `<service>.<node>.n.adi` (docs/fleet.md §1). Nothing
/// local may claim a name inside it, which is exactly what guarantees a remote name can never
/// collide with a local `<service>.adi` — the two zones are disjoint by construction.
pub const MESH_ZONE: &str = "n.adi";

/// The suffix form of [`MESH_ZONE`], so a host *inside* the zone is a plain suffix test.
const MESH_SUFFIX: &str = ".n.adi";

// MARK: parsed hive.yaml (proxy-relevant subset)

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Hive {
    /// Glob patterns (e.g. `$ADI_PROJECTS_DIR/**/hive.yaml`) whose matched hive.yaml files are
    /// fanned in as proxy routes, so this hive is the single front door for every project.
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub proxy: ProxyBinds,
    #[serde(default)]
    pub services: BTreeMap<String, ServiceSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProxyBinds {
    #[serde(default)]
    pub bind: Vec<SocketAddr>,
    /// HTTPS front-door binds. Routed exactly like [`Self::bind`], but TLS-terminated with a
    /// locally-trusted certificate the daemon mints itself (see [`crate::tls`]). Empty means no
    /// HTTPS — plain HTTP keeps working either way, so adding this never takes the front door away.
    ///
    /// This is what makes `https://app.adi` a *secure context*, and so installable as an app: a
    /// service worker is refused over `http://` on any hostname but `localhost`.
    #[serde(default)]
    pub tls_bind: Vec<SocketAddr>,
    /// Optional front-door name; the ports-manager lease key when the bind port is manager-allocated.
    #[serde(default)]
    pub name: Option<String>,
    /// Route imported services, never launch them — say it, rather than let it be inferred.
    ///
    /// Two hives read the same imports: the **front door**, which must only route, and the
    /// per-user **supervisor**, which must actually run things. Until now the difference was
    /// guessed from the effective uid — root means front door — which held only because on macOS
    /// the front door is a root daemon. On a Linux node it runs unprivileged (there is no root
    /// daemon by design), so the guess inverted: the front door kept every imported runner and
    /// launched a second copy of every dashboard, racing the supervisor for the same leased
    /// ports. A config key cannot be wrong about which instance it is.
    ///
    /// `true` here forces route-only; running as root still implies it, so existing configs are
    /// unaffected.
    #[serde(default)]
    pub routes_only: bool,
    /// Loopback address of the local **mesh gateway**. Set it and every `*.n.adi` host is forwarded
    /// there verbatim instead of being matched against the service routes — one rule for the whole
    /// fleet, because the gateway (not the front door) is what knows how to turn a hostname into a
    /// peer key. Unset means this machine has no mesh, and such a host gets the
    /// [mesh-unavailable page](crate::notfound::mesh_unavailable) rather than a misleading 404.
    #[serde(default)]
    pub mesh_gateway: Option<SocketAddr>,
    /// Petnames of the paired nodes whose `*.<node>.n.adi` names the front door's TLS leaf should
    /// cover. Only TLS reads this — routing never needs it, since one gateway rule covers every
    /// node. It exists because a wildcard label matches exactly one level, so `<service>.<node>.n.adi`
    /// needs a *per-node* wildcard in the certificate (see [`crate::tls`]); pairing appends the
    /// petname here so the next start mints a leaf that covers it.
    ///
    /// An entry may be dotted (`nosh.laptop-b`) to cover a service name deeper than one label —
    /// the same wildcard limit applies one level further down. See [`crate::tls`]'s `mesh_sans`.
    #[serde(default)]
    pub mesh_nodes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceSpec {
    #[serde(default)]
    pub proxy: Option<ServiceProxy>,
    #[serde(default)]
    pub rollout: Option<Rollout>,
    /// How to run the service locally — a `script` (a shell command) or a `docker` container.
    /// A runner with neither parses but is skipped (nothing to launch).
    #[serde(default)]
    pub runner: Option<Runner>,
    /// Extra environment for the runner (merged after the injected `PORT*` vars).
    #[serde(default)]
    pub environment: Option<Environment>,
    /// Restart policy: `always` | `on-failure` | `no`. Defaults to `on-failure`.
    #[serde(default)]
    pub restart: Option<String>,
    /// The directory this service's runner resolves relative paths against (`working_dir`, docker
    /// bind-mount host paths). `None` for a service declared in the hive being loaded (it uses the
    /// loader's `base_dir`); set to the imported file's own directory for an imported service — so an
    /// imported project's `working_dir: workspaces/main` resolves under *that* project, not under the
    /// importer (e.g. the per-user supervisor's own dir). Never serialized.
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceProxy {
    pub host: String,
    /// An optional path prefix this service claims **on** [`Self::host`], so several services can
    /// share one hostname: a dashboard's `frontend` takes the host, its `backend` takes
    /// `path: /api`. That is what lets a dashboard be *one origin* — the page uses relative URLs
    /// only and therefore works unchanged under `nosh.adi`, `nosh.laptop-b.n.adi` or a real
    /// customer domain (docs/fleet.md §4).
    ///
    /// Longest prefix wins; a service with no path is the host's fallback. `/` means the same as
    /// no path at all — it is the fallback either way, which is why the many configs written with
    /// `path: /` keep behaving exactly as before.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rollout {
    #[serde(default)]
    pub recreate: Option<Recreate>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Recreate {
    /// Named ports. Each value is a literal integer or a `` bash`ports-manager.get('name')` ``
    /// command (rewritten by the loader's preprocessor into a `datacommand:<hash>` placeholder),
    /// executed to reserve a port when the config is read.
    #[serde(default, deserialize_with = "adi_ports_manager::ports_map")]
    pub ports: BTreeMap<String, u16>,
}

/// The `runner:` block: exactly one kind — a `script` (a shell command) or a `docker`
/// container. Declaring **both** is a config error: [`Hive::runners`] refuses to launch it (it is
/// skipped, with a warning) rather than guess which was meant. A block with neither is likewise
/// skipped (there is nothing to launch).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Runner {
    #[serde(default)]
    pub script: Option<Script>,
    /// Run the service as a Docker container instead of a host process. Compiled to a
    /// foreground `docker run` command the ordinary supervisor drives — so restart, backoff,
    /// hot-reload, and shutdown work identically to a script runner. See [`Docker`].
    #[serde(default)]
    pub docker: Option<Docker>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    /// The shell command to run (executed via `sh -c`).
    pub run: String,
    /// Where to run it, relative to the hive.yaml's directory (or absolute); defaults to that directory.
    #[serde(default)]
    pub working_dir: Option<String>,
}

/// A container runner — an "irregular Docker Compose" service: one container, declared with the
/// familiar compose-ish keys, but supervised by adi-hive rather than by `docker compose`. It
/// compiles (see [`Docker::command_line`]) to an **attach-to-existing** command: `docker start`
/// the named container (a running one is a no-op — never a restart), create it only if it doesn't
/// exist, then `docker wait` it so adi-hive supervises the container's lifetime without owning it.
/// A supervisor restart leaves the container running (stop it with `docker stop <name>`); a
/// container that exits on its own is relaunched per the restart policy.
///
/// Host ports stay adi-hive's job: the service's `rollout.recreate.ports` are the (leased) host
/// ports, and `ports` here maps each of those **port keys** to the container port it targets —
/// published on loopback (`127.0.0.1:<host>:<container>`) so the container is reachable only
/// through the front door, exactly like a script runner listening on `127.0.0.1`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Docker {
    /// The image to run, e.g. `nginx:1.27` (required).
    pub image: String,
    /// Override the image's default command / entrypoint args (appended after the image).
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Map each of the service's host **port keys** (from `rollout.recreate.ports`) to the
    /// container port it forwards to — e.g. `{ http: 8080 }` publishes the leased `http` host
    /// port to the container's `8080`. A key with no matching host port is skipped. The
    /// container also receives the usual `PORT` / `PORT_<KEY>` env (the *container* ports), so a
    /// `$PORT`-aware image works whether it runs as a script or a container.
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
    /// Bind mounts, `host:container[:mode]` (compose syntax). A relative or `./`-prefixed host
    /// path is resolved against the hive.yaml's directory; an absolute path and a named volume
    /// (no path separator) are passed through untouched.
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Extra environment for the container (passed as `-e KEY=VALUE`). Merged over the service's
    /// `environment.static`, which the container also receives.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Image pull policy (`always` | `missing` | `never`) → `docker run --pull <policy>`.
    #[serde(default)]
    pub pull: Option<String>,
    /// Raw extra flags spliced into the `docker run` invocation before the image — the escape
    /// hatch for anything not modelled first-class (`--memory=512m`, `-w /app`, `--network host`,
    /// `--user 1000`, `--gpus all`, …). Each entry is passed as one argument.
    #[serde(default)]
    pub args: Vec<String>,
    /// Override the container name. Defaults to `adi-<service>` (with unsafe characters, like the
    /// `/` in a project-scoped `proj/app`, mapped to `-`).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Environment {
    #[serde(default, rename = "static")]
    pub static_env: BTreeMap<String, String>,
}

impl ServiceSpec {
    /// The port the proxy forwards to: the `http` port, else the sole port, else `None`.
    fn http_port(&self) -> Option<u16> {
        let ports = self.ports();
        if let Some(port) = ports.get(HTTP_PORT_KEY) {
            return Some(*port);
        }
        if ports.len() == 1 {
            return ports.values().next().copied();
        }
        None
    }

    /// This service's declared port map (`rollout.recreate.ports`), or empty.
    fn ports(&self) -> &BTreeMap<String, u16> {
        static EMPTY: BTreeMap<String, u16> = BTreeMap::new();
        self.rollout
            .as_ref()
            .and_then(|r| r.recreate.as_ref())
            .map_or(&EMPTY, |r| &r.ports)
    }

    /// Set the service's `http` port, creating the `rollout.recreate.ports` path if needed.
    fn set_http_port(&mut self, port: u16) {
        self.rollout
            .get_or_insert_with(Rollout::default)
            .recreate
            .get_or_insert_with(Recreate::default)
            .ports
            .insert(HTTP_PORT_KEY.to_string(), port);
    }

    /// A service the proxy or the runner needs a port for.
    fn needs_http_port(&self) -> bool {
        self.proxy.is_some()
            || self
                .runner
                .as_ref()
                .is_some_and(|r| r.script.is_some() || r.docker.is_some())
    }
}

// MARK: runners — from the parsed spec to a launchable process

/// What to do when a runner process exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Always relaunch (after a backoff), whatever the exit status.
    Always,
    /// Relaunch only on a non-zero exit; a clean exit is left stopped.
    OnFailure,
    /// Never relaunch.
    Never,
}

impl RestartPolicy {
    fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("always") => Self::Always,
            Some("no" | "never" | "false") => Self::Never,
            _ => Self::OnFailure,
        }
    }
}

/// A service resolved to a launchable runner: command, working dir, env, and restart policy.
///
/// `PartialEq` is what makes hot reload safe: the supervisor compares a freshly-read spec against
/// the running one and only restarts a service whose definition actually changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSpec {
    pub name: String,
    pub run: String,
    pub working_dir: PathBuf,
    pub env: Vec<(String, String)>,
    pub restart: RestartPolicy,
}

impl Hive {
    /// For each proxied/script-runner service without an HTTP port, reserve a stable one from the ports manager and fill it in; returns the `(service, port)` pairs allocated.
    pub fn allocate_missing_ports(&mut self, manager: &Ports) -> Vec<(String, u16)> {
        let mut allocated = Vec::new();
        for (name, svc) in &mut self.services {
            if !svc.needs_http_port() || svc.http_port().is_some() {
                continue;
            }
            match manager.reserve(name, HTTP_PORT_KEY) {
                Ok(port) => {
                    svc.set_http_port(port);
                    allocated.push((name.clone(), port));
                }
                Err(e) => warn!(service = %name, error = %e, "could not allocate a port"),
            }
        }
        allocated
    }

    /// Reserve adi-hive's own front-door bind port from the ports manager when no explicit `proxy.bind` is set; returns the reserved port, if any.
    pub fn allocate_bind_port(&mut self, manager: &Ports) -> Option<u16> {
        if !self.proxy.bind.is_empty() {
            return None;
        }
        let name = self.proxy.name.as_deref().unwrap_or(FRONT_DOOR_NAME);
        match manager.reserve(name, FRONT_DOOR_KEY) {
            Ok(port) => {
                self.proxy.bind = vec![SocketAddr::new(UPSTREAM_IP, port)];
                Some(port)
            }
            Err(e) => {
                warn!(error = %e, "could not allocate a front-door port; using the default");
                None
            }
        }
    }

    /// Every service that declares a launchable runner (a `script` or a `docker` container),
    /// resolved for launch; `base_dir` anchors relative `working_dir`s and bind-mount host paths.
    /// A runner block with neither kind is skipped — and so is one that declares **both** (that is
    /// ambiguous; exactly one kind is required), with a warning, rather than quietly guessing.
    #[must_use]
    pub fn runners(&self, base_dir: &Path) -> Vec<RunnerSpec> {
        let mut out = Vec::new();
        for (name, svc) in &self.services {
            let Some(runner) = svc.runner.as_ref() else {
                continue;
            };
            let ports = svc.ports();
            let restart = RestartPolicy::parse(svc.restart.as_deref());
            // An imported service resolves relative paths against its own file's directory; a service
            // declared in this hive uses the loader's `base_dir`.
            let dir = svc.base_dir.as_deref().unwrap_or(base_dir);
            let spec = match (runner.docker.as_ref(), runner.script.as_ref()) {
                // Ambiguous: exactly one runner kind is allowed. Refuse to launch either, so a stray
                // second runner can't silently shadow the intended one — surface it and skip.
                (Some(_), Some(_)) => {
                    warn!(service = %name,
                          "runner declares both `docker` and `script`; declare exactly one — skipping");
                    continue;
                }
                // A container runner compiles to one foreground `docker run` command the ordinary
                // supervisor drives; all container state lives in `run`, so the env is empty (the
                // container's env is baked into the command's `-e` flags).
                (Some(docker), None) => RunnerSpec {
                    name: name.clone(),
                    run: docker.command_line(name, svc, ports, dir),
                    working_dir: dir.to_path_buf(),
                    env: Vec::new(),
                    restart,
                },
                (None, Some(script)) => RunnerSpec {
                    name: name.clone(),
                    run: expand_templates(&script.run, ports),
                    working_dir: resolve_working_dir(dir, script.working_dir.as_deref()),
                    env: build_env(svc, ports),
                    restart,
                },
                (None, None) => continue,
            };
            out.push(spec);
        }
        out
    }
}

/// Build the runner's env: `PORT` (http/sole port), a `PORT_<KEY>` per named port, then static env last.
fn build_env(svc: &ServiceSpec, ports: &BTreeMap<String, u16>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(port) = svc.http_port() {
        env.push(("PORT".to_string(), port.to_string()));
    }
    for (key, port) in ports {
        env.push((
            format!("PORT_{}", key.to_ascii_uppercase()),
            port.to_string(),
        ));
    }
    if let Some(environment) = &svc.environment {
        for (key, value) in &environment.static_env {
            env.push((key.clone(), expand_templates(value, ports)));
        }
    }
    env
}

/// Resolve a runner's working directory against `base_dir` (absolute as-is, `None` → `base_dir`).
fn resolve_working_dir(base_dir: &Path, dir: Option<&str>) -> PathBuf {
    match dir {
        Some(dir) => {
            let p = Path::new(dir);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                base_dir.join(p)
            }
        }
        None => base_dir.to_path_buf(),
    }
}

impl Docker {
    /// Compile this container runner into the single shell command adi-hive's supervisor runs.
    ///
    /// Shape: `docker start <name> … || docker run -d --name <name> [flags] <image> [command]; exec
    /// docker wait <name>`.
    ///
    /// It **attaches to an existing container** rather than recreating one:
    /// - `docker start <name>` reuses the container as-is (a running one is a no-op — **no restart**);
    ///   the `|| docker run -d …` create path fires only when the container doesn't exist yet.
    /// - `exec docker wait <name>` then makes *this* the supervised foreground process for the
    ///   container's whole life. A supervisor `SIGTERM` kills the `wait`, **not the container**, so the
    ///   container survives a supervisor restart untouched — to actually stop it, `docker stop <name>`.
    ///   If the container exits on its own, `wait` returns and the supervisor's restart policy relaunches
    ///   this, which `docker start`s it again.
    /// - Every interpolated value is shell-quoted, so image names, env values, and paths with spaces
    ///   or metacharacters can't break out of the command.
    ///
    /// Note: the create-time flags (ports, volumes, env, healthcheck) apply only when the container is
    /// first created; changing them later takes effect after the container is removed and recreated.
    ///
    /// `host_ports` are the service's leased host ports (`svc.ports()`); `base_dir` anchors relative
    /// bind-mount host paths.
    fn command_line(
        &self,
        service: &str,
        svc: &ServiceSpec,
        host_ports: &BTreeMap<String, u16>,
        base_dir: &Path,
    ) -> String {
        let name = container_name(self.name.as_deref(), service);
        // The `docker run` that *creates* the container (first launch only) — detached (`-d`) so it is
        // not `--rm`'d on exit and outlives a supervisor restart.
        let mut create: Vec<String> = ["docker", "run", "-d", "--name"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        create.push(shell_quote(&name));

        if let Some(pull) = self.pull.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            create.push("--pull".to_string());
            create.push(shell_quote(pull));
        }

        // Publish each mapped host port key to its container port, on loopback so the container is
        // reachable only through the front door (as a script runner on 127.0.0.1 would be).
        for (key, container_port) in &self.ports {
            if let Some(host_port) = host_ports.get(key) {
                create.push("-p".to_string());
                create.push(format!("127.0.0.1:{host_port}:{container_port}"));
            }
        }

        for (key, value) in self.container_env(svc) {
            create.push("-e".to_string());
            create.push(shell_quote(&format!("{key}={value}")));
        }

        for volume in &self.volumes {
            create.push("-v".to_string());
            create.push(shell_quote(&resolve_volume(base_dir, volume)));
        }

        // Raw passthrough flags — the escape hatch for anything not modelled first-class.
        for arg in &self.args {
            create.push(shell_quote(arg));
        }

        create.push(shell_quote(&self.image));
        for arg in self.command.iter().flatten() {
            create.push(shell_quote(arg));
        }

        let name = shell_quote(&name);
        format!(
            "docker start {name} >/dev/null 2>&1 || {create}; exec docker wait {name}",
            create = create.join(" "),
        )
    }

    /// The environment handed to the container, in stable (sorted) order: the `PORT` / `PORT_<KEY>`
    /// convention pointing at the *container* ports, then the service's `environment.static`, then
    /// this block's own `environment` — later entries win, so an explicit value overrides a
    /// convention default.
    fn container_env(&self, svc: &ServiceSpec) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        for (key, container_port) in &self.ports {
            env.insert(
                format!("PORT_{}", key.to_ascii_uppercase()),
                container_port.to_string(),
            );
            if key == HTTP_PORT_KEY {
                env.insert("PORT".to_string(), container_port.to_string());
            }
        }
        if let Some(environment) = &svc.environment {
            for (key, value) in &environment.static_env {
                env.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in &self.environment {
            env.insert(key.clone(), value.clone());
        }
        env
    }
}

/// The container name for a service: an explicit override, else `adi-<service>` with characters a
/// Docker name can't hold (notably the `/` in a project-scoped `proj/app`) mapped to `-`. The
/// `adi-` prefix guarantees the required leading alphanumeric.
fn container_name(explicit: Option<&str>, service: &str) -> String {
    if let Some(name) = explicit.map(str::trim).filter(|n| !n.is_empty()) {
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

/// Resolve the host side of a `host:container[:mode]` bind mount against `base_dir`: a relative
/// path (starts with `.` or contains a `/`) is joined onto `base_dir`; an absolute path and a
/// named volume (no separator) are left as-is. A string with no `:` (no container target) is
/// passed through untouched.
fn resolve_volume(base_dir: &Path, volume: &str) -> String {
    let Some((host, rest)) = volume.split_once(':') else {
        return volume.to_string();
    };
    let looks_like_path = host.starts_with('.') || host.contains('/');
    if !looks_like_path {
        return volume.to_string();
    }
    // A leading `./` is just "here" — drop it so the joined path stays clean (`/base/site`, not
    // `/base/./site`); both are equivalent to Docker, but the tidy form is what shows in logs.
    let host = host.strip_prefix("./").unwrap_or(host);
    let path = Path::new(host);
    let resolved = if path.is_absolute() {
        host.to_string()
    } else {
        base_dir.join(path).to_string_lossy().into_owned()
    };
    format!("{resolved}:{rest}")
}

/// Quote a value for safe interpolation into an `sh -c` command line. Values made only of a small
/// safe set are passed through bare; anything else is single-quoted, with embedded single quotes
/// escaped the POSIX way (`'\''`). An empty string becomes `''`.
fn shell_quote(value: &str) -> String {
    const SAFE: &str = "-_./=:@%+,";
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Substitute `{{ runtime.port.<key> }}` placeholders with the named port; unknown/malformed left verbatim.
fn expand_templates(input: &str, ports: &BTreeMap<String, u16>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str("{{");
            rest = after;
            break;
        };
        let inner = after[..close].trim();
        if let Some(port) = inner
            .strip_prefix("runtime.port.")
            .and_then(|key| ports.get(key.trim()))
        {
            out.push_str(&port.to_string());
        } else {
            out.push_str("{{");
            out.push_str(&after[..close]);
            out.push_str("}}");
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

// MARK: imports — fan every project's hive.yaml into one front door

/// Substitute config variables in an import pattern: `$ADI_PROJECTS_DIR` and
/// `$ADI_DASHBOARDS_DIR` (the projects / dashboards module dirs, honoring `$ADI_DIR`), and `$HOME`.
fn expand_vars(pattern: &str) -> String {
    let cfg = adi_config::Config::open();
    let projects = cfg.projects_dir().to_string_lossy().into_owned();
    let dashboards = cfg
        .module("dashboards")
        .dir()
        .to_string_lossy()
        .into_owned();
    let mut out = pattern
        .replace("$ADI_PROJECTS_DIR", &projects)
        .replace("$ADI_DASHBOARDS_DIR", &dashboards);
    if let Some(home) = std::env::var_os("HOME") {
        out = out.replace("$HOME", &home.to_string_lossy());
    }
    out
}

/// How long a `**` pattern's resolved file list is reused before the tree is walked again.
///
/// Discovery and re-reading are two different questions on two different clocks. *Which* hive.yaml
/// files exist changes when someone adds a project — rarely. What is *in* them changes whenever a
/// service is edited, and the caller re-reads every one of them on its own tick, so that stays as
/// responsive as it ever was.
///
/// Walking the tree is inherently proportional to the tree, and the tree is not ours: a dashboard
/// legitimately kept 17 000 directories of records under its `backend/`, and the reload tick ran
/// every 3 seconds, so *rediscovery alone* cost ~14 000 directory reads a second across two hives —
/// to re-learn a twelve-item list. Pruning build output (see [`PRUNED_DIRS`]) cut half of it; only
/// asking less often removes the rest.
///
/// The cost of the cache is the only thing it delays: a hive.yaml that appears on disk is imported
/// within this window rather than within one tick.
const IMPORT_DISCOVERY_TTL: Duration = Duration::from_secs(60);

/// Resolved `**` patterns, with the instant each was resolved. Keyed by the expanded pattern, so
/// two hives in one process (or a config that imports two roots) never share an entry.
static IMPORT_CACHE: LazyLock<Mutex<HashMap<String, (Instant, Vec<PathBuf>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether a directory entry is a directory, following a symlink but nothing else.
///
/// `file_type` is answered by the directory entry itself on Unix, so the common case costs no
/// `stat` — the syscall that dominated the import walk's profile. A symlink is the one case that
/// still needs one, because a project may legitimately be linked into the projects dir.
fn entry_is_dir(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
        Ok(kind) => kind.is_dir(),
        Err(_) => entry.path().is_dir(),
    }
}

/// Expand a pattern whose wildcards are single `*` segments, each standing for exactly **one**
/// directory level.
///
/// This is the bounded form, and the one the generated configs use. A hive.yaml has a fixed home —
/// `<root>/<id>/.adi/hive.yaml` — so searching for it buys nothing and costs everything: the cost
/// here is the number of entries at each named level, and no part of it grows with what a project
/// happens to keep inside itself.
///
/// Only a whole segment may be `*`; a partial wildcard (`proj-*`) is not supported, and says so
/// rather than silently matching nothing.
fn expand_star_pattern(pattern: &str) -> Vec<PathBuf> {
    let root = if pattern.starts_with('/') { "/" } else { "" };
    let mut heads = vec![PathBuf::from(root)];
    for segment in pattern.split('/').filter(|s| !s.is_empty()) {
        if segment == "*" {
            heads = heads
                .iter()
                .filter_map(|dir| std::fs::read_dir(dir).ok())
                .flat_map(|entries| {
                    entries
                        .flatten()
                        .filter(entry_is_dir)
                        .map(|entry| entry.path())
                        .collect::<Vec<_>>()
                })
                .collect();
        } else {
            if segment.contains('*') {
                warn!(
                    segment,
                    pattern, "a partial wildcard is not supported; use a whole `*` segment"
                );
                return Vec::new();
            }
            for head in &mut heads {
                head.push(segment);
            }
        }
    }
    // Only the files: a `*` level can land on a directory that has no such config in it.
    heads.retain(|p| p.is_file());
    heads.sort();
    heads
}

/// Resolve an import pattern to concrete files. Three forms:
///
/// * `<base>/*/…/<filename>` — each `*` is exactly one directory level. Bounded and predictable;
///   this is what the generated configs use. See [`expand_star_pattern`].
/// * `<base>/**/<filename>` — walk `<base>` recursively. Kept for configs written before the
///   bounded form, and correspondingly defensive: [`PRUNED_DIRS`] keeps it out of build output and
///   its result is cached for [`IMPORT_DISCOVERY_TTL`], because the walk is proportional to a tree
///   that is not ours to bound.
/// * a plain path — included if it exists.
fn find_imports(pattern: &str) -> Vec<PathBuf> {
    if let Some((base, filename)) = pattern.split_once("/**/") {
        // A poisoned lock would mean a panic inside the walk; there is nothing to recover, and
        // falling back to walking is strictly better than propagating it into a reload tick.
        if let Ok(cache) = IMPORT_CACHE.lock()
            && let Some((walked_at, files)) = cache.get(pattern)
            && walked_at.elapsed() < IMPORT_DISCOVERY_TTL
        {
            return files.clone();
        }
        let mut out = Vec::new();
        walk_collect(Path::new(base), filename, &mut out);
        out.sort();
        if let Ok(mut cache) = IMPORT_CACHE.lock() {
            cache.insert(pattern.to_string(), (Instant::now(), out.clone()));
        }
        out
    } else if pattern.contains('*') {
        expand_star_pattern(pattern)
    } else {
        let p = PathBuf::from(pattern);
        if p.exists() { vec![p] } else { Vec::new() }
    }
}

/// Directory names the import walk never descends into.
///
/// A hive.yaml lives at a project's root or in its `.adi/` dir — never inside build output or a
/// vendored dependency tree. Descending anyway is not merely wasted work: one `target/debug/deps`
/// in this very repo reached 562 000 files, and this walk runs on **every** reload tick (see
/// `RELOAD_INTERVAL`), so an unpruned walk keeps a core busy in `stat` forever and starves every
/// other build on the machine of filesystem metadata — a `cargo` invocation that has to list a
/// directory of that size went from 0.08s to 141s while two hives were walking it.
///
/// Hidden directories are deliberately *not* skipped wholesale: `.adi` is exactly where a
/// project's hive.yaml lives.
const PRUNED_DIRS: [&str; 7] = [
    "target",
    "node_modules",
    ".git",
    "dist",
    ".venv",
    "__pycache__",
    ".next",
];

/// Recursively collect files named `filename` under `dir`, skipping [`PRUNED_DIRS`].
fn walk_collect(dir: &Path, filename: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if entry_is_dir(&entry) {
            if PRUNED_DIRS.iter().any(|pruned| name == *pruned) {
                continue;
            }
            walk_collect(&entry.path(), filename, out);
        } else if name == *filename {
            out.push(entry.path());
        }
    }
}

/// The namespace for an imported hive's services: the project id from
/// `.../<project>/.adi/hive.yaml`, else the file's parent dir name, else `import`.
/// The directory an imported hive's runners resolve their relative paths against: the project /
/// dashboard **root**. A hive.yaml at `<root>/.adi/hive.yaml` has its runners' `working_dir` (e.g.
/// `workspaces/main`) relative to `<root>`, not the `.adi` dir — so a trailing `.adi` is stripped
/// (mirroring [`import_namespace`]). Otherwise the file's own directory is used. This is the same
/// base adi-app resolves against, so a service starts the same whether the supervisor or adi-app
/// launched it.
fn import_base_dir(file: &Path) -> Option<PathBuf> {
    let parent = file.parent()?;
    if parent.file_name().is_some_and(|n| n == ".adi") {
        Some(parent.parent().unwrap_or(parent).to_path_buf())
    } else {
        Some(parent.to_path_buf())
    }
}

fn import_namespace(file: &Path) -> String {
    let parent = file.parent();
    let ns = if parent
        .and_then(Path::file_name)
        .is_some_and(|n| n == ".adi")
    {
        parent.and_then(Path::parent).and_then(Path::file_name)
    } else {
        parent.and_then(Path::file_name)
    };
    ns.map_or_else(
        || "import".to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Whether this hive runs as root — i.e. is the machine front door, which routes imported
/// services but must never spawn their (user-owned) processes.
///
/// Deliberately an effective-uid check rather than `$USER`/`$HOME`: the front-door `LaunchDaemon`
/// runs as root while still setting `HOME` to the login user, so the environment does not
/// distinguish the two.
#[cfg(unix)]
fn running_as_root() -> bool {
    // SAFETY: POSIX `geteuid` takes no arguments, cannot fail, and reads no caller memory.
    // Declared inline to keep adi-hive free of a `libc` dependency for this single call.
    #[allow(unsafe_code)]
    unsafe {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        geteuid() == 0
    }
}

/// Windows has no root front door: the `.adi` front door runs as an ordinary per-user scheduled
/// task (see `adi-core`'s `dns` service), so the "am I the privileged daemon?" distinction that
/// gates route-only behavior never applies here.
#[cfg(not(unix))]
fn running_as_root() -> bool {
    false
}

/// Whether two paths point at the same file (canonicalized), so a hive never imports itself.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

// MARK: resolution — from the parsed spec to what the daemon runs

/// One routing rule the proxy enforces: `Host: host` (optionally under `path`) → `upstream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub host: String,
    /// The path prefix this route claims on `host`, already normalised by [`path_prefix`].
    /// `None` is the host's fallback route — the shape every pre-`proxy.path` config has.
    pub path: Option<String>,
    pub upstream: SocketAddr,
}

/// Everything the daemon needs, derived from the spec: binds, routes, and skipped services.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub binds: Vec<SocketAddr>,
    /// TLS binds, verbatim from the config — never defaulted. HTTPS is opt-in: inventing a 443
    /// listener for a hive that didn't ask for one would try to take a privileged port on every
    /// unprivileged dev run.
    pub tls_binds: Vec<SocketAddr>,
    pub routes: Vec<ResolvedRoute>,
    /// Where `*.n.adi` goes, if anywhere. See [`ProxyBinds::mesh_gateway`].
    pub mesh_gateway: Option<SocketAddr>,
    /// Paired node petnames, for the TLS leaf only. See [`ProxyBinds::mesh_nodes`].
    pub mesh_nodes: Vec<String>,
    pub skipped: Vec<String>,
}

/// The comparable form of a `Host` header value: lowercased, without the optional `:port` and
/// without the trailing root dot a resolver-literal name may carry. Both a config's `proxy.host`
/// and a request's `Host` go through this, so the two are compared in the same shape.
#[must_use]
pub fn host_key(host: &str) -> String {
    let host = host.trim();
    let host = host.split(':').next().unwrap_or(host);
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether this hostname lives in the reserved [`MESH_ZONE`] — i.e. names a service on a *remote*
/// node rather than anything on this machine.
#[must_use]
pub fn is_mesh_host(host: &str) -> bool {
    let key = host_key(host);
    key == MESH_ZONE || key.ends_with(MESH_SUFFIX)
}

/// The node label out of `<service>.<node>.n.adi`, for the error page that has to name it — the
/// last label before the zone, so a service that is itself several labels (`app.nosh`) still
/// names its node. The front door deliberately learns nothing else from the name: which peer key
/// a node maps to, and which service it exposes, are the gateway's business (docs/fleet.md §3).
#[must_use]
pub fn mesh_node(host: &str) -> Option<String> {
    let key = host_key(host);
    let head = key.strip_suffix(MESH_SUFFIX)?;
    let node = head.rsplit('.').next().unwrap_or(head);
    (!node.is_empty()).then(|| node.to_string())
}

/// Normalise a configured `proxy.path` into the prefix the router matches on: `None` (this route
/// is the host's fallback) or a `/`-rooted prefix with no trailing slash, so `/api`, `/api/` and
/// `api` all describe the same claim. `/` normalises to `None` — a prefix that matches every path
/// *is* the fallback, and saying so once here keeps the matcher from having to special-case it.
#[must_use]
pub fn path_prefix(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    })
}

impl Hive {
    /// Load a hive.yaml and fan in every service reachable through its `imports`, so one hive can
    /// front-door an entire machine.
    ///
    /// # Errors
    /// Fails only on *this* file: unreadable, or not valid hive YAML. An import that is either is
    /// logged and skipped — one broken project must not take the whole front door down with it.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut hive = Self::parse_file(path)?;
        hive.apply_imports(path);
        Ok(hive)
    }

    /// This file alone, with no import expansion — so the caller pays one read instead of a walk
    /// of every imported project.
    ///
    /// It exists for questions that are answered by the top-level file and nothing else. The bind
    /// addresses are the one that matters: [`merge_import`](Self::merge_import) merges *services*,
    /// so an import can never contribute an address, which lets a caller settle "is another
    /// instance already serving these?" before committing to the expensive load.
    ///
    /// # Errors
    /// Unreadable, or not valid hive YAML.
    pub fn parse_no_imports(path: &Path) -> anyhow::Result<Self> {
        Self::parse_file(path)
    }

    /// Parse a single hive.yaml with no import expansion: rewrite ``bash`…` `` port commands into
    /// valid YAML placeholders, then parse with the command table installed so port fields run
    /// their commands on read.
    fn parse_file(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let (yaml, commands) = adi_ports_manager::preprocess(&raw);
        let hive: Self =
            adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str(&yaml))
                .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(hive)
    }

    /// Expand each `imports` glob and merge the matched hive.yaml files' services in, keyed
    /// `<project>/<service>`.
    ///
    /// A **root** hive keeps only the routes and drops every imported runner, so the machine
    /// front door never spawns user processes. An **unprivileged** hive keeps them, so a
    /// per-user supervisor can import the same files and actually run those services. Both
    /// resolve identical service keys, so the ports manager hands each side the same port.
    ///
    /// Best-effort: an unreadable or unparsable import is logged and skipped, never fatal.
    fn apply_imports(&mut self, base: &Path) {
        // The declared intent wins; root still implies it, so a config that predates the key
        // behaves exactly as before.
        let strip_runners = self.proxy.routes_only || running_as_root();
        let patterns = std::mem::take(&mut self.imports);
        for pattern in patterns {
            for file in find_imports(&expand_vars(&pattern)) {
                if same_file(&file, base) {
                    continue;
                }
                match Self::parse_file(&file) {
                    Ok(child) => {
                        // Imported services resolve their relative paths against their project root
                        // (the dir that holds `.adi/`) — the same base adi-app uses, not this
                        // importer's base_dir. See [`import_base_dir`].
                        self.merge_import(
                            child,
                            &import_namespace(&file),
                            strip_runners,
                            import_base_dir(&file).as_deref(),
                        );
                    }
                    Err(e) => {
                        warn!(file = %file.display(), error = %e, "skipping unreadable import");
                    }
                }
            }
        }
    }

    /// Merge one imported hive's services under `ns`, dropping their runners when
    /// `strip_runners`. An already-present key wins, so a local service is never overridden.
    fn merge_import(&mut self, child: Self, ns: &str, strip_runners: bool, dir: Option<&Path>) {
        for (name, mut svc) in child.services {
            if strip_runners {
                svc.runner = None;
            }
            // Remember where this service came from, so its runner's relative paths resolve there.
            svc.base_dir = dir.map(Path::to_path_buf);
            self.services.entry(format!("{ns}/{name}")).or_insert(svc);
        }
    }

    #[must_use]
    pub fn resolve(&self) -> Resolved {
        let binds = if self.proxy.bind.is_empty() {
            default_bind()
        } else {
            self.proxy.bind.clone()
        };

        let mut routes = Vec::new();
        let mut skipped = Vec::new();
        for (name, svc) in &self.services {
            let Some(proxy) = &svc.proxy else {
                continue;
            };
            // `n.adi` belongs to remote nodes and to nothing else. Honouring a local claim on it
            // would let any imported project shadow a whole fleet's namespace, so the route is
            // dropped here rather than deprioritised in the router — the collision must be
            // impossible, not merely unlikely.
            if is_mesh_host(&proxy.host) {
                warn!(service = %name, host = %proxy.host,
                      "`{MESH_ZONE}` is reserved for remote nodes; refusing this route");
                skipped.push(format!(
                    "{name} (host {}): `{MESH_ZONE}` is reserved for remote nodes",
                    proxy.host
                ));
                continue;
            }
            match svc.http_port() {
                Some(port) => routes.push(ResolvedRoute {
                    host: proxy.host.clone(),
                    path: path_prefix(proxy.path.as_deref()),
                    upstream: SocketAddr::new(UPSTREAM_IP, port),
                }),
                None => skipped.push(format!("{name} (host {}): no HTTP port", proxy.host)),
            }
        }
        Resolved {
            binds,
            tls_binds: self.proxy.tls_bind.clone(),
            routes,
            mesh_gateway: self.proxy.mesh_gateway,
            mesh_nodes: self.proxy.mesh_nodes.clone(),
            skipped,
        }
    }
}

fn default_bind() -> Vec<SocketAddr> {
    vec![SocketAddr::new(UPSTREAM_IP, 8080)]
}

/// The canonical config location `$HOME/$ADI_DIR/mono/hive/hive.yaml` (default `~/.adi/mono/hive/hive.yaml`).
/// The location comes from the shared [`adi_config`] store; hive owns the YAML format
/// and reads it as a raw file within the `hive` module.
#[must_use]
pub fn default_config_path() -> PathBuf {
    adi_config::Config::open()
        .module(HIVE_MODULE)
        .raw_path(HIVE_CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
version: "1"

proxy:
  bind:
    - "127.0.0.1:80"
    - "127.0.0.1:8080"

services:
  frontend:
    proxy:
      host: app.test
      path: /
    rollout:
      type: recreate
      recreate:
        ports:
          http: 8010
    runner:
      type: script
      script:
        run: bun run dev
  backend:
    proxy:
      host: api.test
    rollout:
      recreate:
        ports:
          http: 8009
  postgres:
    rollout:
      recreate:
        ports:
          db: 8045
"#;

    #[test]
    fn resolves_proxy_binds_and_service_routes_from_a_full_hive_yaml() {
        let hive: Hive = serde_yaml_ng::from_str(SAMPLE).expect("hive.yaml parses");
        let r = hive.resolve();

        assert_eq!(
            r.binds,
            vec![
                "127.0.0.1:80".parse().unwrap(),
                "127.0.0.1:8080".parse().unwrap(),
            ]
        );

        let mut got: Vec<(String, String)> = r
            .routes
            .iter()
            .map(|route| (route.host.clone(), route.upstream.to_string()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("api.test".to_string(), "127.0.0.1:8009".to_string()),
                ("app.test".to_string(), "127.0.0.1:8010".to_string()),
            ]
        );

        assert!(r.skipped.is_empty(), "postgres is silently not-routed");
    }

    /// The shipped example is documentation, and documentation rots. Load it for real so a field
    /// renamed here can't leave the worked example describing a schema that no longer exists.
    #[test]
    fn the_worked_example_hive_yaml_still_parses_and_resolves() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join(HIVE_CONFIG_FILE);
        let hive = Hive::load(&example).expect("the example hive.yaml loads");
        let r = hive.resolve();
        // Against the platform constant, not a literal: the example is what an operator copies,
        // so if it drifts from the port adi-mesh actually binds, the copy silently 502s.
        assert_eq!(r.mesh_gateway, Some(adi_config::mesh_gateway_addr()));
        assert_eq!(r.mesh_nodes, vec!["laptop-b".to_string()]);
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        let nosh: Vec<_> = r.routes.iter().filter(|x| x.host == "nosh.adi").collect();
        assert_eq!(nosh.len(), 2, "the one-origin dashboard is two routes");
        assert!(nosh.iter().any(|x| x.path.as_deref() == Some("/api")));
        assert!(nosh.iter().any(|x| x.path.is_none()));
    }

    #[test]
    fn a_service_path_becomes_a_normalised_route_prefix() {
        // One host, two services: the frontend owns it, the backend claims `/api` on it.
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  frontend:
    proxy: { host: nosh.adi }
    rollout: { recreate: { ports: { http: 8010 } } }
  backend:
    proxy: { host: nosh.adi, path: /api/ }
    rollout: { recreate: { ports: { http: 8011 } } }
",
        )
        .unwrap();
        let mut got: Vec<(String, Option<String>, u16)> = hive
            .resolve()
            .routes
            .into_iter()
            .map(|r| (r.host, r.path, r.upstream.port()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("nosh.adi".to_string(), None, 8010),
                ("nosh.adi".to_string(), Some("/api".to_string()), 8011),
            ]
        );
    }

    #[test]
    fn a_slash_path_is_the_host_fallback_just_like_no_path() {
        // Every config written before `proxy.path` did anything says `path: /`. It must stay the
        // plain host route, or those configs would change behaviour under the new matcher.
        assert_eq!(path_prefix(None), None);
        assert_eq!(path_prefix(Some("/")), None);
        assert_eq!(path_prefix(Some("   ")), None);
        assert_eq!(path_prefix(Some("/api")), Some("/api".to_string()));
        assert_eq!(path_prefix(Some("/api/")), Some("/api".to_string()));
        assert_eq!(path_prefix(Some("api")), Some("/api".to_string()));
        assert_eq!(path_prefix(Some(" /api/v1 ")), Some("/api/v1".to_string()));
    }

    #[test]
    fn a_service_may_not_claim_a_host_in_the_reserved_mesh_zone() {
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  impostor:
    proxy: { host: app.laptop-b.n.adi }
    rollout: { recreate: { ports: { http: 8010 } } }
  apex:
    proxy: { host: n.adi }
    rollout: { recreate: { ports: { http: 8011 } } }
  local:
    proxy: { host: app.adi }
    rollout: { recreate: { ports: { http: 8012 } } }
",
        )
        .unwrap();
        let r = hive.resolve();
        assert_eq!(
            r.routes.iter().map(|x| x.host.as_str()).collect::<Vec<_>>(),
            vec!["app.adi"],
            "only the local host is routable"
        );
        assert_eq!(r.skipped.len(), 2);
        assert!(r.skipped.iter().all(|s| s.contains("reserved")), "{:?}", r.skipped);
    }

    #[test]
    fn recognises_the_reserved_mesh_zone_and_reads_the_node_out_of_it() {
        assert!(is_mesh_host("nosh.laptop-b.n.adi"));
        assert!(is_mesh_host("NOSH.Laptop-B.N.ADI:443"));
        assert!(is_mesh_host("n.adi"));
        // A name that merely *contains* the labels is not in the zone.
        assert!(!is_mesh_host("app.adi"));
        assert!(!is_mesh_host("n.adi.example.com"));
        assert!(!is_mesh_host("notn.adi"));

        assert_eq!(mesh_node("nosh.laptop-b.n.adi").as_deref(), Some("laptop-b"));
        assert_eq!(mesh_node("laptop-b.n.adi").as_deref(), Some("laptop-b"));
        assert_eq!(mesh_node("a.b.laptop-b.n.adi").as_deref(), Some("laptop-b"));
        assert_eq!(mesh_node("n.adi"), None, "the zone apex names no node");
        assert_eq!(mesh_node("app.adi"), None);
    }

    #[test]
    fn the_mesh_gateway_and_node_list_are_read_from_the_proxy_block() {
        let hive: Hive = serde_yaml_ng::from_str(
            r#"
proxy:
  bind: ["127.0.0.1:8080"]
  mesh_gateway: "127.0.0.1:8099"
  mesh_nodes: [laptop-b, tower]
"#,
        )
        .unwrap();
        let r = hive.resolve();
        assert_eq!(r.mesh_gateway, "127.0.0.1:8099".parse().ok());
        assert_eq!(r.mesh_nodes, vec!["laptop-b".to_string(), "tower".to_string()]);

        // Absent by default — a hive that says nothing about the mesh has none.
        let bare = Hive::default().resolve();
        assert_eq!(bare.mesh_gateway, None);
        assert!(bare.mesh_nodes.is_empty());
    }

    #[test]
    fn ignores_unknown_hive_fields() {
        let hive: Hive = serde_yaml_ng::from_str(
            r#"
observability:
  plugins: [stdout]
services:
  api:
    proxy: { host: api.test }
    healthcheck: { type: tcp }
    environment:
      static: { PORT: "8009" }
    depends_on: [postgres]
    restart: on-failure
    rollout: { recreate: { ports: { http: 8009 } } }
"#,
        )
        .expect("unknown fields are ignored");
        assert_eq!(hive.resolve().routes.len(), 1);
    }

    #[test]
    fn skips_a_proxied_service_with_no_http_port() {
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  api:
    proxy: { host: api.test }
    rollout: { recreate: { ports: { db: 5432, grpc: 9000 } } }
",
        )
        .unwrap();
        let r = hive.resolve();
        assert!(r.routes.is_empty());
        assert_eq!(r.skipped.len(), 1);
        assert!(r.skipped[0].contains("api.test"));
    }

    #[test]
    fn empty_config_falls_back_to_the_default_bind() {
        let r = Hive::default().resolve();
        assert_eq!(r.binds, vec!["127.0.0.1:8080".parse().unwrap()]);
        assert!(r.routes.is_empty());
    }

    #[test]
    fn default_path_is_under_the_mono_hive_namespace() {
        let p = default_config_path();
        assert!(p.ends_with("mono/hive/hive.yaml"), "got {}", p.display());
    }

    #[test]
    fn resolves_a_script_runner_with_port_env_and_working_dir() {
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  frontend:
    proxy: { host: app.adi }
    rollout: { recreate: { ports: { http: 8010 } } }
    restart: always
    runner:
      type: script
      script:
        run: serve --port {{runtime.port.http}}
        working_dir: web/frontend
",
        )
        .expect("hive.yaml parses");
        let runners = hive.runners(Path::new("/project"));

        assert_eq!(runners.len(), 1);
        let frontend = &runners[0];
        assert_eq!(frontend.name, "frontend");
        assert_eq!(frontend.run, "serve --port 8010");
        assert_eq!(frontend.working_dir, Path::new("/project/web/frontend"));
        assert!(
            frontend
                .env
                .contains(&("PORT".to_string(), "8010".to_string()))
        );
        assert!(
            frontend
                .env
                .contains(&("PORT_HTTP".to_string(), "8010".to_string()))
        );
        assert_eq!(frontend.restart, RestartPolicy::Always);
    }

    #[test]
    fn expands_runtime_port_templates_and_leaves_unknown_ones() {
        let mut ports = BTreeMap::new();
        ports.insert("http".to_string(), 8010u16);
        assert_eq!(
            expand_templates("serve --port {{runtime.port.http}}", &ports),
            "serve --port 8010"
        );
        assert_eq!(
            expand_templates("p={{ runtime.port.http }}", &ports),
            "p=8010"
        );
        assert_eq!(
            expand_templates("{{runtime.port.db}} and {{oops", &ports),
            "{{runtime.port.db}} and {{oops"
        );
    }

    /// `routes_only` must strip imported runners for an **unprivileged** hive — the whole point
    /// of the key. Root already stripped them, which is why the front door behaved on macOS and
    /// misbehaved on a Linux node: unprivileged there, it kept every imported runner and started
    /// a second copy of every dashboard against the supervisor's own leased ports.
    #[test]
    fn a_routes_only_hive_imports_routes_without_runners() {
        let base = std::env::temp_dir().join(format!(
            "adi-hive-routesonly-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let proj = base.join("proj/.adi");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("hive.yaml"),
            "services:\n  app:\n    proxy: { host: proj.adi }\n    rollout: { recreate: { ports: { http: 9124 } } }\n    runner: { type: script, script: { run: \"echo hi\" } }\n",
        )
        .unwrap();

        assert!(
            !running_as_root(),
            "this test asserts the unprivileged path; run it as a normal user"
        );

        // Same imports, the flag the only difference.
        let importer = |routes_only: bool, name: &str| {
            let path = base.join(name);
            std::fs::write(
                &path,
                format!(
                    "proxy:\n  routes_only: {routes_only}\nimports:\n  - {}/**/hive.yaml\n",
                    base.display()
                ),
            )
            .unwrap();
            Hive::load(&path).expect("load with imports")
        };

        let supervisor = importer(false, "supervisor.yaml");
        assert!(
            supervisor.services["proj/app"].runner.is_some(),
            "without the flag an unprivileged hive still supervises what it imports"
        );

        let front_door = importer(true, "frontdoor.yaml");
        let svc = &front_door.services["proj/app"];
        assert!(
            svc.runner.is_none(),
            "routes_only must drop the runner, or two hives race to run one service"
        );
        assert_eq!(
            svc.proxy.as_ref().expect("proxy").host,
            "proj.adi",
            "the route survives — dropping the runner must not drop the reason to import"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn imports_fan_in_project_services_namespaced_and_runnable() {
        let base = std::env::temp_dir().join(format!(
            "adi-hive-imports-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let proj = base.join("proj/.adi");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("hive.yaml"),
            "services:\n  app:\n    proxy: { host: proj.adi }\n    rollout: { recreate: { ports: { http: 9123 } } }\n    runner: { type: script, script: { run: \"echo hi\" } }\n",
        )
        .unwrap();
        let parent = base.join("parent.yaml");
        std::fs::write(
            &parent,
            format!("imports:\n  - {}/**/hive.yaml\n", base.display()),
        )
        .unwrap();

        let hive = Hive::load(&parent).expect("load with imports");
        let svc = hive
            .services
            .get("proj/app")
            .expect("imported service present");
        assert_eq!(svc.proxy.as_ref().expect("proxy").host, "proj.adi");
        assert_eq!(svc.http_port(), Some(9123));
        // Runners survive an import here because the test process is unprivileged — that is
        // what lets a per-user supervisor run the services a root front door only routes.
        // The root-strips-runners half of the contract is asserted in
        // `a_root_hive_keeps_only_routes_from_imports`.
        assert!(
            !running_as_root(),
            "this test asserts the unprivileged import path; run it as a normal user"
        );
        assert!(
            svc.runner.is_some(),
            "an unprivileged hive keeps imported runners so it can supervise them"
        );
        let routes = hive.resolve().routes;
        assert!(
            routes
                .iter()
                .any(|r| r.host == "proj.adi" && r.upstream.port() == 9123)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A build directory is not a place projects live, and walking one is what made this walk
    /// expensive enough to starve the machine: `target/debug/deps` here reached 562 000 files, and
    /// the walk runs on every reload tick. A hive.yaml inside a pruned directory is therefore
    /// deliberately invisible — the sibling real project proves the walk still works.
    #[test]
    fn the_import_walk_skips_build_and_vendor_directories() {
        let base = std::env::temp_dir().join(format!(
            "adi-hive-pruned-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let service = |host: &str| {
            format!("services:\n  app:\n    proxy: {{ host: {host} }}\n    rollout: {{ recreate: {{ ports: {{ http: 9124 }} }} }}\n")
        };
        for pruned in ["target", "node_modules", ".git", "dist"] {
            let dir = base.join(pruned).join("buried/.adi");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("hive.yaml"), service("buried.adi")).unwrap();
        }
        // `.adi` is hidden too, and is exactly where a real project keeps its config — so the
        // prune must not be "skip dotted directories".
        let real = base.join("proj/.adi");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("hive.yaml"), service("proj.adi")).unwrap();

        let parent = base.join("parent.yaml");
        std::fs::write(
            &parent,
            format!("imports:\n  - {}/**/hive.yaml\n", base.display()),
        )
        .unwrap();

        let hive = Hive::load(&parent).expect("load with imports");
        assert!(
            hive.services.contains_key("proj/app"),
            "the real project must still be imported, got: {:?}",
            hive.services.keys().collect::<Vec<_>>()
        );
        assert!(
            !hive.services.contains_key("buried/app"),
            "a hive.yaml inside a pruned directory must not be imported, got: {:?}",
            hive.services.keys().collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The bounded form: `*` is one directory level, so what a project keeps inside itself cannot
    /// make discovery more expensive — the whole point of preferring it to `**`.
    #[test]
    fn a_star_segment_matches_one_level_and_never_descends() {
        let base = std::env::temp_dir().join(format!(
            "adi-hive-star-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        // The real layout: `<root>/<id>/.adi/hive.yaml`, for two projects.
        for id in ["proj-a", "proj-b"] {
            let dir = base.join(id).join(".adi");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("hive.yaml"), "services: {}\n").unwrap();
        }
        // Deeper than the pattern reaches, and inside a directory a project owns: found by `**`,
        // deliberately invisible to `*`. This is the data store that made the walk expensive.
        let deep = base.join("proj-a/backend/database/records/x/.adi");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("hive.yaml"), "services: {}\n").unwrap();

        let found = find_imports(&format!("{}/*/.adi/hive.yaml", base.display()));
        assert_eq!(
            found.len(),
            2,
            "exactly the two projects at the fixed depth: {found:?}"
        );
        assert!(
            found.iter().all(|p| !p.starts_with(base.join("proj-a/backend"))),
            "a `*` segment must not descend into a project's own tree: {found:?}"
        );
        // A level that exists but holds no such file contributes nothing rather than erroring.
        std::fs::create_dir_all(base.join("empty-project")).unwrap();
        assert_eq!(
            find_imports(&format!("{}/*/.adi/hive.yaml", base.display())).len(),
            2,
            "a project without a config is skipped, not fatal"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Discovery is cached, and this is the price of that: a hive.yaml that appears *after* a walk
    /// waits for [`IMPORT_DISCOVERY_TTL`] rather than for the next tick. Asserted rather than left
    /// implicit, because it is the one behaviour the cache changes — and the reason it is worth it
    /// is that the walk is proportional to a tree we do not own.
    #[test]
    fn import_discovery_is_cached_so_a_reload_tick_does_not_rewalk_the_tree() {
        let base = std::env::temp_dir().join(format!(
            "adi-hive-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let first = base.join("one/.adi");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("hive.yaml"), "services: {}\n").unwrap();
        let pattern = format!("{}/**/hive.yaml", base.display());

        let found = find_imports(&pattern);
        assert_eq!(found.len(), 1, "the first call walks: {found:?}");

        // A second project appears on disk after that walk.
        let second = base.join("two/.adi");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("hive.yaml"), "services: {}\n").unwrap();
        assert_eq!(
            find_imports(&pattern).len(),
            1,
            "within the TTL the cached list is reused, so the walk is not repeated"
        );

        // A different root is a different key, and is walked on its own.
        let other = base.join("other/proj/.adi");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("hive.yaml"), "services: {}\n").unwrap();
        assert_eq!(
            find_imports(&format!("{}/other/**/hive.yaml", base.display())).len(),
            1,
            "a pattern that was never walked must not read another pattern's cache"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The root front door's half of the import contract: keep the route, drop the runner, so
    /// a root hive never spawns a user process. Exercised through [`Hive::merge_import`]
    /// directly — the branch is uid-gated, and the suite does not run as root.
    #[test]
    fn a_root_hive_keeps_only_routes_from_imports() {
        let child: Hive = serde_yaml_ng::from_str(
            "services:\n  app:\n    proxy: { host: proj.adi }\n    rollout: { recreate: { ports: { http: 9123 } } }\n    runner: { type: script, script: { run: \"echo hi\" } }\n",
        )
        .expect("parse child hive");

        let mut root = Hive::default();
        root.merge_import(child.clone(), "proj", true, None);
        let svc = root.services.get("proj/app").expect("service imported");
        assert!(
            svc.runner.is_none(),
            "a root hive must not carry an imported runner"
        );
        assert_eq!(
            svc.http_port(),
            Some(9123),
            "dropping the runner must not drop the route"
        );

        // An unprivileged import keeps the runner AND records the import's directory, so its runner
        // resolves relative paths there rather than under the importer.
        let mut user = Hive::default();
        user.merge_import(child, "proj", false, Some(Path::new("/srv/proj")));
        let svc = &user.services["proj/app"];
        assert!(
            svc.runner.is_some(),
            "an unprivileged hive keeps the runner so it can supervise it"
        );
        assert_eq!(svc.base_dir.as_deref(), Some(Path::new("/srv/proj")));
    }

    #[test]
    fn import_base_dir_strips_a_trailing_dot_adi() {
        // A project hive.yaml lives in `<project>/.adi/hive.yaml`; its runners resolve against
        // `<project>` (where `workspaces/` lives), NOT the `.adi` dir.
        assert_eq!(
            import_base_dir(Path::new("/x/mono/projects/backend/.adi/hive.yaml")),
            Some(PathBuf::from("/x/mono/projects/backend")),
        );
        // A hive.yaml not in a `.adi` dir just uses its own directory.
        assert_eq!(
            import_base_dir(Path::new("/x/mono/hive/hive.yaml")),
            Some(PathBuf::from("/x/mono/hive")),
        );
    }

    #[test]
    fn an_imported_runner_resolves_its_relative_working_dir_under_its_own_project() {
        // A project service with a *relative* working_dir, imported by a supervisor rooted elsewhere.
        let child: Hive = serde_yaml_ng::from_str(
            "services:\n  api:\n    runner: { script: { run: \"bun run dev\", working_dir: workspaces/main } }\n",
        )
        .expect("parse child");
        let mut sup = Hive::default();
        sup.merge_import(child, "proj", false, Some(Path::new("/home/u/.adi/mono/projects/proj")));

        // The supervisor's own base_dir is elsewhere; the imported service must ignore it.
        let runners = sup.runners(Path::new("/home/u/.adi/mono/dashboards"));
        assert_eq!(runners.len(), 1);
        assert_eq!(
            runners[0].working_dir,
            Path::new("/home/u/.adi/mono/projects/proj/workspaces/main"),
            "imported relative working_dir must resolve under the project, not the supervisor",
        );
    }

    #[test]
    fn restart_policy_parses_case_insensitively_with_on_failure_default() {
        assert_eq!(RestartPolicy::parse(Some("Always")), RestartPolicy::Always);
        assert_eq!(RestartPolicy::parse(Some(" no ")), RestartPolicy::Never);
        assert_eq!(
            RestartPolicy::parse(Some("on-failure")),
            RestartPolicy::OnFailure
        );
        assert_eq!(RestartPolicy::parse(None), RestartPolicy::OnFailure);
    }

    #[test]
    fn allocates_a_missing_port_from_the_manager_for_both_route_and_runner() {
        let mut hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  app:
    proxy: { host: app.adi }
    runner: { type: script, script: { run: adi-app } }
",
        )
        .unwrap();
        assert!(hive.resolve().routes.is_empty(), "no port yet -> no route");

        let registry = std::env::temp_dir().join(format!(
            "adi-hive-alloc-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
        let manager = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: registry.clone(),
            ..adi_ports_manager::Config::default()
        });

        let allocated = hive.allocate_missing_ports(&manager);
        assert_eq!(allocated.len(), 1);
        let (svc, port) = &allocated[0];
        assert_eq!(svc, "app");

        let routes = hive.resolve().routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].upstream.port(), *port);
        let runners = hive.runners(Path::new("/x"));
        assert!(
            runners[0]
                .env
                .contains(&("PORT".to_string(), port.to_string()))
        );

        assert!(hive.allocate_missing_ports(&manager).is_empty());
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
    }

    #[test]
    fn resolves_a_bash_backtick_port_command_written_unquoted() {
        let registry = std::env::temp_dir().join(format!(
            "adi-hive-cmd-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
        let manager = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: registry.clone(),
            ..adi_ports_manager::Config::default()
        });

        // The command is written UNQUOTED, in flow style, exactly as a project hive.yaml does:
        // `http: bash`ports-manager.get('demo/app')``. The preprocessor rewrites it to a valid
        // `datacommand:<hash>` placeholder; parsing then runs it on read, reserving the port
        // against the (overridden) registry.
        let raw = r"
services:
  app:
    proxy: { host: demo.adi, path: / }
    rollout: { recreate: { ports: { http: bash`ports-manager.get('demo/app')` } } }
";
        let (yaml, commands) = adi_ports_manager::preprocess(raw);
        let hive: Hive = adi_ports_manager::with_ports(manager.clone(), || {
            adi_ports_manager::with_commands(commands, || serde_yaml_ng::from_str(&yaml))
                .expect("preprocessed bash`…` command parses and resolves")
        });

        let port = manager
            .get("demo/app", "port")
            .expect("lookup")
            .expect("the command reserved a port on read");
        let routes = hive.resolve().routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].host, "demo.adi");
        assert_eq!(routes[0].upstream.port(), port);
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
    }

    #[test]
    fn takes_the_front_door_bind_port_from_the_manager_when_unset() {
        let registry = std::env::temp_dir().join(format!(
            "adi-hive-bind-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
        let manager = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: registry.clone(),
            ..adi_ports_manager::Config::default()
        });

        let mut hive = Hive::default();
        let port = hive
            .allocate_bind_port(&manager)
            .expect("allocated a bind port");
        assert_eq!(
            hive.resolve().binds,
            vec![SocketAddr::new(UPSTREAM_IP, port)]
        );
        assert_eq!(hive.allocate_bind_port(&manager), None);

        let mut explicit: Hive =
            serde_yaml_ng::from_str(r#"proxy: { bind: ["127.0.0.53:80"] }"#).unwrap();
        assert_eq!(explicit.allocate_bind_port(&manager), None);
        assert_eq!(
            explicit.resolve().binds,
            vec!["127.0.0.53:80".parse().unwrap()]
        );
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
    }

    #[test]
    fn a_runner_with_neither_script_nor_docker_is_skipped() {
        // A bare `type: docker` is *not* a docker runner — the runner kind is chosen by the
        // `script`/`docker` sub-block, and an unknown `type` key is ignored. With neither block
        // there is nothing to launch, so the service is skipped.
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  worker:
    runner:
      type: docker
    rollout: { recreate: { ports: { http: 8009 } } }
",
        )
        .unwrap();
        assert!(hive.runners(Path::new("/x")).is_empty());
    }

    #[test]
    fn docker_runner_compiles_to_a_supervised_docker_run() {
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  web:
    proxy: { host: web.adi }
    rollout: { recreate: { ports: { http: 8080 } } }
    restart: always
    environment: { static: { LOG_LEVEL: info } }
    runner:
      docker:
        image: nginx:1.27
        ports: { http: 80 }
        volumes: ['./site:/usr/share/nginx/html:ro', 'named:/cache']
        environment: { LOG_LEVEL: debug, EXTRA: '1' }
        pull: always
        args: ['--memory=512m']
        command: ['nginx', '-g', 'daemon off;']
",
        )
        .unwrap();

        let runners = hive.runners(Path::new("/srv/web"));
        assert_eq!(runners.len(), 1);
        let spec = &runners[0];
        assert_eq!(spec.name, "web");
        assert_eq!(spec.restart, RestartPolicy::Always);
        // The container is everything — no host-process env is threaded in.
        assert!(spec.env.is_empty());

        let run = &spec.run;
        // Attach to an existing container (start it, no restart); create it only if absent; then
        // `wait` so this stays the supervised foreground process. Never `--rm` / `rm -f`.
        assert!(
            run.starts_with("docker start adi-web >/dev/null 2>&1 || docker run -d --name adi-web"),
            "got: {run}"
        );
        assert!(run.ends_with("; exec docker wait adi-web"), "got: {run}");
        assert!(!run.contains("--rm"), "no --rm (persistent container): {run}");
        assert!(run.contains("--pull always"), "got: {run}");
        // Leased host port 8080 → container 80, on loopback.
        assert!(run.contains("-p 127.0.0.1:8080:80"), "got: {run}");
        // Container gets the PORT convention pointing at the *container* port.
        assert!(run.contains("-e PORT=80"), "got: {run}");
        assert!(run.contains("-e PORT_HTTP=80"), "got: {run}");
        // The block's env overrides the service's static env of the same name.
        assert!(run.contains("-e LOG_LEVEL=debug"), "got: {run}");
        assert!(!run.contains("LOG_LEVEL=info"), "override should win: {run}");
        assert!(run.contains("-e EXTRA=1"), "got: {run}");
        // Relative bind-mount host path resolved against base_dir; a named volume left alone.
        assert!(
            run.contains("-v /srv/web/site:/usr/share/nginx/html:ro"),
            "got: {run}"
        );
        assert!(run.contains("-v named:/cache"), "got: {run}");
        assert!(run.contains("--memory=512m"), "got: {run}");
        // Image, then the overriding command (with the space-bearing arg quoted), before the `wait`.
        assert!(
            run.contains("nginx:1.27 nginx -g 'daemon off;'; exec docker wait adi-web"),
            "got: {run}"
        );
    }

    #[test]
    fn docker_runner_gets_a_host_port_allocated_like_a_script() {
        // A proxied docker service with no declared http port has one leased, just as a script
        // runner would — so `ports: { http: ... }` has a host side to publish.
        let registry = std::env::temp_dir().join(format!(
            "adi-hive-docker-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
        let manager = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: registry.clone(),
            ..adi_ports_manager::Config::default()
        });

        let mut hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  api:
    proxy: { host: api.adi }
    runner:
      docker:
        image: my/api:latest
        ports: { http: 3000 }
",
        )
        .unwrap();
        let allocated = hive.allocate_missing_ports(&manager);
        assert_eq!(allocated.len(), 1, "one http port leased for the container");
        let host = allocated[0].1;

        let runners = hive.runners(Path::new("/x"));
        assert!(
            runners[0].run.contains(&format!("-p 127.0.0.1:{host}:3000")),
            "got: {}",
            runners[0].run
        );
        let _ = std::fs::remove_dir_all(registry.parent().unwrap());
    }

    #[test]
    fn declaring_both_runner_kinds_is_refused() {
        // Ambiguous: a service with both a `script` and a `docker` runner is skipped (not started),
        // rather than silently picking one.
        let hive: Hive = serde_yaml_ng::from_str(
            r"
services:
  svc:
    runner:
      script: { run: 'echo hi' }
      docker: { image: busybox }
",
        )
        .unwrap();
        assert!(hive.runners(Path::new("/x")).is_empty());
    }

    #[test]
    fn container_name_sanitizes_and_can_be_overridden() {
        assert_eq!(container_name(None, "app"), "adi-app");
        assert_eq!(container_name(None, "proj/app"), "adi-proj-app");
        assert_eq!(container_name(Some("custom"), "proj/app"), "custom");
        assert_eq!(container_name(Some("  "), "app"), "adi-app");
    }

    #[test]
    fn resolve_volume_only_rewrites_relative_paths() {
        let base = Path::new("/base");
        assert_eq!(
            resolve_volume(base, "./data:/data"),
            "/base/data:/data"
        );
        assert_eq!(resolve_volume(base, "sub/x:/x:ro"), "/base/sub/x:/x:ro");
        assert_eq!(resolve_volume(base, "/abs:/data"), "/abs:/data");
        assert_eq!(resolve_volume(base, "named:/data"), "named:/data");
        assert_eq!(resolve_volume(base, "no-target"), "no-target");
    }

    #[test]
    fn shell_quote_passes_safe_and_escapes_the_rest() {
        assert_eq!(shell_quote("nginx:1.27"), "nginx:1.27");
        assert_eq!(shell_quote("PORT=80"), "PORT=80");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("daemon off;"), "'daemon off;'");
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
