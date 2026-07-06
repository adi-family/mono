//! The single hive config, loaded from `~/.adi/mono/hive/hive.yaml`.
//!
//! This parses the nakit-yok **hive.yaml** format, reading the slice adi-hive acts on:
//! the reverse-proxy fields — `proxy.bind`, and per service its `proxy.host` + HTTP port
//! (`rollout.recreate.ports.http`) — plus the fields needed to *run* a service locally:
//! `runner.script` (the command + `working_dir`), `environment.static`, and `restart`.
//! Everything else in the wider hive spec (healthcheck, hooks, `depends_on`, defaults,
//! observability, …) is accepted-but-ignored: we deliberately do *not*
//! `deny_unknown_fields`, so a full hive.yaml parses cleanly here.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use adi_ports_manager::Ports;
use anyhow::Context as _;
use serde::Deserialize;
use tracing::warn;

const ADI_DIR_ENV: &str = "ADI_DIR";
const DEFAULT_ADI_DIR: &str = ".adi";

/// Upstreams are always local — a service's HTTP port on loopback.
const UPSTREAM_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The port-map key that names a service's HTTP port (what the proxy targets).
const HTTP_PORT_KEY: &str = "http";

/// The ports-manager lease for adi-hive's own front-door port, used when no explicit
/// `proxy.bind` is configured. `proxy.name` overrides the service part (so several
/// manager-bound hives can coexist with distinct leases).
const FRONT_DOOR_NAME: &str = "adi-hive";
const FRONT_DOOR_KEY: &str = "front-door";

// MARK: parsed hive.yaml (proxy-relevant subset)

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Hive {
    #[serde(default)]
    pub proxy: ProxyBinds,
    #[serde(default)]
    pub services: BTreeMap<String, ServiceSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProxyBinds {
    #[serde(default)]
    pub bind: Vec<SocketAddr>,
    /// Optional name for the front door. When the bind port is manager-allocated (no
    /// explicit `bind`), this is the ports-manager lease's service key. Defaults to
    /// `adi-hive`.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceSpec {
    #[serde(default)]
    pub proxy: Option<ServiceProxy>,
    #[serde(default)]
    pub rollout: Option<Rollout>,
    /// How to actually run the service locally. Only `type: script` is supported;
    /// other runner types parse but are skipped (their `script` is absent).
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
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
}

/// The `runner:` block. We model only the `script` runner (a shell command); a runner
/// of any other `type` deserializes with `script == None` and is skipped.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Runner {
    #[serde(default)]
    pub script: Option<Script>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    /// The shell command to run (executed via `sh -c`).
    pub run: String,
    /// Where to run it, relative to the hive.yaml's directory (or absolute). Defaults
    /// to the config directory itself.
    #[serde(default)]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Environment {
    #[serde(default, rename = "static")]
    pub static_env: BTreeMap<String, String>,
}

impl ServiceSpec {
    /// The port the proxy forwards to: the `http` port if named, else the sole port
    /// if there's exactly one, else `None` (nothing sensible to route to).
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

    /// Set the service's `http` port, creating the `rollout.recreate.ports` path if
    /// needed. Used to record a port allocated from the ports manager.
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

/// A single service resolved to a launchable, self-contained runner: the exact shell
/// command (templates expanded), the absolute working directory, the environment to
/// inject, and what to do when it exits.
#[derive(Debug, Clone)]
pub struct RunnerSpec {
    pub name: String,
    pub run: String,
    pub working_dir: PathBuf,
    pub env: Vec<(String, String)>,
    pub restart: RestartPolicy,
}

impl Hive {
    /// For every proxied or script-runner service that doesn't already declare an HTTP
    /// port, reserve a stable one from the ports manager (a durable lease keyed by the
    /// service name) and fill it in — so the proxy route and the runner's `$PORT` both
    /// use the same manager-allocated port. Explicitly-declared ports are left as-is.
    /// Returns the `(service, port)` pairs allocated, for logging.
    ///
    /// Best-effort: a service whose allocation fails is left without a port (it will be
    /// skipped by routing) rather than aborting the whole config.
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

    /// Take adi-hive's own front-door listen port from the ports manager: when no
    /// explicit `proxy.bind` is set, reserve a stable port (a durable lease keyed by
    /// `proxy.name`, default `adi-hive`) and bind loopback on it — so the proxy's port,
    /// like the services', comes from the manager instead of a hard-coded default. An
    /// explicit `proxy.bind` (e.g. the `127.0.0.53:80` front door) is left untouched.
    /// Returns the reserved port, if one was taken.
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

    /// Every service that declares a `script` runner, resolved for launch. `base_dir`
    /// (the hive.yaml's directory) anchors relative `working_dir`s. Services without a
    /// script runner are omitted.
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

/// Build the runner's environment: `PORT` = the http/sole port (the common single-port
/// convention), a `PORT_<KEY>` for every named port, then the service's static env last
/// so an explicit value wins over the injected defaults.
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

/// Resolve a runner's working directory: absolute paths as-is, relative ones against
/// `base_dir`, and `None` to `base_dir` itself.
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

/// Substitute `{{ runtime.port.<key> }}` placeholders (any inner spacing) with the
/// named port. Unknown keys and malformed placeholders are left verbatim.
fn expand_templates(input: &str, ports: &BTreeMap<String, u16>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            // No closing braces: emit the rest literally and stop.
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
            // Unknown key or non-port placeholder: leave it verbatim.
            out.push_str("{{");
            out.push_str(&after[..close]);
            out.push_str("}}");
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

// MARK: resolution — from the parsed spec to what the daemon runs

/// One routing rule the proxy enforces: `Host: host` → `upstream`.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub host: String,
    pub upstream: SocketAddr,
}

/// Everything the daemon needs, derived from the spec: where to listen, where to
/// route, and which proxied services were skipped (no usable HTTP port).
#[derive(Debug, Clone)]
pub struct Resolved {
    pub binds: Vec<SocketAddr>,
    pub routes: Vec<ResolvedRoute>,
    pub skipped: Vec<String>,
}

impl Hive {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let hive: Self = serde_yaml_ng::from_str(&raw)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(hive)
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
                continue; // not fronted by the proxy
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

/// The single canonical config location: `$HOME/$ADI_DIR/mono/hive/hive.yaml`
/// (default `~/.adi/mono/hive/hive.yaml`). Mirrors `adi-core`'s `paths::support_dir`
/// (`$HOME/$ADI_DIR/mono`) so adi-hive stays a standalone binary with no
/// workspace-internal dependency, exactly like adi-dns.
#[must_use]
pub fn default_config_path() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let adi_dir = std::env::var(ADI_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_ADI_DIR.to_string());
    home.join(adi_dir)
        .join("mono")
        .join("hive")
        .join("hive.yaml")
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

        // Two proxied services become routes (BTreeMap → alphabetical by service name).
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

        // postgres has no `proxy:` → not a route, not skipped.
        assert!(r.skipped.is_empty(), "postgres is silently not-routed");
    }

    #[test]
    fn ignores_unknown_hive_fields() {
        // A service laden with fields adi-hive doesn't model still parses.
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
        // The {{runtime.port.http}} template is expanded to the declared port.
        assert_eq!(frontend.run, "serve --port 8010");
        // A relative working_dir is anchored at the config directory.
        assert_eq!(frontend.working_dir, Path::new("/project/web/frontend"));
        // PORT = the http port; PORT_HTTP mirrors the named slot.
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
        // Arbitrary inner spacing is tolerated.
        assert_eq!(
            expand_templates("p={{ runtime.port.http }}", &ports),
            "p=8010"
        );
        // Unknown key and a stray opener are left verbatim.
        assert_eq!(
            expand_templates("{{runtime.port.db}} and {{oops", &ports),
            "{{runtime.port.db}} and {{oops"
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

        // A ports manager with an isolated temp registry.
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

        // The allocated port now drives the route AND the runner's PORT env.
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

        // No proxy.bind -> the port is reserved from the manager, bound on loopback.
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

        // An explicit bind is left untouched.
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
