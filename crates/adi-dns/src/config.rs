//! Runtime configuration, loaded from a TOML file (see `adi-dns.toml`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: IpAddr,

    /// Unprivileged; avoid `15353` (adi.hive) and `5353` (mDNS).
    #[serde(default = "default_preferred_port")]
    pub preferred_port: u16,

    #[serde(default = "default_fallback_ports")]
    pub fallback_ports: Vec<u16>,

    #[serde(default = "default_domain")]
    pub domain: String,

    #[serde(default = "default_upstreams")]
    pub upstreams: Vec<SocketAddr>,

    #[serde(default)]
    pub manage_os_routing: bool,

    #[serde(default)]
    pub overrides: Vec<OverrideZone>,

    #[serde(default)]
    pub status_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideZone {
    pub suffix: String,
    pub address: IpAddr,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Self = toml::from_str(&raw)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(cfg)
    }

    /// Windows NRPT can't target a custom port, so the resolver must bind `:53` there.
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
            status_file: None,
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
