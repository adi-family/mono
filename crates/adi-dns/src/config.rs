//! Runtime configuration, loaded from a TOML file (see `adi-dns.toml`).
//!
//! The resolver owns a single TLD (`domain`, e.g. `adi`) and answers it locally
//! (split-DNS) while forwarding everything else. It binds an **unprivileged** port
//! by preference and only falls back to others if that one is busy, so it works on
//! any machine that starts it without fighting over a port.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Loopback address to bind. Default `127.0.0.1`.
    #[serde(default = "default_bind_addr")]
    pub bind_addr: IpAddr,

    /// Preferred (unprivileged) port. Default `10053` — high enough to need no
    /// root, below the ephemeral range, and clear of `15353` (ADI DNS / adi.hive)
    /// and `5353` (mDNS).
    #[serde(default = "default_preferred_port")]
    pub preferred_port: u16,

    /// Ports to try, in order, if `preferred_port` is already taken.
    #[serde(default = "default_fallback_ports")]
    pub fallback_ports: Vec<u16>,

    /// The TLD this resolver owns and registers with the OS (e.g. `adi`).
    #[serde(default = "default_domain")]
    pub domain: String,

    /// Upstream resolvers that every non-override query is forwarded to.
    #[serde(default = "default_upstreams")]
    pub upstreams: Vec<SocketAddr>,

    /// When true, install the OS route for `domain` at startup and remove it at
    /// shutdown (macOS `/etc/resolver`, Linux systemd-resolved, Windows NRPT).
    /// Requires admin/root; degrades to a warning if it can't.
    #[serde(default)]
    pub manage_os_routing: bool,

    /// Local override zones. If empty, defaults to `domain -> 127.0.0.1`.
    #[serde(default)]
    pub overrides: Vec<OverrideZone>,
}

/// A single split-DNS override: everything under `suffix` resolves to `address`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideZone {
    /// Domain suffix, e.g. `"adi"` matches `adi.` and `*.adi.`.
    pub suffix: String,
    /// The address every name under `suffix` resolves to.
    pub address: IpAddr,
}

impl Config {
    /// Load and parse the config file at `path`.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Self = toml::from_str(&raw)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(cfg)
    }

    /// The ordered list of ports to attempt.
    ///
    /// On Windows the OS route (NRPT) cannot target a custom port, so the resolver
    /// must bind `:53` there regardless of the preferred/fallback ports.
    pub fn effective_ports(&self) -> Vec<u16> {
        if cfg!(windows) {
            vec![53]
        } else {
            let mut ports = Vec::with_capacity(1 + self.fallback_ports.len());
            ports.push(self.preferred_port);
            ports.extend(self.fallback_ports.iter().copied());
            ports
        }
    }

    /// Override zones, defaulting to `domain -> 127.0.0.1` when none are given so
    /// that a minimal config (`domain = "adi"`) is already a working resolver.
    pub fn overrides_or_default(&self) -> Vec<OverrideZone> {
        if self.overrides.is_empty() {
            vec![OverrideZone {
                suffix: self.domain.clone(),
                address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            }]
        } else {
            self.overrides.clone()
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            preferred_port: default_preferred_port(),
            fallback_ports: default_fallback_ports(),
            domain: default_domain(),
            upstreams: default_upstreams(),
            manage_os_routing: false,
            overrides: Vec::new(),
        }
    }
}

fn default_bind_addr() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn default_preferred_port() -> u16 {
    10053
}

fn default_fallback_ports() -> Vec<u16> {
    vec![10153, 24053]
}

fn default_domain() -> String {
    "adi".to_string()
}

fn default_upstreams() -> Vec<SocketAddr> {
    vec![
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
    ]
}
