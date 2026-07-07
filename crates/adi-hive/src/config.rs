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
    /// How to run the service locally; only `type: script` is supported (others parse but are skipped).
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

/// The `runner:` block; only the `script` runner is modelled (other types get `script == None`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Runner {
    #[serde(default)]
    pub script: Option<Script>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    /// The shell command to run (executed via `sh -c`).
    pub run: String,
    /// Where to run it, relative to the hive.yaml's directory (or absolute); defaults to that directory.
    #[serde(default)]
    pub working_dir: Option<String>,
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
                .and_then(|r| r.script.as_ref())
                .is_some()
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
#[derive(Debug, Clone)]
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

    /// Every service that declares a `script` runner, resolved for launch; `base_dir` anchors relative `working_dir`s.
    #[must_use]
    pub fn runners(&self, base_dir: &Path) -> Vec<RunnerSpec> {
        let mut out = Vec::new();
        for (name, svc) in &self.services {
            let Some(script) = svc.runner.as_ref().and_then(|r| r.script.as_ref()) else {
                continue;
            };
            let ports = svc.ports();
            out.push(RunnerSpec {
                name: name.clone(),
                run: expand_templates(&script.run, ports),
                working_dir: resolve_working_dir(base_dir, script.working_dir.as_deref()),
                env: build_env(svc, ports),
                restart: RestartPolicy::parse(svc.restart.as_deref()),
            });
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

/// Substitute config variables in an import pattern: `$ADI_PROJECTS_DIR` (the projects module
/// dir, honoring `$ADI_DIR`) and `$HOME`.
fn expand_vars(pattern: &str) -> String {
    let cfg = adi_config::Config::open();
    let projects = cfg.module("projects").dir().to_string_lossy().into_owned();
    let mut out = pattern.replace("$ADI_PROJECTS_DIR", &projects);
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
    let ns = if parent.and_then(Path::file_name).is_some_and(|n| n == ".adi") {
        parent.and_then(Path::parent).and_then(Path::file_name)
    } else {
        parent.and_then(Path::file_name)
    };
    ns.map_or_else(|| "import".to_string(), |n| n.to_string_lossy().into_owned())
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
#[derive(Debug, Clone)]
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
        let hive: Self = adi_ports_manager::with_commands(commands, || {
            serde_yaml_ng::from_str(&yaml)
        })
        .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(hive)
    }

    /// Expand each `imports` glob and merge the matched hive.yaml files' services in as
    /// **proxy-only** routes — keyed `<project>/<service>`, with runners stripped (the front door
    /// routes them; it does not run them, so a root front door never spawns user processes).
    /// Best-effort: an unreadable or unparsable import is logged and skipped, never fatal.
    fn apply_imports(&mut self, base: &Path) {
        let patterns = std::mem::take(&mut self.imports);
        for pattern in patterns {
            for file in find_imports(&expand_vars(&pattern)) {
                if same_file(&file, base) {
                    continue; // never import ourselves
                }
                match Self::parse_file(&file) {
                    Ok(child) => {
                        let ns = import_namespace(&file);
                        for (name, mut svc) in child.services {
                            svc.runner = None;
                            self.services.entry(format!("{ns}/{name}")).or_insert(svc);
                        }
                    }
                    Err(e) => warn!(file = %file.display(), error = %e, "skipping unreadable import"),
                }
            }
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
    fn imports_fan_in_project_services_namespaced_and_proxy_only() {
        // A temp tree: <base>/proj/.adi/hive.yaml with a proxied service that has a runner and an
        // integer port (no ports-manager command, so the test touches no registry).
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
        // A parent hive that imports the temp tree via a `<base>/**/hive.yaml` glob.
        let parent = base.join("parent.yaml");
        std::fs::write(
            &parent,
            format!("imports:\n  - {}/**/hive.yaml\n", base.display()),
        )
        .unwrap();

        let hive = Hive::load(&parent).expect("load with imports");
        // Fanned in under `<project>/<service>`, proxy kept, runner stripped (proxy-only).
        let svc = hive.services.get("proj/app").expect("imported service present");
        assert_eq!(svc.proxy.as_ref().expect("proxy").host, "proj.adi");
        assert_eq!(svc.http_port(), Some(9123));
        assert!(svc.runner.is_none(), "imported services are proxy-only");
        // resolve() routes the imported host to the imported port.
        let routes = hive.resolve().routes;
        assert!(
            routes
                .iter()
                .any(|r| r.host == "proj.adi" && r.upstream.port() == 9123)
        );
        let _ = std::fs::remove_dir_all(&base);
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
        // A proxied service with a runner but NO declared port.
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

        // Idempotent: a second pass allocates nothing (the port is already set).
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
        // Idempotent (same lease) and a no-op once bound.
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
    fn a_runner_with_no_script_is_skipped() {
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
}
