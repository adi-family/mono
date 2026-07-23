//! The single hive config, loaded from `~/.adi/mono/hive/hive.yaml`: the reverse-proxy
//! fields and the fields needed to run a service locally. Unknown hive.yaml fields are
//! accepted-but-ignored (no `deny_unknown_fields`).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

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
    /// Optional front-door name; the ports-manager lease key when the bind port is manager-allocated.
    #[serde(default)]
    pub name: Option<String>,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceProxy {
    pub host: String,
    /// Accepted (part of the hive schema) but unused: routing is host-based for now.
    #[serde(default)]
    #[allow(dead_code)]
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
/// compiles (see [`Docker::command_line`]) to a single foreground
/// `docker run --rm` invocation, so the existing supervisor handles its whole lifecycle: a clean
/// `docker run` (no `-d`) stays attached, forwards adi-hive's `SIGTERM` to the container, and
/// removes it on exit; a changed spec hot-reloads like any other runner.
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
                    run: docker.command_line(name, svc, ports, base_dir),
                    working_dir: base_dir.to_path_buf(),
                    env: Vec::new(),
                    restart,
                },
                (None, Some(script)) => RunnerSpec {
                    name: name.clone(),
                    run: expand_templates(&script.run, ports),
                    working_dir: resolve_working_dir(base_dir, script.working_dir.as_deref()),
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
    /// Shape: `docker rm -f <name> …; exec docker run --rm --name <name> [flags] <image> [command]`.
    ///
    /// - `exec` makes `docker run` replace the shell, so it *is* the supervised process — adi-hive's
    ///   `SIGTERM` reaches it directly, and `docker run` (foreground) forwards it to the container.
    /// - The leading `docker rm -f` clears any container the previous run orphaned (if adi-hive had
    ///   to `SIGKILL` it past the grace period), so a relaunch never trips over a name clash.
    /// - Every interpolated value is shell-quoted, so image names, env values, and paths with spaces
    ///   or metacharacters can't break out of the command.
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
        let mut run: Vec<String> = ["docker", "run", "--rm", "--name"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        run.push(shell_quote(&name));

        if let Some(pull) = self.pull.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            run.push("--pull".to_string());
            run.push(shell_quote(pull));
        }

        // Publish each mapped host port key to its container port, on loopback so the container is
        // reachable only through the front door (as a script runner on 127.0.0.1 would be).
        for (key, container_port) in &self.ports {
            if let Some(host_port) = host_ports.get(key) {
                run.push("-p".to_string());
                run.push(format!("127.0.0.1:{host_port}:{container_port}"));
            }
        }

        for (key, value) in self.container_env(svc) {
            run.push("-e".to_string());
            run.push(shell_quote(&format!("{key}={value}")));
        }

        for volume in &self.volumes {
            run.push("-v".to_string());
            run.push(shell_quote(&resolve_volume(base_dir, volume)));
        }

        // Raw passthrough flags — the escape hatch for anything not modelled first-class.
        for arg in &self.args {
            run.push(shell_quote(arg));
        }

        run.push(shell_quote(&self.image));
        for arg in self.command.iter().flatten() {
            run.push(shell_quote(arg));
        }

        format!(
            "docker rm -f {} >/dev/null 2>&1; exec {}",
            shell_quote(&name),
            run.join(" ")
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
    let projects = cfg.module("projects").dir().to_string_lossy().into_owned();
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

/// Resolve an import pattern to concrete files. Supports `<base>/**/<filename>` (walk `<base>`
/// recursively, collect files named `<filename>`) and a plain path (included if it exists).
fn find_imports(pattern: &str) -> Vec<PathBuf> {
    if let Some((base, filename)) = pattern.split_once("/**/") {
        let mut out = Vec::new();
        walk_collect(Path::new(base), filename, &mut out);
        out.sort();
        out
    } else {
        let p = PathBuf::from(pattern);
        if p.exists() { vec![p] } else { Vec::new() }
    }
}

/// Recursively collect files named `filename` under `dir`.
fn walk_collect(dir: &Path, filename: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_collect(&path, filename, out);
        } else if path.file_name().is_some_and(|n| n == filename) {
            out.push(path);
        }
    }
}

/// The namespace for an imported hive's services: the project id from
/// `.../<project>/.adi/hive.yaml`, else the file's parent dir name, else `import`.
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

/// Whether two paths point at the same file (canonicalized), so a hive never imports itself.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

// MARK: resolution — from the parsed spec to what the daemon runs

/// One routing rule the proxy enforces: `Host: host` → `upstream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub host: String,
    pub upstream: SocketAddr,
}

/// Everything the daemon needs, derived from the spec: binds, routes, and skipped services.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub binds: Vec<SocketAddr>,
    pub routes: Vec<ResolvedRoute>,
    pub skipped: Vec<String>,
}

impl Hive {
    /// Load a hive.yaml and fan in every service reachable through its `imports`, so one hive can
    /// front-door an entire machine.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut hive = Self::parse_file(path)?;
        hive.apply_imports(path);
        Ok(hive)
    }

    /// Parse a single hive.yaml with no import expansion: rewrite `bash`…`` port commands into
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
        let strip_runners = running_as_root();
        let patterns = std::mem::take(&mut self.imports);
        for pattern in patterns {
            for file in find_imports(&expand_vars(&pattern)) {
                if same_file(&file, base) {
                    continue;
                }
                match Self::parse_file(&file) {
                    Ok(child) => {
                        self.merge_import(child, &import_namespace(&file), strip_runners);
                    }
                    Err(e) => {
                        warn!(file = %file.display(), error = %e, "skipping unreadable import")
                    }
                }
            }
        }
    }

    /// Merge one imported hive's services under `ns`, dropping their runners when
    /// `strip_runners`. An already-present key wins, so a local service is never overridden.
    fn merge_import(&mut self, child: Self, ns: &str, strip_runners: bool) {
        for (name, mut svc) in child.services {
            if strip_runners {
                svc.runner = None;
            }
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
            match svc.http_port() {
                Some(port) => routes.push(ResolvedRoute {
                    host: proxy.host.clone(),
                    upstream: SocketAddr::new(UPSTREAM_IP, port),
                }),
                None => skipped.push(format!("{name} (host {}): no HTTP port", proxy.host)),
            }
        }
        Resolved {
            binds,
            routes,
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
        root.merge_import(child.clone(), "proj", true);
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

        let mut user = Hive::default();
        user.merge_import(child, "proj", false);
        assert!(
            user.services["proj/app"].runner.is_some(),
            "an unprivileged hive keeps the runner so it can supervise it"
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
        // Pre-clean then exec so `docker run` becomes the supervised process.
        assert!(
            run.starts_with("docker rm -f adi-web >/dev/null 2>&1; exec docker run --rm --name adi-web"),
            "got: {run}"
        );
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
        // Image, then the overriding command (with the space-bearing arg quoted).
        assert!(
            run.trim_end().ends_with("nginx:1.27 nginx -g 'daemon off;'"),
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
