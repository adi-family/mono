//! `relay.toml` (`docs/webhook-relay.md` §6): one binary, one config file, `mode` picks the
//! behaviour. Every limit is optional in the file — an absent one resolves to its mode's own
//! default from the table in §6, which is why parsing goes through [`RawConfig`] (every limit
//! `Option<T>`) rather than `#[serde(default)]` on [`Config`] directly: the default a missing
//! field resolves to depends on `mode`, not on the field alone.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// The three modes (§6) — a relay operator picks exactly one per running instance; a node's own
/// `extra["relay"]` setting (§10) later picks a mode by picking which relay it registers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Public,
    Enterprise,
    SelfHost,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Public => "public",
            Self::Enterprise => "enterprise",
            Self::SelfHost => "self-host",
        })
    }
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Self::Public),
            "enterprise" => Ok(Self::Enterprise),
            "self-host" => Ok(Self::SelfHost),
            other => Err(format!("unknown mode {other:?}: expected public, enterprise, or self-host")),
        }
    }
}

/// The resolved limits for one running instance — every field always has a value, either the
/// operator's own or the mode's default (§6's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_endpoints_per_key: u32,
    pub max_body_bytes: u64,
    pub max_requests_per_minute_per_endpoint: u32,
    pub max_registrations_per_ip_per_day: u32,
    pub default_retention_secs: u64,
    pub max_retention_secs: u64,
    pub pull_batch_max: u32,
    pub pull_long_poll_secs: u64,
    /// Not in `relay.toml` (§6 names no knob for it) — the per-endpoint pending-record cap §7
    /// proposes a single number for (10,000) regardless of mode, so it isn't part of the
    /// mode-default table below.
    pub max_pending_per_endpoint: u64,
}

impl Limits {
    /// §6's table, literally — including the two places self-host's own column differs from
    /// simply reusing enterprise's numbers: `max_endpoints_per_key` and `max_retention_secs` are
    /// "unlimited" there (`authorized_keys` is already the gate), represented as `u32::MAX` /
    /// `u64::MAX` rather than a sentinel the rest of this crate would have to know about.
    #[must_use]
    pub fn defaults_for(mode: Mode) -> Self {
        match mode {
            Mode::Public => Self {
                max_endpoints_per_key: 5,
                max_body_bytes: 262_144,
                max_requests_per_minute_per_endpoint: 60,
                max_registrations_per_ip_per_day: 10,
                default_retention_secs: 300,
                max_retention_secs: 900,
                pull_batch_max: 50,
                pull_long_poll_secs: 25,
                max_pending_per_endpoint: 10_000,
            },
            Mode::Enterprise => Self {
                max_endpoints_per_key: 100,
                max_body_bytes: 2 << 20,
                max_requests_per_minute_per_endpoint: 600,
                max_registrations_per_ip_per_day: 10,
                default_retention_secs: 3600,
                max_retention_secs: 86_400,
                pull_batch_max: 200,
                pull_long_poll_secs: 25,
                max_pending_per_endpoint: 10_000,
            },
            Mode::SelfHost => Self {
                max_endpoints_per_key: u32::MAX,
                max_body_bytes: 2 << 20,
                max_requests_per_minute_per_endpoint: 600,
                max_registrations_per_ip_per_day: u32::MAX,
                default_retention_secs: 300,
                max_retention_secs: u64::MAX,
                pull_batch_max: 200,
                pull_long_poll_secs: 25,
                max_pending_per_endpoint: 10_000,
            },
        }
    }
}

/// Enterprise-mode licence gating (§6). **`public_key` is not in the task's own sample
/// `relay.toml` — added here, documented as a deviation in `docs/webhook-relay.md` §14 Q6: the
/// relay must verify the licence service's beam signature itself (the same "CLIENT ACCEPTANCE"
/// rule `adi-enterprise-beam-wire-contract-v1` states for every beam client), and there is
/// nowhere else to configure the key it verifies against.**
#[derive(Debug, Clone, Deserialize)]
pub struct LicenceConfig {
    pub service_url: String,
    /// Base64 of the 32 raw ed25519 public-key bytes the licence service signs verdicts with.
    pub public_key: String,
    #[serde(default = "default_recheck_interval")]
    pub recheck_interval_secs: u64,
}

fn default_recheck_interval() -> u64 {
    86_400
}

/// Self-host gating (§6).
#[derive(Debug, Clone, Deserialize)]
pub struct SelfHostConfig {
    pub authorized_keys_path: PathBuf,
    #[serde(default)]
    pub shared_secret_env: Option<String>,
    #[serde(default)]
    pub ip_allowlist: Vec<String>,
}

/// What `toml::from_str` parses directly: every limit optional, so a missing one can be told
/// apart from an operator's explicit `0`.
#[derive(Debug, Deserialize)]
struct RawConfig {
    mode: String,
    bind: String,
    db_path: PathBuf,
    hostname: String,
    #[serde(default)]
    limits: RawLimits,
    licence: Option<LicenceConfig>,
    self_host: Option<SelfHostConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLimits {
    max_endpoints_per_key: Option<u32>,
    max_body_bytes: Option<u64>,
    max_requests_per_minute_per_endpoint: Option<u32>,
    max_registrations_per_ip_per_day: Option<u32>,
    default_retention_secs: Option<u64>,
    max_retention_secs: Option<u64>,
    pull_batch_max: Option<u32>,
    pull_long_poll_secs: Option<u64>,
    max_pending_per_endpoint: Option<u64>,
}

impl RawLimits {
    fn resolve(self, mode: Mode) -> Limits {
        let d = Limits::defaults_for(mode);
        Limits {
            max_endpoints_per_key: self.max_endpoints_per_key.unwrap_or(d.max_endpoints_per_key),
            max_body_bytes: self.max_body_bytes.unwrap_or(d.max_body_bytes),
            max_requests_per_minute_per_endpoint: self
                .max_requests_per_minute_per_endpoint
                .unwrap_or(d.max_requests_per_minute_per_endpoint),
            max_registrations_per_ip_per_day: self
                .max_registrations_per_ip_per_day
                .unwrap_or(d.max_registrations_per_ip_per_day),
            default_retention_secs: self.default_retention_secs.unwrap_or(d.default_retention_secs),
            max_retention_secs: self.max_retention_secs.unwrap_or(d.max_retention_secs),
            pull_batch_max: self.pull_batch_max.unwrap_or(d.pull_batch_max),
            pull_long_poll_secs: self.pull_long_poll_secs.unwrap_or(d.pull_long_poll_secs),
            max_pending_per_endpoint: self.max_pending_per_endpoint.unwrap_or(d.max_pending_per_endpoint),
        }
    }
}

/// The resolved configuration one running instance uses.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub hostname: String,
    pub limits: Limits,
    pub licence: Option<LicenceConfig>,
    pub self_host: Option<SelfHostConfig>,
}

impl Config {
    /// Parse and resolve `text` (the contents of a `relay.toml`).
    ///
    /// # Errors
    /// A TOML parse error, an unknown `mode`, an unparseable `bind` address, or a mode missing
    /// the config section it needs (`[licence]` for enterprise, `[self_host]` for self-host).
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let raw: RawConfig = toml::from_str(text)?;
        let mode: Mode = raw.mode.parse().map_err(|e: String| anyhow::anyhow!(e))?;
        let bind: SocketAddr = raw
            .bind
            .parse()
            .map_err(|e| anyhow::anyhow!("bad bind address {:?}: {e}", raw.bind))?;

        match mode {
            Mode::Enterprise if raw.licence.is_none() => {
                anyhow::bail!("mode = \"enterprise\" needs a [licence] section");
            }
            Mode::SelfHost if raw.self_host.is_none() => {
                anyhow::bail!("mode = \"self-host\" needs a [self_host] section");
            }
            _ => {}
        }

        Ok(Self {
            mode,
            bind,
            db_path: raw.db_path,
            hostname: raw.hostname,
            limits: raw.limits.resolve(mode),
            licence: raw.licence,
            self_host: raw.self_host,
        })
    }

    /// Load and parse from a file on disk.
    ///
    /// # Errors
    /// An I/O error reading `path`, or anything [`Self::parse`] can fail with.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// The public URL `register` (§5.2) echoes back for a freshly minted endpoint.
    #[must_use]
    pub fn endpoint_url(&self, endpoint_id: &str) -> String {
        format!("https://{}/h/{endpoint_id}", self.hostname)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_TOML: &str = r#"
mode     = "public"
bind     = "0.0.0.0:8080"
db_path  = "/data/relay.db"
hostname = "eu.webhooks.withadi.dev"
"#;

    #[test]
    fn a_minimal_public_config_resolves_public_defaults() {
        let cfg = Config::parse(PUBLIC_TOML).expect("parses");
        assert_eq!(cfg.mode, Mode::Public);
        assert_eq!(cfg.bind, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.limits.max_endpoints_per_key, 5);
        assert_eq!(cfg.limits.max_body_bytes, 262_144);
        assert_eq!(cfg.limits.max_retention_secs, 900);
        assert_eq!(cfg.limits.pull_batch_max, 50);
    }

    #[test]
    fn an_explicit_limit_overrides_its_modes_default() {
        let toml = format!(
            "{PUBLIC_TOML}\n[limits]\nmax_body_bytes = 1000\n"
        );
        let cfg = Config::parse(&toml).expect("parses");
        assert_eq!(cfg.limits.max_body_bytes, 1000);
        // Everything else untouched still resolves to public's default.
        assert_eq!(cfg.limits.max_endpoints_per_key, 5);
    }

    #[test]
    fn enterprise_defaults_are_more_generous_than_public() {
        let toml = r#"
mode = "enterprise"
bind = "0.0.0.0:8080"
db_path = "/data/relay.db"
hostname = "eu.ent.webhooks.withadi.dev"

[licence]
service_url = "https://licence.withadi.dev"
public_key = "JiT26+wj0nxniW3wH5KvEC1fDwl2clxgAiBkUMxLfhM="
"#;
        let cfg = Config::parse(toml).expect("parses");
        assert_eq!(cfg.limits.max_endpoints_per_key, 100);
        assert_eq!(cfg.limits.max_retention_secs, 86_400);
        assert_eq!(cfg.licence.unwrap().recheck_interval_secs, 86_400);
    }

    #[test]
    fn enterprise_without_a_licence_section_is_refused() {
        let toml = r#"
mode = "enterprise"
bind = "0.0.0.0:8080"
db_path = "/data/relay.db"
hostname = "x"
"#;
        assert!(Config::parse(toml).is_err());
    }

    #[test]
    fn self_host_without_a_self_host_section_is_refused() {
        let toml = r#"
mode = "self-host"
bind = "0.0.0.0:8080"
db_path = "/data/relay.db"
hostname = "x"
"#;
        assert!(Config::parse(toml).is_err());
    }

    #[test]
    fn self_host_parses_its_authorized_keys_path_and_optional_extras() {
        let toml = r#"
mode = "self-host"
bind = "0.0.0.0:8080"
db_path = "/data/relay.db"
hostname = "hooks.example.com"

[self_host]
authorized_keys_path = "/etc/webhook-relay/authorized_keys"
shared_secret_env = "RELAY_SHARED_SECRET"
ip_allowlist = ["10.0.0.0/8"]
"#;
        let cfg = Config::parse(toml).expect("parses");
        let sh = cfg.self_host.expect("self_host section");
        assert_eq!(sh.authorized_keys_path, PathBuf::from("/etc/webhook-relay/authorized_keys"));
        assert_eq!(sh.shared_secret_env, Some("RELAY_SHARED_SECRET".to_string()));
        assert_eq!(sh.ip_allowlist, vec!["10.0.0.0/8".to_string()]);
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        let toml = PUBLIC_TOML.replace("public", "pro");
        assert!(Config::parse(&toml).is_err());
    }

    #[test]
    fn endpoint_url_is_built_from_the_hostname() {
        let cfg = Config::parse(PUBLIC_TOML).expect("parses");
        assert_eq!(
            cfg.endpoint_url("wh_abc"),
            "https://eu.webhooks.withadi.dev/h/wh_abc"
        );
    }
}
