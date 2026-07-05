//! The single hive config, loaded from `~/.adi/mono/hive/hive.yaml`.
//!
//! This parses the nakit-yok **hive.yaml** format, but reads only the slice the
//! reverse proxy needs — `proxy.bind`, and per service its `proxy.host` + HTTP port
//! (`rollout.recreate.ports.http`). Every other field of the wider hive spec (runner,
//! healthcheck, environment, hooks, `depends_on`, defaults, observability, …) is
//! accepted-but-ignored: we deliberately do *not* `deny_unknown_fields`, so a full
//! hive.yaml parses cleanly here.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

const ADI_DIR_ENV: &str = "ADI_DIR";
const DEFAULT_ADI_DIR: &str = ".adi";

/// Upstreams are always local — a service's HTTP port on loopback.
const UPSTREAM_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The port-map key that names a service's HTTP port (what the proxy targets).
const HTTP_PORT_KEY: &str = "http";

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
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceSpec {
    #[serde(default)]
    pub proxy: Option<ServiceProxy>,
    #[serde(default)]
    pub rollout: Option<Rollout>,
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

impl ServiceSpec {
    /// The port the proxy forwards to: the `http` port if named, else the sole port
    /// if there's exactly one, else `None` (nothing sensible to route to).
    fn http_port(&self) -> Option<u16> {
        let ports = &self.rollout.as_ref()?.recreate.as_ref()?.ports;
        if let Some(port) = ports.get(HTTP_PORT_KEY) {
            return Some(*port);
        }
        if ports.len() == 1 {
            return ports.values().next().copied();
        }
        None
    }
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
    home.join(adi_dir).join("mono").join("hive").join("hive.yaml")
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
        assert!(
            p.ends_with("mono/hive/hive.yaml"),
            "got {}",
            p.display()
        );
    }
}
