//! The `adi-dns` resolver as an ADI service, split by privilege so the on/off toggle
//! never needs a password.
//!
//! - **macOS:** an unprivileged per-user resolver `LaunchAgent` (`127.0.0.1:10053`), plus a root
//!   front-door `LaunchDaemon` (`adi-hive` on `127.0.0.53:80`) installed once via one
//!   Authorization prompt; `.adi` is routed with an `/etc/resolver/adi` file.
//! - **Linux:** an unprivileged per-user resolver unit (`127.0.0.1:10053`) and a front-door unit
//!   *beside it, under the same `systemd --user` manager* — deliberately **not** a root system
//!   unit. A node runs everything as an ordinary user (`docs/fleet.md` §6,
//!   `apps/linux/README.md`); a root daemon would have to be told whose `~/.adi/mono` to read,
//!   would leave root-owned files in it, and would move the one process that terminates every
//!   `.adi` connection outside the privilege boundary the rest of the node keeps. So the front
//!   door stays unprivileged and is granted exactly one thing — `CAP_NET_BIND_SERVICE` on the
//!   `adi-hive` binary — which is what lets it bind `:80`/`:443`. `.adi` is routed with a
//!   **`systemd-resolved` drop-in** (`Domains=~adi`, routing-only), in the file format and at the
//!   path `adi-dns` itself defines.
//! - **Windows:** an unprivileged per-user resolver task (`127.0.0.1:53` — NRPT can only redirect a
//!   whole namespace, not a port), plus a per-user front-door task (`adi-hive` on `127.0.0.53:80` —
//!   Windows needs no loopback alias and does not reserve low ports for admin). `.adi` is routed
//!   with a **DNS Client NRPT rule**, the one step that needs a single UAC elevation.
//!
//! The privileged/routing surface (`install_route`, `update_frontdoor`, `remove_route`,
//! `route_installed`) is split per-OS; the config/YAML rendering below it is shared and unit-tested.
//!
//! # What the root daemon is allowed to run
//!
//! Only the macOS front door is a **root** daemon, and a root daemon is worth no more than the
//! file it executes: whoever can rewrite that program — or rename a different file over it, which
//! needs only a writable *directory* — runs code as root. Here that is not even a race to win.
//! [`frontdoor_env`] sets `ADI_WATCH_SELF=1` so the running front door polls its own inode and
//! exits when the file changes, and `KeepAlive` starts the replacement: a plain
//! `cargo build --release -p adi-hive`, an npm postinstall, or a drive-by that can write one file
//! is root within about sixty seconds, with no prompt.
//!
//! [`hive_binary_path`] resolves the program as a sibling of whatever binary is doing the
//! installing, so running `adi enable` from a repo build is all it takes to put
//! `~/…/target/release/adi-hive` into a root plist. So [`write_frontdoor_artifacts`] now refuses
//! to stage a plist whose program, or any directory above it, is owned by a non-root user or is
//! group/other-writable. It names the offending component and stops before the Authorization
//! prompt: nothing is written and nothing is asked for.
//!
//! Refuse, never rewrite. Silently substituting the bundle path would surprise a developer who
//! repointed the daemon on purpose, which is a documented workflow (`CLAUDE.md`). The escape hatch
//! is [`ALLOW_UNSAFE_PROGRAM_ENV`] and has to be asked for by name — `ADI_HIVE_BIN` chooses the
//! path, which is a different decision from accepting that an ordinary user may rewrite it.
//!
//! **This applies to root daemons only.** The Linux user unit and the Windows per-user task run as
//! the same account that owns the binary, which is no boundary at all, and they never call this.
//!
//! The split is per **`target_os`**, not `unix`. Linux *is* unix: while the macOS branch carried
//! `#[cfg(unix)]` a Linux build compiled it and then shelled out to an `osascript`, a `launchctl`
//! and a `/Library/LaunchDaemons` that do not exist there — and because `proc::run_admin`'s result
//! was dropped on the floor, `adi-mono up` **exited 0 having done nothing at all**. No prompt, no
//! route, no front door, no error. Every privileged step below now reports what it did, or exactly
//! what the operator has to run instead.
//!
//! Nothing on Linux ever prompts: the privileged steps go through `sudo -n`, which fails
//! immediately rather than asking. A node is installed over ssh and then left alone, so a command
//! that *can* block on a password is a command that hangs the install.
//!
//! Everything renderable on the Linux path — the drop-in, the commands, the status line — is a
//! pure function in `linux_plan`, compiled under `cfg(test)` on every host. That is what makes
//! a node's privileged steps testable from a macOS checkout, the same trick `launchd.rs` and
//! `adi-dns/src/os_routing.rs` use.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use adi_config::Flavor;
use serde::{Deserialize, Serialize};

use crate::launchd;
use crate::paths;
#[cfg(any(unix, windows))]
use crate::proc;
use crate::service::{Action, Service};
use crate::status::DaemonStatus;

/// The TLD this install serves — `adi`, or whatever the process's [`Flavor`] says.
///
/// An accessor rather than a constant because it is the first thing two installs on the same
/// machine must differ in: the resolver zone, the `/etc/resolver` file, every generated
/// hostname and the front door's host list are all derived from it, so making it a function
/// makes all of them follow at once.
fn domain() -> &'static str {
    &Flavor::current().domain
}

/// The resolver's listen port. macOS/Linux take a high port from the flavour and route
/// `.<domain>` to it out-of-band. Windows must use `53`, because an NRPT rule redirects a
/// namespace to a nameserver *address* with no port field — there it is not a free choice, and
/// so not the flavour's to make.
#[cfg(not(windows))]
fn port() -> u16 {
    Flavor::current().resolver_port
}
#[cfg(windows)]
fn port() -> u16 {
    53
}

/// The address [`render_config`] tells the resolver to bind — and therefore the address the OS
/// route has to point *at*. One constant, so a route can never be written for an address the
/// resolver does not answer on.
const RESOLVER_BIND: &str = "127.0.0.1";

pub(crate) fn label() -> String {
    Flavor::current().label("dns")
}

/// Kept off `127.0.0.1` so `:80` never collides with anything else serving there — and one
/// alias per flavour, so two installs' front doors do not collide with each other either.
fn frontdoor_addr() -> Ipv4Addr {
    Flavor::current().frontdoor_addr
}
const FRONTDOOR_PORT: u16 = 80;
/// The HTTPS front door. Same privileged-port story as [`FRONTDOOR_PORT`] — fine, because the
/// front-door daemon already runs as root; an unprivileged hive just logs a skipped bind.
const FRONTDOOR_TLS_PORT: u16 = 443;
fn frontdoor_label() -> String {
    Flavor::current().label("dns-landing")
}

/// Where [`Dns::front_door_answering`] knocks: the same address a browser reaches for `.adi`.
fn frontdoor_probe_addr() -> SocketAddr {
    SocketAddr::from((frontdoor_addr(), FRONTDOOR_PORT))
}

/// How long that knock waits.
///
/// It has to be a *timeout* rather than a plain connect, and the reason is the failure it
/// exists to catch: the front door is also what aliases its address onto `lo0` (adi-hive's
/// `ensure_loopback_alias`), so on a machine where it never started, that address belongs to no
/// interface and macOS **drops** packets to it instead of refusing them. A bare
/// `TcpStream::connect` there blocks for the OS default — over a minute — which is the same
/// reason `http://app.adi/` loads forever in a browser instead of failing.
///
/// A loopback handshake against a live listener costs microseconds, so this budget is only
/// ever spent on the broken machine, and 250 ms leaves three orders of magnitude of headroom
/// for a loaded one.
const FRONTDOOR_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// The budget for the *second* knock, the one asked before raising a password prompt. Longer
/// on purpose: a false negative on the cheap probe costs a spurious button, but a false
/// negative here costs an admin prompt on a machine that was working.
const FRONTDOOR_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The loopback port the local **mesh gateway** listens on — where the front door hands every
/// `*.n.adi` request (`docs/fleet.md` §3).
///
/// Re-exported, not defined here. The number has to be agreed by two crates that cannot see each
/// other — this one writes it into the generated front door's `proxy.mesh_gateway`, and
/// `adi-mesh` binds the listener — so it lives in [`adi_config`], which both already depend on,
/// and the compiler keeps them in step instead of a comment asking them to. The reasoning for
/// the value is documented there.
pub use adi_config::{MESH_GATEWAY_PORT, mesh_gateway_addr};

// macOS-only: the root front-door `LaunchDaemon` lives at a system path with a system log.
// Both are named after the label, so a second install's root daemon can never be mistaken for
// -- or worse, installed over -- the real one's.
#[cfg(target_os = "macos")]
fn frontdoor_plist() -> String {
    format!("/Library/LaunchDaemons/{}.plist", frontdoor_label())
}

/// The real install keeps the log path it has always had; anything else is namespaced. Moving
/// the release log would orphan whatever is already tailing it, for no gain.
#[cfg(target_os = "macos")]
fn frontdoor_log() -> String {
    let flavour = Flavor::current();
    if flavour.is_release() {
        "/Library/Logs/adi-hive-frontdoor.log".to_string()
    } else {
        format!("/Library/Logs/{}.log", frontdoor_label())
    }
}

/// The binary the **root** daemon runs: a root-owned copy of `adi-hive`, put here by the
/// privileged install.
///
/// Deliberately not the one inside the app bundle, and this is the difference between a front
/// door that installs and one that does not. A bundle dragged into `/Applications` belongs to
/// the user who dragged it — uid 501, not root — so [`root_program_objection`] refused to name it
/// in a root daemon's `ProgramArguments`, and refused *before* the password prompt. Correctly:
/// with `ADI_WATCH_SELF` and `KeepAlive`, whoever may rewrite that file is root within the
/// minute. But it left macOS unable to install or repair its own front door at all from 1.0.0
/// onwards, silently, on every machine whose daemon did not predate the check.
///
/// So the privileged step copies the bundle's binary here once, as root, and the daemon runs the
/// copy. What is left is the window between the check and the copy — a race an attacker must win
/// while the operator is typing their password — instead of a file they may replace at leisure,
/// forever, on a machine nobody is watching.
///
/// `/Library/Application Support` is `root:admin 0755` on macOS: root-owned and *not*
/// group-writable, so it passes the same check that `/Applications` (0775) fails.
#[cfg(target_os = "macos")]
fn frontdoor_program_path() -> String {
    format!(
        "/Library/Application Support/{}/adi-hive",
        Flavor::current().app_name
    )
}

// MARK: file locations (free helpers — all state is on disk / in the OS supervisor)

/// The `dns` module directory inside a *given* store. The `_in` suffix runs through every
/// helper that touches disk here: the plain form resolves the real `~/.adi/mono`, this one takes
/// the store as an argument so a test can hand it a temp dir (`Config::with_root`) and never go
/// near the machine's live front door.
fn service_dir_in(store: &adi_config::Config) -> PathBuf {
    store.module("dns").dir().to_path_buf()
}
fn service_dir() -> PathBuf {
    service_dir_in(&adi_config::Config::open())
}
fn config_path() -> PathBuf {
    service_dir().join("adi-dns.toml")
}
fn status_file() -> PathBuf {
    // A resolver-specific name: the front-door adi-hive writes its OWN `status.json` in this
    // same dir (it sits beside `hive-frontdoor.yaml`), so sharing the name makes the two
    // clobber each other — the GUI then misreads the proxy's status as the resolver's, its
    // shape doesn't match, and the service shows a stuck "starting…". Keep them separate.
    service_dir().join("resolver.json")
}
fn frontdoor_config_path_in(store: &adi_config::Config) -> PathBuf {
    service_dir_in(store).join("hive-frontdoor.yaml")
}
fn frontdoor_config_path() -> PathBuf {
    frontdoor_config_path_in(&adi_config::Config::open())
}

// macOS-only route/daemon artifact paths.
#[cfg(target_os = "macos")]
fn stage_path() -> PathBuf {
    let domain = domain();
    service_dir().join(format!("resolver-{domain}"))
}
#[cfg(target_os = "macos")]
fn resolver_file() -> PathBuf {
    let domain = domain();
    PathBuf::from(format!("/etc/resolver/{domain}"))
}
#[cfg(target_os = "macos")]
fn frontdoor_plist_stage() -> PathBuf {
    let frontdoor_label = frontdoor_label();
    service_dir().join(format!("{frontdoor_label}.plist"))
}

// Linux-only: the drop-in is staged unprivileged inside the store and *copied* into place by the
// one privileged command, exactly as macOS stages `/etc/resolver/adi`. Staging first is what keeps
// the root-owned step down to `cp` + `chmod`, with nothing to quote and nothing to interpolate.
#[cfg(target_os = "linux")]
fn stage_path() -> PathBuf {
    service_dir().join(format!("resolved-{}.conf", domain()))
}
#[cfg(target_os = "linux")]
fn resolver_file() -> PathBuf {
    PathBuf::from(linux_plan::resolved_drop_in_path(domain()))
}

/// The socket the resolver listens on, as the route has to name it.
#[cfg(any(target_os = "linux", test))]
fn resolver_addr() -> std::net::SocketAddr {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    let ip = RESOLVER_BIND
        .parse()
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    SocketAddr::new(ip, port())
}

// Windows-only: a marker written once the NRPT route + front-door task are installed, so the
// toggle can cheaply tell "route present" without querying the OS each poll.
#[cfg(windows)]
fn route_marker() -> PathBuf {
    service_dir().join("route.installed")
}

// MARK: front-door settings — the .adi hosts the front door proxies to the control panel

/// Simple, user-editable settings for the always-on front door: the `.adi` hosts proxied to
/// the control panel (`adi-app`). Every host is an alternative name for the *same* adi-app
/// process — they all share its single ports-manager-allocated port — so e.g. `api.adi` reaches
/// the very `/api` that `app.adi` serves. Lives at `~/.adi/mono/dns/frontdoor.toml`; edit
/// `hosts` to add or rename entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrontdoorSettings {
    hosts: Vec<String>,
}

impl Default for FrontdoorSettings {
    fn default() -> Self {
        let domain = domain();
        Self {
            hosts: vec![format!("app.{domain}"), format!("api.{domain}")],
        }
    }
}

/// The typed `frontdoor.toml` settings file within the `dns` module of a given store.
fn frontdoor_settings_in(store: &adi_config::Config) -> adi_config::ConfigFile<FrontdoorSettings> {
    store.module("dns").file("frontdoor.toml")
}

/// The front-door hosts to render, materializing the default `frontdoor.toml` on first use so
/// it's there to edit. Any read/parse failure, or an empty list, falls back to the defaults —
/// the front door must always render *something* (never an empty proxy).
fn frontdoor_hosts_in(store: &adi_config::Config) -> Vec<String> {
    let hosts = frontdoor_settings_in(store)
        .load_or_create()
        .unwrap_or_default()
        .hosts;
    if hosts.is_empty() {
        FrontdoorSettings::default().hosts
    } else {
        hosts
    }
}

fn frontdoor_hosts() -> Vec<String> {
    frontdoor_hosts_in(&adi_config::Config::open())
}

/// The paired node petnames currently written into the generated front door, read straight back
/// out of it.
///
/// The generated file is the store for this list, which is unusual for a file stamped "edits are
/// overwritten" — the reason is that the list has exactly one consumer (adi-hive's TLS leaf) and
/// exactly one producer (pairing, via [`Dns::add_mesh_node`]), and putting it anywhere else would
/// mean a second file that has to be kept in step with this one. Reading it back before a
/// re-render is what makes "overwritten" safe: [`write_frontdoor_artifacts`] regenerates the file
/// *around* the node list instead of through it, so refreshing the front door never silently
/// un-pairs a node's certificate.
fn frontdoor_mesh_nodes_in(store: &adi_config::Config) -> Vec<String> {
    std::fs::read_to_string(frontdoor_config_path_in(store))
        .map(|yaml| parse_mesh_nodes(&yaml))
        .unwrap_or_default()
}

fn frontdoor_mesh_nodes() -> Vec<String> {
    frontdoor_mesh_nodes_in(&adi_config::Config::open())
}

/// The bundled `adi-dns`, resolved as a sibling of the running executable, overridable via `ADI_DNS_BIN`.
fn binary_path() -> String {
    sibling_binary("adi-dns", "ADI_DNS_BIN")
}

/// The bundled `adi-hive` (the front-door proxy), resolved like `adi-dns`, overridable via `ADI_HIVE_BIN`.
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn hive_binary_path() -> String {
    sibling_binary("adi-hive", "ADI_HIVE_BIN")
}

/// Resolve a bundled binary as a sibling of the running executable, honoring `env_override` first.
/// On Windows the bundled binaries carry the `.exe` suffix; add it when the override doesn't.
pub(crate) fn sibling_binary(name: &str, env_override: &str) -> String {
    if let Some(p) = std::env::var_os(env_override)
        && !p.is_empty()
    {
        return p.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    let name = &format!("{name}.exe");
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .map_or_else(|| name.to_string(), |p| p.to_string_lossy().into_owned())
}

// MARK: config rendering (pure — unit-tested)

fn render_config() -> String {
    let domain = domain();
    let frontdoor_addr = frontdoor_addr();
    let port = port();
    format!(
        "# Written by adi-core — edits are overwritten when the CLI rewrites it.\n\
         domain = \"{domain}\"\n\
         bind_addr = \"{RESOLVER_BIND}\"\n\
         preferred_port = {port}\n\
         fallback_ports = []\n\
         upstreams = [\"1.1.1.1:53\", \"8.8.8.8:53\"]\n\
         manage_os_routing = false\n\
         status_file = \"{status}\"\n\
         \n\
         # Route .{domain} to the front-door address so http://<name>.{domain}/ hits adi-hive.\n\
         [[overrides]]\n\
         suffix = \"{domain}\"\n\
         address = \"{frontdoor_addr}\"\n",
        status = status_file().to_string_lossy(),
    )
}

/// The one line the node list lives on, indent included. Both the renderer and the in-place
/// patcher build it through [`mesh_nodes_line`], so a patched file is byte-identical to a fresh
/// render of the same list — which is what keeps [`frontdoor_config_current`] from reporting
/// drift (and prompting for a password) every time a node is paired.
const MESH_NODES_KEY: &str = "  mesh_nodes:";

/// `  mesh_nodes: ["laptop-b", "tower"]` — a flow sequence, so the whole list is one line and
/// editing it is a line replacement rather than a block rewrite.
fn mesh_nodes_line(nodes: &[String]) -> String {
    let inner = nodes
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{MESH_NODES_KEY} [{inner}]")
}

/// The petnames on the `mesh_nodes:` line of a rendered front door.
///
/// Reads the flow form [`mesh_nodes_line`] writes, and — because this file is meant to be
/// readable and an operator may well pre-seed a node by hand — the block form YAML also allows.
/// No match at all (an older generated file, from before the mesh existed) is an empty list, not
/// an error: a front door with no nodes still routes, it just has no mesh names on its leaf.
fn parse_mesh_nodes(yaml: &str) -> Vec<String> {
    let mut lines = yaml.lines();
    let Some(rest) = lines.find_map(|l| l.strip_prefix(MESH_NODES_KEY)) else {
        return Vec::new();
    };
    let rest = rest.trim();
    if !rest.is_empty() {
        return rest
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|n| n.trim().trim_matches('"').trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
    }
    // Block form: `- name` items indented deeper than the key, until the first line that isn't one.
    let mut out = Vec::new();
    for line in lines {
        let trimmed = line.trim_start();
        let Some(item) = trimmed.strip_prefix('-') else {
            break;
        };
        if line.len() - trimmed.len() <= 2 {
            break;
        }
        let item = item.trim().trim_matches('"').trim();
        if !item.is_empty() {
            out.push(item.to_string());
        }
    }
    out
}

/// Rewrite just the `mesh_nodes:` entry of a rendered front door, leaving every other byte where
/// it was — the comments an operator reads, the hosts, the ports, the ordering.
///
/// A whole-file re-render would be simpler, but it would need the control panel's port and the
/// host list, i.e. it would turn "record a petname" into "reach into the ports manager". Pairing
/// should not be able to fail for that reason.
///
/// When the key is missing entirely (a front door generated before the mesh existed) the line is
/// inserted into the `proxy:` block, so the pairing still takes effect at the next hive start;
/// that file is stale in other ways too and the ordinary staleness check will regenerate it. With
/// no `proxy:` block to insert into the input is returned unchanged — the caller reads that as a
/// failure and warns rather than writing something it does not understand.
fn patch_mesh_nodes(yaml: &str, nodes: &[String]) -> String {
    let line = mesh_nodes_line(nodes);
    let mut out = String::with_capacity(yaml.len() + line.len() + 1);

    if yaml.lines().any(|l| l.starts_with(MESH_NODES_KEY)) {
        // Replace in place, dropping any block-form items that belonged to the old value.
        let mut replaced = false;
        let mut in_block = false;
        for l in yaml.lines() {
            if !replaced && let Some(rest) = l.strip_prefix(MESH_NODES_KEY) {
                out.push_str(&line);
                out.push('\n');
                replaced = true;
                in_block = rest.trim().is_empty();
                continue;
            }
            if in_block {
                let trimmed = l.trim_start();
                if trimmed.starts_with('-') && l.len() - trimmed.len() > 2 {
                    continue;
                }
                in_block = false;
            }
            out.push_str(l);
            out.push('\n');
        }
        return out;
    }

    if !yaml.lines().any(|l| l.trim_end() == "proxy:") {
        return yaml.to_string();
    }
    let mut inserted = false;
    for l in yaml.lines() {
        out.push_str(l);
        out.push('\n');
        if !inserted && l.trim_end() == "proxy:" {
            out.push_str(&line);
            out.push('\n');
            inserted = true;
        }
    }
    out
}

/// The front-door `hive.yaml`: adi-hive binds `127.0.0.53:80` and **proxies** every host in
/// `hosts` (from [`frontdoor_hosts`]) to the control panel (`adi-app`) on `app_port` — all to
/// the same process, so `api.adi` reaches the same `/api` `app.adi` serves. It no longer *runs*
/// adi-app — that's a separate per-user service ([`crate::app`]) so the on/off toggle can
/// start/stop it (and its in-process mesh) without a password. Any other host gets the 4XX page.
///
/// `mesh_nodes` are the petnames of the machines this one has paired with, carried through from
/// the file being replaced (see [`frontdoor_mesh_nodes_in`]) so a regeneration keeps them.
fn render_frontdoor_hive(hosts: &[String], app_port: u16, mesh_nodes: &[String]) -> String {
    let domain = domain();
    let frontdoor_addr = frontdoor_addr();
    // One `services:` entry per host, keyed by the host's first label (`app.adi` → `app`). All
    // point at the same `app_port` — different names for one upstream. Built as a plain literal
    // so YAML indentation is exact.
    use std::fmt::Write as _;
    let mut routes = String::new();
    for host in hosts {
        let name = host
            .split('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(host);
        let _ = write!(
            routes,
            "  {name}:\n    proxy:\n      host: {host}\n    rollout:\n      recreate:\n        ports:\n          http: {app_port}\n"
        );
    }
    format!(
        "# Written by adi-core — adi-hive front door for the .{domain} zone.
# Always-on plumbing: proxies the hosts below to the adi control panel (adi-app), which runs
# as its own per-user service on this reserved port so it can be toggled without a
# password. Hosts come from ~/.adi/mono/dns/frontdoor.toml. Any other host gets the 4XX page.
proxy:
  bind:
    - \"{frontdoor_addr}:{FRONTDOOR_PORT}\"
  # HTTPS alongside (never instead of) plain HTTP. adi-hive mints a locally-trusted certificate
  # for the hosts below; trusting its CA once makes https://app.adi a secure context, which is
  # what lets the control panel be installed as an app. See adi-hive's `tls` module.
  tls_bind:
    - \"{frontdoor_addr}:{FRONTDOOR_TLS_PORT}\"
  # `n.adi` is RESERVED, and it is not a zone of this machine: <service>.<node>.n.adi names a
  # service on a DIFFERENT adi machine, reached over the mesh. Local services keep <service>.adi,
  # so the two can never collide. The front door deliberately knows nothing about peers — it sees
  # the reserved suffix and hands the connection, Host header untouched, to the local mesh gateway
  # on loopback; turning <node> into a peer key and dialling it is the gateway's job. Remove this
  # key and every such host gets adi-hive's \"mesh gateway unavailable\" page instead of a route.
  mesh_gateway: \"{gateway}\"
  # The nodes paired with this machine, used ONLY to mint TLS names — routing needs no per-node
  # entry, the one rule above covers the whole fleet. It exists because a wildcard matches exactly
  # one label: `*.n.adi` covers <node>.n.adi but never <service>.<node>.n.adi, so each node needs
  # its own `*.<node>.n.adi` on the leaf. A node missing from this list is therefore still
  # reachable over http:// immediately; only https:// warns, until the next front-door start
  # re-mints the certificate. Pairing appends here. An entry may also be dotted
  # (`nosh.<node>`) to cover a service name deeper than one label, e.g. app.nosh.<node>.n.adi —
  # nothing writes those automatically, since pairing learns a petname and not the node's hosts.
{mesh_nodes}
  # Route what is imported below; never launch it. Stated rather than inferred: adi-hive used to
  # decide this from the effective uid (root == front door), which is true on macOS and false on
  # a node, where the front door runs unprivileged. Without this key a node's front door would
  # start a second copy of every dashboard, racing its own supervisor for the same leased ports.
  routes_only: true

# Every project and every dashboard, fanned into this one front door, so a service that declares
# a `proxy.host` gets its `.adi` name here. Without these a node scaffolds dashboards that no
# hostname ever reaches — and the mesh gateway, which answers `<service>.<node>.n.adi` out of
# this same table, refuses them with \"no such service\".
# A `*` is one directory level, deliberately: a project's config has a fixed home, so these four
# lines name it outright. `**` searched for it instead, and the search is proportional to whatever
# a project keeps inside itself — one dashboard's file-backed data store (17 000 directories) made
# rediscovery cost more than everything else this daemon does, on every reload tick.
imports:
  - $ADI_PROJECTS_DIR/*/.adi/hive.yaml
  - $ADI_PROJECTS_DIR/*/hive.yaml
  - $ADI_DASHBOARDS_DIR/*/.adi/hive.yaml
  - $ADI_DASHBOARDS_DIR/*/hive.yaml

services:
{routes}",
        gateway = mesh_gateway_addr(),
        mesh_nodes = mesh_nodes_line(mesh_nodes),
    )
}

fn write_config() {
    let _ = std::fs::create_dir_all(service_dir());
    let _ = std::fs::write(config_path(), render_config());
}

/// True when the installed front-door config already matches what we'd render now, so no
/// update/restart is needed. A mismatch (or missing file) means the front door is running an
/// old config and should be refreshed once.
fn frontdoor_config_current() -> bool {
    let rendered = render_frontdoor_hive(
        &frontdoor_hosts(),
        crate::app::port(),
        &frontdoor_mesh_nodes(),
    );
    std::fs::read_to_string(frontdoor_config_path()).is_ok_and(|on_disk| on_disk == rendered)
}

// MARK: front-door staging (shared) + macOS plist checks

/// Write the generated front-door `hive.yaml` — the one artifact every platform's installer needs,
/// and the only one that is the same everywhere. What supervises it differs per OS; what it says
/// does not.
#[cfg(any(unix, windows))]
fn write_frontdoor_config() {
    let _ = std::fs::create_dir_all(service_dir());
    let _ = std::fs::write(
        frontdoor_config_path(),
        render_frontdoor_hive(
            &frontdoor_hosts(),
            crate::app::port(),
            &frontdoor_mesh_nodes(),
        ),
    );
}

/// The environment every front door is started with, on any platform.
///
/// Carries the whole resolved [`Flavor`], not just `ADI_DIR`: the front door is started by a
/// supervisor with a bare environment, and one that resolved the default identity would serve
/// the *other* install's hosts on this flavour's address. `HOME` is pinned by the macOS caller
/// on top of this, because a root daemon would otherwise read `/var/root`.
fn frontdoor_env(watch_self: bool) -> Vec<(String, String)> {
    let mut env = vec![("RUST_LOG".to_string(), "info".to_string())];
    env.extend(
        Flavor::current()
            .env()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );
    if watch_self {
        env.push(("ADI_WATCH_SELF".to_string(), "1".to_string()));
    }
    env
}

/// The front door's own log, namespaced so two installs never interleave into one file.
#[cfg(any(target_os = "linux", windows))]
fn frontdoor_user_log() -> PathBuf {
    let flavour = Flavor::current();
    let name = if flavour.is_release() {
        "adi-hive-frontdoor.log".to_string()
    } else {
        format!("adi-hive-frontdoor-{}.log", flavour.domain)
    };
    paths::logs_dir().join(name)
}

// MARK: what a root daemon may be asked to run (pure predicate + a walk, unit-tested)

/// Why one component of a program path leaves a **root** daemon only as privileged as an ordinary
/// user.
#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Debug, PartialEq, Eq)]
enum Unsafe {
    /// Owned by somebody who is not root, and so replaceable by them whenever they like.
    OwnedBy(u32),
    /// Writable by a group, or by everyone.
    Writable(u32),
}

#[cfg(any(target_os = "macos", all(test, unix)))]
impl std::fmt::Display for Unsafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwnedBy(uid) => write!(f, "is owned by uid {uid}, not by root"),
            Self::Writable(mode) => write!(f, "is writable beyond its owner (mode {mode:04o})"),
        }
    }
}

/// The verdict on one component of a program path, from its owner and its mode alone.
///
/// Ownership first, because it is the stronger statement: a file's owner can grant themselves any
/// mode they like, so `0755 alice` is exactly as replaceable as `0777 alice` and the mode says
/// nothing extra about it.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn component_verdict(uid: u32, mode: u32) -> Option<Unsafe> {
    if uid != 0 {
        return Some(Unsafe::OwnedBy(uid));
    }
    // 0o022 — group-write and other-write. Root may own the file and still have handed the right
    // to replace it to the `admin` group, which on macOS is every administrator account.
    (mode & 0o022 != 0).then_some(Unsafe::Writable(mode & 0o7777))
}

/// The first component of `program` — the file itself, then each directory above it — that a
/// non-root user could use to put different code in a root daemon's way.
///
/// A directory counts as much as the file: whoever may write `…/release/` may replace `adi-hive`
/// inside it with a `rename(2)`, whatever the old file's own mode was. The walk is over the
/// canonical path when there is one, so a symlink is judged by what it points at rather than by
/// the link's own (meaningless) mode.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn first_unsafe_component(program: &std::path::Path) -> Option<(PathBuf, Unsafe)> {
    use std::os::unix::fs::MetadataExt as _;

    let resolved = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    resolved.ancestors().find_map(|component| {
        let meta = std::fs::metadata(component).ok()?;
        component_verdict(meta.uid(), meta.mode()).map(|verdict| (component.to_path_buf(), verdict))
    })
}

/// Set this to install the root front door anyway, over the objection below.
///
/// Deliberately its own variable rather than something inferred from `ADI_HIVE_BIN`: pointing the
/// daemon at another binary and accepting that an ordinary user may rewrite it are two different
/// decisions, and only the second one is dangerous.
#[cfg(any(target_os = "macos", all(test, unix)))]
const ALLOW_UNSAFE_PROGRAM_ENV: &str = "ADI_ALLOW_UNSAFE_ROOT_PROGRAM";

/// Whether a **root** daemon may be pointed at `program`, or the sentence explaining why not.
///
/// See this module's header for the reasoning; the short version is that a root daemon whose
/// program a user can rewrite is not a root daemon, it is that user with a root-shaped hole in
/// front of them — and `ADI_WATCH_SELF` makes the hole open itself within the minute.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn root_program_objection(program: &std::path::Path) -> Option<String> {
    let (component, verdict) = first_unsafe_component(program)?;
    if std::env::var_os(ALLOW_UNSAFE_PROGRAM_ENV).is_some_and(|v| !v.is_empty()) {
        eprintln!(
            "adi: installing a root daemon that runs {} even though {} {verdict} — \
             {ALLOW_UNSAFE_PROGRAM_ENV} is set, so anyone who can write that can run code as root",
            program.display(),
            component.display(),
        );
        return None;
    }
    Some(format!(
        "refusing to install a root daemon that runs {program}: {component} {verdict}, so \
         anyone who can write it can run code as root — and ADI_WATCH_SELF makes the daemon \
         adopt a replacement within a minute, with no prompt.\n  \
         Point it at a root-owned copy (ADI_HIVE_BIN=/path/to/adi-hive, or `sudo chown root:wheel` \
         the component named above), or set {ALLOW_UNSAFE_PROGRAM_ENV}=1 to install it anyway.",
        program = program.display(),
        component = component.display(),
    ))
}

/// Stage the front-door daemon's config + plist (unprivileged); pins `HOME`/`ADI_DIR` to the installing user's, since the root daemon would otherwise use `/var/root/.adi`.
///
/// `Err` when the binary this would put in a **root** daemon's `ProgramArguments` is one an
/// ordinary user may rewrite. Nothing is staged in that case and no prompt is raised: the three
/// privileged entry points below all start here, so this is the one gate they share.
#[cfg(target_os = "macos")]
fn write_frontdoor_artifacts() -> Result<(), String> {
    let plist = render_frontdoor_plist()?;
    write_frontdoor_config();
    let _ = std::fs::write(frontdoor_plist_stage(), plist);
    Ok(())
}

/// The daemon definition itself, rendered but not written — so what a root daemon is about to be
/// told to run can be asserted in a test instead of only in a review.
#[cfg(target_os = "macos")]
fn render_frontdoor_plist() -> Result<String, String> {
    // The destination, not the source. What a root daemon may run is a question about the file
    // it will execute for months, and [`frontdoor_program_path`] is root-owned because
    // [`install_program_shell`] below puts it there under the same prompt. The check stays in
    // front of it all the same: a `/Library/Application Support` that some other installer left
    // group-writable would put the whole hole back, and this is the one place that would notice.
    let program = frontdoor_program_path();
    if let Some(objection) = root_program_objection(std::path::Path::new(&program)) {
        return Err(objection);
    }
    // `ADI_WATCH_SELF` makes the front door poll its own binary and exit when it changes, so
    // launchd's KeepAlive starts whatever is there now. That binary is the root copy, which
    // changes only under a privileged step — so an auto-update no longer restarts the front door
    // into the new build. It goes on proxying with the copy it has until a repair refreshes it,
    // which [`frontdoor_program_stale`] reports and the services list offers as a button.
    let mut env = frontdoor_env(true);
    env.push((
        "HOME".to_string(),
        std::env::var("HOME").unwrap_or_default(),
    ));
    Ok(launchd::plist_xml(
        &frontdoor_label(),
        &[
            program,
            frontdoor_config_path().to_string_lossy().into_owned(),
        ],
        &frontdoor_log(),
        &env,
    ))
}

/// The privileged fragment that puts the daemon's program in place, for the three installs below.
///
/// Copied through a temporary name and `mv`'d over, rather than written in place: `mv` is a
/// `rename(2)`, which macOS allows over a *running* executable, while writing into one gives
/// `ETXTBSY`. The directory is created and owned in the same breath, because a root-owned binary
/// inside a directory somebody else may write is not a root-owned binary.
#[cfg(target_os = "macos")]
fn install_program_shell() -> String {
    let source = hive_binary_path();
    let program = frontdoor_program_path();
    let dir = std::path::Path::new(&program)
        .parent()
        .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
    format!(
        "mkdir -p '{dir}'\
         && chown root:wheel '{dir}'\
         && chmod 755 '{dir}'\
         && install -o root -g wheel -m 755 '{source}' '{program}.new'\
         && mv -f '{program}.new' '{program}'"
    )
}

/// Whether the daemon's root copy is missing, or is not the build the app bundle now carries.
///
/// Length rather than a hash: this is read on the status path, the two files are tens of
/// megabytes, and the question is "did an update move on without the front door" — for which a
/// changed size is evidence and a matching size on two different builds is a coincidence nobody
/// is harmed by. A missing copy counts as stale, since that is the state a repair fixes.
#[cfg(target_os = "macos")]
fn frontdoor_program_stale() -> bool {
    let len = |p: &str| std::fs::metadata(p).ok().map(|m| m.len());
    match (len(&frontdoor_program_path()), len(&hive_binary_path())) {
        (Some(copy), Some(bundle)) => copy != bundle,
        // No copy at all is stale; no bundle binary to compare against is not this check's
        // problem to report — `write_frontdoor_artifacts` is where that becomes an error.
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// True when the installed root daemon plist is the standard one we manage — it runs
/// the rendered front-door config. A dev machine may deliberately repoint the daemon
/// at another binary/config (e.g. `target/release/adi-hive` with the full
/// `hive/hive.yaml`); that plist is hand-managed and `up` must never overwrite it.
#[cfg(target_os = "macos")]
fn frontdoor_plist_managed() -> bool {
    let config = frontdoor_config_path();
    let config = config.to_string_lossy();
    std::fs::read_to_string(frontdoor_plist()).is_ok_and(|plist| {
        // Any of three, because "ours" has had three shapes and the oldest is the one still
        // sitting on the machines this matters for: the rendered config it points at, the
        // root-owned program 1.4.1 installs, or — from every build before that — the binary
        // inside the app bundle. A plist naming only the *config* was the original test, and it
        // read a pre-1.0 daemon that predates that config as somebody else's to leave alone,
        // which is precisely the machine whose front door then never got repaired.
        //
        // What stays out is the case this exists for: a plist naming a build that is not the one
        // this process would install from — somebody's own, which `up` must never take over. A
        // plist naming exactly the binary we would install from *is* taken over, and that is the
        // migration, not a takeover: the daemon ends up running a root-owned copy of the same
        // build. It also closes the hole that put this check here, since a later `cargo build`
        // then replaces a file the daemon no longer runs.
        plist.contains(config.as_ref())
            || plist.contains(&frontdoor_program_path())
            || plist.contains(&hive_binary_path())
    })
}

/// True when the installed root daemon plist already carries the self-watch env — the
/// one-time migration that lets auto-updates restart the front door without a password.
/// Deliberately a marker check, not a byte compare: the plist embeds the machine's
/// binary path, which legitimately differs between installs.
#[cfg(target_os = "macos")]
fn frontdoor_plist_current() -> bool {
    std::fs::read_to_string(frontdoor_plist()).is_ok_and(|p| p.contains("ADI_WATCH_SELF"))
}

// MARK: the automatic front-door repair
//
// [`Dns::front_door_installed`] is a `stat`. It says the daemon plist is on disk and nothing
// whatsoever about whether launchd ever loaded it — and the two come apart in one specific,
// silent way. The privileged install is a `&&` chain (`cp` the plist … `&& launchctl
// bootstrap`), so a prompt cancelled after the copy, a bootstrap that lost a race, or a
// background item switched off later leaves the file installed with nothing running. From then
// on `route_installed` reports the machine as provisioned for good: `on_enable` skips the
// install on every later launch, `up` and a relaunch of the app repair nothing, and every
// `.adi` name resolves and then hangs. So the enable path asks the socket, not the file.

/// Where the automatic repair records its last attempt.
fn repair_stamp_path_in(store: &adi_config::Config) -> PathBuf {
    service_dir_in(store).join("frontdoor-repair.json")
}

/// The automatic repair's own memory. One field today; a struct because it is a state file
/// like the updater's, and the next thing anyone wants from it is why the last attempt failed.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default, Serialize, Deserialize)]
struct RepairStamp {
    /// When the last automatic attempt raised its prompt — recorded whether or not the prompt
    /// was answered, because a cancelled one is exactly what must not come straight back.
    last_attempt_unix: u64,
}

/// The gap the automatic repair leaves between two prompts.
///
/// Not zero, because `up` runs on more than the user's own double-click — the app's launch, a
/// CLI invocation, the updater's restart — and three password prompts for one gesture is how
/// an operator learns to cancel them on sight. Not long, because the fix is meant to be "open
/// ADI again": five minutes is under any deliberate relaunch and over any burst of `up`s. The
/// button in the services list carries no cooldown at all — an explicit act needs none.
#[cfg(any(target_os = "macos", test))]
const REPAIR_COOLDOWN: Duration = Duration::from_secs(300);

/// Whether an automatic repair may prompt now, given what the stamp at `path` remembers.
///
/// Unreadable, absent or corrupt all mean *yes*: this gate exists to stop a prompt storm, not
/// to withhold the fix, so every way of not knowing resolves towards trying.
#[cfg(any(target_os = "macos", test))]
fn may_repair_at(path: &Path, now: u64) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(stamp) = serde_json::from_str::<RepairStamp>(&raw) else {
        return true;
    };
    // A stamp from the future — a clock that moved, or a store copied off another machine —
    // must not lock the repair out until it catches up.
    now < stamp.last_attempt_unix || now - stamp.last_attempt_unix >= REPAIR_COOLDOWN.as_secs()
}

#[cfg(any(target_os = "macos", test))]
fn record_repair_at(path: &Path, now: u64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stamp = RepairStamp {
        last_attempt_unix: now,
    };
    if let Ok(json) = serde_json::to_string(&stamp) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(target_os = "macos")]
fn may_repair_front_door() -> bool {
    may_repair_at(
        &repair_stamp_path_in(&adi_config::Config::open()),
        adi_config::now_unix(),
    )
}

#[cfg(target_os = "macos")]
fn record_front_door_repair() {
    record_repair_at(
        &repair_stamp_path_in(&adi_config::Config::open()),
        adi_config::now_unix(),
    );
}

/// Report the outcome of an elevated step instead of discarding it.
///
/// `proc::run_admin` hands back an `Output` that every call site here used to drop, so a cancelled
/// Authorization prompt — or a `launchctl bootstrap` that lost a race — left `up` looking like it
/// had succeeded. Not fatal: the resolver is a separate, already-running service, and the front
/// door is retried on the next `up`. But never silent.
/// The files that decide whether the front door runs at all.
///
/// Exposed for the collector. A diagnostic report used to describe every per-user `LaunchAgent`
/// in detail and say nothing at all about the one **root** job — so a machine whose plist was on
/// disk, unloaded, and pointing somewhere unexpected looked, in the archive, exactly like a
/// healthy one. Answering that took a round trip to the person having the problem; now it is in
/// the file they already sent.
#[derive(Debug, Clone)]
pub struct FrontDoorFiles {
    /// The `LaunchDaemon` definition — what launchd was asked to run.
    pub plist: PathBuf,
    /// The program that plist names: the root-owned copy.
    pub program: PathBuf,
    /// The bundle binary that copy is made from, so the two can be compared.
    pub bundle_source: PathBuf,
}

/// [`FrontDoorFiles`] for this install, or `None` where the front door is not a root install of
/// its own (Linux and Windows run it as the user, from the binary itself).
#[cfg(target_os = "macos")]
#[must_use]
pub fn front_door_files() -> Option<FrontDoorFiles> {
    Some(FrontDoorFiles {
        plist: PathBuf::from(frontdoor_plist()),
        program: PathBuf::from(frontdoor_program_path()),
        bundle_source: PathBuf::from(hive_binary_path()),
    })
}

#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn front_door_files() -> Option<FrontDoorFiles> {
    None
}

/// Load the daemon plist that is already on disk, without rewriting a byte of it.
///
/// For the one case [`Dns::install_front_door`] must not touch: a plist somebody repointed at
/// their own build. Rewriting it would take their machine over; bootstrapping it takes nothing
/// away, and an unloaded job is the same failure whoever wrote the file.
#[cfg(target_os = "macos")]
fn bootstrap_frontdoor_only() {
    let frontdoor_label = frontdoor_label();
    let frontdoor_plist = frontdoor_plist();
    let shell = format!(
        "(launchctl bootout system/{frontdoor_label} 2>/dev/null || true)\
         && launchctl bootstrap system '{frontdoor_plist}'\
         && launchctl enable system/{frontdoor_label}"
    );
    report_admin(
        "starting the front door already installed here",
        &proc::run_admin(&shell),
    );
}

#[cfg(target_os = "macos")]
fn report_admin(what: &str, out: &proc::Output) {
    if !out.ok() {
        eprintln!("adi: {what} failed ({}): {}", out.status, out.text.trim());
    }
}

// MARK: Windows front-door (a per-user Task Scheduler task, no elevation)

/// Write the front-door config and (re)register the front-door task, then start it. Unprivileged:
/// a per-user task binding `127.0.0.53:80` needs no admin on Windows.
#[cfg(windows)]
fn install_frontdoor_task() {
    write_frontdoor_config();
    // Self-watch so an auto-update that swaps the binary makes the task restart into it.
    let env = frontdoor_env(true);
    let log = frontdoor_user_log();
    launchd::enable(
        &frontdoor_label(),
        &[
            hive_binary_path(),
            frontdoor_config_path().to_string_lossy().into_owned(),
        ],
        &log.to_string_lossy(),
        &env,
    );
}

// MARK: Linux — the privileged steps, rendered as data (pure)
//
// Compiled on Linux and, under `cfg(test)`, everywhere. Nothing in here will ever run on the
// machine that builds a node package, so the only way it can be checked at all is to assert the
// bytes it would write and the argv it would spawn — which is what the tests at the bottom of this
// file do. Same arrangement as `launchd.rs`'s unit renderer.
#[cfg(any(target_os = "linux", test))]
mod linux_plan {
    use std::net::SocketAddr;

    /// Where `adi-dns` looks for its own route file (`adi-dns/src/os_routing.rs`, the Linux
    /// `platform::drop_in_path`).
    ///
    /// The path and the contents below are **`adi-dns`'s** definition, not a second one. adi-core
    /// cannot call that code — `adi-dns` is a binary-only crate and `os_routing` is a private
    /// module of its `main.rs`, so there is no library to depend on — but it must not invent a
    /// competing file either, because `adi-dns` removes exactly this path when it shuts down after
    /// having installed the route itself. Writing the same bytes at the same path keeps the two in
    /// one place logically even though the compiler cannot enforce it; the tests pin every byte.
    ///
    /// Only one of the two ever writes it in practice: [`render_config`](super::render_config)
    /// emits `manage_os_routing = false`, because the resolver runs unprivileged and could not
    /// write into `/etc` if it tried.
    #[must_use]
    pub fn resolved_drop_in_path(domain: &str) -> String {
        format!("/etc/systemd/resolved.conf.d/adi-dns-{domain}.conf")
    }

    /// The drop-in itself, byte-for-byte `adi-dns`'s `linux_resolved_contents`.
    ///
    /// `Domains=~adi` is a **routing-only** domain: it sends `.adi` queries to this resolver and
    /// changes nothing else about the machine's DNS, which is what makes installing it safe on a
    /// box whose real work is elsewhere. The port suffix is omitted on `53` for the same reason it
    /// is there — older `systemd-resolved` only learned `address:port` late, and on `53` it is
    /// simply not needed.
    #[must_use]
    pub fn resolved_drop_in(domain: &str, addr: SocketAddr) -> String {
        let dns = if addr.port() == 53 {
            addr.ip().to_string()
        } else {
            format!("{}:{}", addr.ip(), addr.port())
        };
        format!(
            "# Managed by adi-dns. Split-DNS: route only .{domain} to this resolver.\n\
             [Resolve]\n\
             DNS={dns}\n\
             Domains=~{domain}\n"
        )
    }

    /// Put the staged drop-in in place and make `systemd-resolved` read it.
    ///
    /// A copy from a staged file, not a heredoc: the privileged half then contains no data at all,
    /// so nothing about the resolver's address can reach a root shell as text.
    #[must_use]
    pub fn route_install_steps(stage: &str, drop_in: &str) -> Vec<String> {
        let dir = "/etc/systemd/resolved.conf.d";
        vec![
            format!("mkdir -p {dir}"),
            format!("cp {} {}", quote(stage), quote(drop_in)),
            format!("chmod 644 {}", quote(drop_in)),
            "systemctl restart systemd-resolved".to_string(),
        ]
    }

    /// Undo exactly what [`route_install_steps`] did, and nothing else — the drop-in is the only
    /// file this module ever put on the system.
    #[must_use]
    pub fn route_remove_steps(drop_in: &str) -> Vec<String> {
        vec![
            format!("rm -f {}", quote(drop_in)),
            "systemctl restart systemd-resolved".to_string(),
        ]
    }

    /// The front door's one privileged prerequisite: let an *unprivileged* `adi-hive` bind `:80`
    /// and `:443`.
    ///
    /// `+ep` is effective+permitted and nothing else — no inheritable bit, no other capability. It
    /// buys precisely "may bind a low port" and cannot be used to read a file, sign a packet, or
    /// become anyone. The alternative, `sysctl net.ipv4.ip_unprivileged_port_start=80`, is offered
    /// in the message instead of used here because it lowers the floor for *every* process on the
    /// machine, which is a much larger change than one bit on one binary.
    ///
    /// It does not survive the file being replaced, so an upgrade has to run it again — which is
    /// why [`Dns::install_route`](super::Dns::install_route) is idempotent and why `up` checks and
    /// says so rather than assuming.
    #[must_use]
    pub fn capability_steps(hive_bin: &str) -> Vec<String> {
        vec![format!(
            "setcap 'cap_net_bind_service=+ep' {}",
            quote(hive_bin)
        )]
    }

    /// Give the binary its ordinary powers back.
    #[must_use]
    pub fn capability_remove_steps(hive_bin: &str) -> Vec<String> {
        vec![format!("setcap -r {}", quote(hive_bin))]
    }

    /// The steps as one `/bin/sh` command line. `&&` and not `;`: a `cp` that failed must not be
    /// followed by a `systemctl restart` that makes the failure look like a reload.
    #[must_use]
    pub fn script(steps: &[String]) -> String {
        steps.join(" && ")
    }

    /// The argv that runs `script` with root's privileges.
    ///
    /// `sudo -n` is the whole point: it either already has the right (root, `NOPASSWD`, or a live
    /// credential cache) or it fails on the spot. It never prompts, so it can never wedge an
    /// `ssh node adi-mono up` that has no terminal to prompt on. When we are already root there is
    /// no reason to require `sudo` to be installed at all — a minimal container image often has no
    /// such thing.
    #[must_use]
    pub fn privileged_argv(script: &str, root: bool) -> Vec<String> {
        let mut argv: Vec<String> = if root {
            Vec::new()
        } else {
            vec!["sudo".to_string(), "-n".to_string()]
        };
        argv.push("/bin/sh".to_string());
        argv.push("-c".to_string());
        argv.push(script.to_string());
        argv
    }

    /// The same steps written out for a human to paste. This is the half that matters: when
    /// `sudo -n` cannot do it, the operator is owed the exact commands, not "failed".
    #[must_use]
    pub fn manual(steps: &[String]) -> String {
        steps
            .iter()
            .map(|s| format!("    sudo {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The machine-wide alternative to the capability, named in full so an operator who prefers it
    /// does not have to go and look it up.
    #[must_use]
    pub fn port_floor_alternative() -> String {
        "sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80".to_string()
    }

    /// Whether `getcap`'s output says the port-bind capability is on the file.
    ///
    /// Two spellings have to be read: libcap ≥ 2.60 prints `<path> cap_net_bind_service=ep`, older
    /// releases `<path> = cap_net_bind_service+ep`. A file with no capabilities prints nothing at
    /// all, which is why an empty string must be false rather than "unknown" — the caller's next
    /// move on false is to *tell* the operator, and a wrong "granted" would leave them with a
    /// front door that silently never binds.
    #[must_use]
    pub fn capability_granted(getcap_output: &str) -> bool {
        getcap_output.contains("cap_net_bind_service")
    }

    /// What to add after `Running · <addr>` so the status line describes the node it is actually
    /// on. Both are *normal* states for a node reached over the mesh (`apps/linux/README.md`), so
    /// this reads as a description and not as an error.
    #[must_use]
    pub fn detail_suffix(route_installed: bool, frontdoor_installed: bool) -> String {
        let mut missing = Vec::new();
        if !frontdoor_installed {
            missing.push("no front door");
        }
        if !route_installed {
            missing.push(".adi not routed locally");
        }
        if missing.is_empty() {
            String::new()
        } else {
            format!(" · {}", missing.join(", "))
        }
    }

    /// Single-quote one word for `/bin/sh`. Paths here come from `$HOME` and from beside the
    /// running executable, so they are not attacker-controlled — but a home directory with an
    /// apostrophe in it is an ordinary thing, and it would otherwise end the quoting mid-path and
    /// hand the rest of the name to a root shell as code.
    fn quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

// MARK: Linux front door (a per-user systemd unit + one setcap — never a root daemon)

/// Write the front-door config and (re)install the front-door unit under this user's
/// `systemd --user` manager, which also starts it. Unprivileged from beginning to end: the one
/// privileged thing the front door needs is the capability on its binary, and that is granted
/// separately by [`Dns::install_route`].
#[cfg(target_os = "linux")]
fn install_frontdoor_unit() {
    write_frontdoor_config();
    // No `ADI_WATCH_SELF` here, unlike macOS and Windows. Self-watch exists because a root
    // `LaunchDaemon` cannot be restarted without a password; a user unit can, freely. And it
    // would actively hurt: replacing the binary drops its capability, so a front door that
    // exited the moment the file changed would be restarted into a binary that can no longer
    // bind `:80` — `Restart=always` would then loop it forever. Moving the front door onto a
    // new build is `dns install-route` (re-grant, then restart), a step that knows about the
    // capability, rather than a race that does not.
    let env = frontdoor_env(false);
    let log = frontdoor_user_log();
    launchd::enable(
        &frontdoor_label(),
        &[
            hive_binary_path(),
            frontdoor_config_path().to_string_lossy().into_owned(),
        ],
        &log.to_string_lossy(),
        &env,
    );
}

/// Whether an `adi-hive` started right now could take `127.0.0.53:80`.
///
/// Asked rather than assumed, because starting a front door that cannot bind is worse than not
/// starting one: `adi-hive` exits when *nothing* bound, and the unit's `Restart=always` would turn
/// that into a permanent crash loop in the node's logs.
///
/// Two independent ways it can be true, so both are tried:
/// 1. this process can bind the port itself — true when the machine's
///    `net.ipv4.ip_unprivileged_port_start` has been lowered, or when we are root. `AddrInUse`
///    counts as yes: something already holds it, which on a working node is our own front door;
/// 2. the `adi-hive` binary carries `CAP_NET_BIND_SERVICE`. The probe above cannot see this — a
///    file capability belongs to the *file*, not to us — so it is read off the file with `getcap`.
#[cfg(target_os = "linux")]
fn frontdoor_can_bind() -> bool {
    use std::io::ErrorKind;
    use std::net::TcpListener;

    match TcpListener::bind((frontdoor_addr(), FRONTDOOR_PORT)) {
        // Bound and immediately dropped — this is a question, not a reservation.
        Ok(_) => return true,
        Err(e) if e.kind() == ErrorKind::AddrInUse => return true,
        Err(_) => {}
    }
    let bin = hive_binary_path();
    // `getcap` lives in `/sbin` on most distros, which is often not on a user's PATH.
    ["getcap", "/sbin/getcap", "/usr/sbin/getcap"]
        .into_iter()
        .map(|getcap| proc::run(&[getcap, bin.as_str()]))
        .find(proc::Output::ok)
        .is_some_and(|out| linux_plan::capability_granted(&out.text))
}

/// Are we root already? Then `sudo` is neither needed nor necessarily installed.
#[cfg(target_os = "linux")]
fn running_as_root() -> bool {
    proc::run(&["id", "-u"]).text.trim() == "0"
}

/// Carry out one privileged step and **say what happened** — the behaviour this module previously
/// lacked entirely on Linux.
///
/// Returns whether it was actually done. On a refusal the operator gets the literal commands, so
/// a node that cannot elevate is a node with instructions rather than a node that silently did
/// nothing.
#[cfg(target_os = "linux")]
fn run_privileged(what: &str, steps: &[String]) -> bool {
    let argv = linux_plan::privileged_argv(&linux_plan::script(steps), running_as_root());
    let out = proc::run(&argv);
    if out.ok() {
        eprintln!("adi: {what} — done");
        return true;
    }
    let why = out.text.trim();
    eprintln!(
        "adi: {what} needs root, and `sudo -n` could not do it without asking ({})",
        out.status
    );
    if !why.is_empty() {
        eprintln!("adi: {why}");
    }
    // The printed steps *are* the action — running them by hand finishes it, with nothing to
    // re-run afterwards. (The one exception is the capability, whose caller adds its own follow-up
    // because the front-door unit still has to be started once the binary may bind.)
    eprintln!("adi: run it yourself:");
    eprintln!("{}", linux_plan::manual(steps));
    false
}

/// Tell the operator the front door is not running and exactly what to do about it.
///
/// Deliberately says the mesh case out loud: on a node reached through `<service>.<node>.n.adi`
/// this is not a fault, and an operator who is told to "fix" something that is working as designed
/// learns to ignore the messages that matter.
#[cfg(target_os = "linux")]
fn report_frontdoor_blocked() {
    eprintln!(
        "adi: the .{} front door cannot start — adi-hive may not bind {}:{FRONTDOOR_PORT} as this user.",
        domain(),
        frontdoor_addr()
    );
    eprintln!(
        "adi: grant it once (or use {}), then re-run `{} dns install-route`:",
        linux_plan::port_floor_alternative(),
        crate::BIN_NAME
    );
    eprintln!(
        "{}",
        linux_plan::manual(&linux_plan::capability_steps(&hive_binary_path()))
    );
    eprintln!("adi: a node you reach over the mesh does not need this — see apps/linux/README.md.");
}

// MARK: paired nodes on the front door (docs/fleet.md F2)

/// What recording a paired node did to the front door.
///
/// Every variant is survivable, which is the point of naming them at all. The node list feeds
/// **only** the TLS leaf: `*.n.adi` routing is one gateway rule that covers the whole fleet and
/// never consults it. So a node this call failed to record is still reachable at
/// `http://<service>.<node>.n.adi` the instant it is paired — the cost is that `https://` warns
/// about an uncovered name until the front door is restarted with a correct list. Pairing must
/// therefore treat a non-[`Changed`](Self::Changed) result as a notice to the operator, never as
/// a reason to fail the pairing: the registry entry is what makes the node usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshNodeChange {
    /// The list was rewritten. The next front-door start mints a leaf covering the node.
    Changed,
    /// The petname was already recorded (or already absent). Nothing to do — the common case on
    /// a re-pair, which is why this is not an error.
    Unchanged,
    /// Nothing was written: an unreadable/unwritable front-door config, a file with no `proxy:`
    /// block to extend, or a petname that is not a DNS label and so could never appear in a
    /// certificate. HTTP still works; HTTPS is what degrades.
    Failed,
}

/// Whether `name` is usable as a petname *here*: one lowercase DNS label (`docs/fleet.md` §2).
/// The fleet registry validates the same shape at pairing; this is the front door refusing to
/// write a name it could not turn into a `*.<name>.n.adi` SAN, rather than trusting its caller.
fn is_petname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Add or drop `petname` in the generated front door's `proxy.mesh_nodes`, in place.
///
/// Idempotent in both directions — adding a listed node and removing an absent one are both
/// [`MeshNodeChange::Unchanged`] and touch no bytes — because pairing is retried and renames are
/// expressed as a remove followed by an add.
/// The front-door config this machine's daemon is actually started with.
///
/// A machine may carry a **hand-managed** `hive/hive.yaml` — the richer one, with imports — and
/// where it exists that is what the front door runs; a node has only the config we generate.
/// The TLS node list has to be written to whichever of the two is live, or it is bookkeeping in
/// a file nobody reads: exactly what happened here, where every pairing dutifully updated the
/// generated config while the running front door kept minting a certificate from the other one,
/// and `https://<service>.<node>.n.adi` failed on a name the machine believed it had covered.
///
/// Same preference the mesh gateway uses to resolve service labels, for the same reason.
fn front_door_in_use(store: &adi_config::Config) -> PathBuf {
    let hand_managed = store.module("hive").raw_path("hive.yaml");
    if hand_managed.is_file() {
        return hand_managed;
    }
    frontdoor_config_path_in(store)
}

fn set_mesh_node_in(store: &adi_config::Config, petname: &str, present: bool) -> MeshNodeChange {
    if present && !is_petname(petname) {
        return MeshNodeChange::Failed;
    }
    let path = front_door_in_use(store);
    let Ok(current) = std::fs::read_to_string(&path) else {
        // No generated front door to amend (the route was never installed). Removing a node it
        // never listed is already true, so say so; recording one is a real miss — the file is
        // rendered fresh on the next `adi up` and will not know about this node until a re-pair.
        return if present {
            MeshNodeChange::Failed
        } else {
            MeshNodeChange::Unchanged
        };
    };

    let mut nodes = parse_mesh_nodes(&current);
    if nodes.iter().any(|n| n == petname) == present {
        return MeshNodeChange::Unchanged;
    }
    if present {
        nodes.push(petname.to_string());
        nodes.sort();
    } else {
        nodes.retain(|n| n != petname);
    }

    let next = patch_mesh_nodes(&current, &nodes);
    if next == current {
        // `patch_mesh_nodes` found nowhere to put the line — a hand-mangled config. Leave it be.
        return MeshNodeChange::Failed;
    }
    if std::fs::write(&path, next).is_err() {
        return MeshNodeChange::Failed;
    }
    MeshNodeChange::Changed
}

/// The DNS command surface (`adi.dns.*`) — a zero-sized facade; all state lives on disk / in the OS supervisor.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dns;

#[allow(clippy::unused_self)]
impl Dns {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Record a newly paired node so the front door's next certificate covers
    /// `*.<petname>.n.adi`. Call it once the fleet registry has *saved* the pairing — this is
    /// bookkeeping about a node that already exists, so it must never run ahead of the registry.
    ///
    /// Deliberately infallible: see [`MeshNodeChange`] for why a failure here is a warning and
    /// not an aborted pairing.
    #[must_use]
    pub fn add_mesh_node(self, petname: &str) -> MeshNodeChange {
        set_mesh_node_in(&adi_config::Config::open(), petname, true)
    }

    /// Drop a node from the front door's certificate list — the other half of
    /// [`add_mesh_node`](Self::add_mesh_node), for unpairing. A rename is the two composed:
    /// remove the old petname, add the new one.
    #[must_use]
    pub fn remove_mesh_node(self, petname: &str) -> MeshNodeChange {
        set_mesh_node_in(&adi_config::Config::open(), petname, false)
    }

    /// Whether names in this install's zone resolve locally — the `/etc/resolver` file.
    ///
    /// Half of what [`Self::route_installed`] asks. Split out because the two are separate
    /// grants with separate consequences: without this, `app.adi` does not resolve at all;
    /// without the front door, it resolves and then nothing answers on port 80.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn dns_route_installed(self) -> bool {
        resolver_file().exists()
    }

    /// Whether the root front door is installed — the thing that answers on `:80`/`:443`.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn front_door_installed(self) -> bool {
        PathBuf::from(frontdoor_plist()).exists()
    }

    /// Whether the `.adi` route and front door are installed.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn route_installed(self) -> bool {
        // Both bits must be present; a missing either re-runs the idempotent install rather
        // than stranding a half state.
        self.dns_route_installed() && self.front_door_installed()
    }

    /// Whether the `.adi` route and front door are installed.
    ///
    /// The same two-files question macOS asks, about this platform's two files: the
    /// `systemd-resolved` drop-in, and the front door's unit under `~/.config/systemd/user`. Both
    /// are plain `stat`s — asking `systemctl` would be a subprocess on every status poll to learn
    /// something the file already says.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn route_installed(self) -> bool {
        self.dns_route_installed() && self.front_door_installed()
    }

    /// Half of [`Self::route_installed`] — the `systemd-resolved` drop-in.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn dns_route_installed(self) -> bool {
        resolver_file().exists()
    }

    /// Half of [`Self::route_installed`] — the front door's user unit.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn front_door_installed(self) -> bool {
        launchd::unit_path(&frontdoor_label()).exists()
    }

    /// Whether the `.adi` NRPT route + front-door task are installed (marker written on install).
    #[cfg(windows)]
    #[must_use]
    pub fn route_installed(self) -> bool {
        self.dns_route_installed() && self.front_door_installed()
    }

    /// Half of [`Self::route_installed`] — the NRPT rule, recorded by a marker on install.
    #[cfg(windows)]
    #[must_use]
    pub fn dns_route_installed(self) -> bool {
        route_marker().exists()
    }

    /// Half of [`Self::route_installed`] — the front door's scheduled task.
    #[cfg(windows)]
    #[must_use]
    pub fn front_door_installed(self) -> bool {
        launchd::is_loaded(&frontdoor_label())
    }

    /// Whether the front door is **answering** — the question every `front_door_installed`
    /// above cannot ask.
    ///
    /// Those check that a file is in place; this checks that something is behind it. Putting a
    /// process behind that address needs root, but asking it a question needs nothing at all,
    /// so this is an unprivileged connect to `<frontdoor_addr>:80` — the same move the browser
    /// makes, and the only check that fails on the machine where the plist is installed and
    /// launchd never loaded it.
    ///
    /// Cheap and wrong-in-the-safe-direction: a machine under enough load to miss the budget
    /// reads as "not answering", which shows a repair button and, on the enable path, is
    /// confirmed with a longer knock before anything asks for a password.
    #[must_use]
    pub fn front_door_answering(self) -> bool {
        TcpStream::connect_timeout(&frontdoor_probe_addr(), FRONTDOOR_PROBE_TIMEOUT).is_ok()
    }

    /// The same question, asked the way it must be asked before spending someone's password:
    /// twice, the second time patiently.
    #[cfg(target_os = "macos")]
    #[must_use]
    fn front_door_confirmed_dead(self) -> bool {
        !self.front_door_answering()
            && TcpStream::connect_timeout(&frontdoor_probe_addr(), FRONTDOOR_CONFIRM_TIMEOUT)
                .is_err()
    }

    /// The one privileged step: install the `/etc/resolver` route AND the root front-door daemon in a single admin prompt.
    #[cfg(target_os = "macos")]
    pub fn install_route(self) {
        let frontdoor_label = frontdoor_label();
        let frontdoor_plist = frontdoor_plist();
        let port = port();
        let _ = std::fs::create_dir_all(service_dir());
        let _ = std::fs::write(
            stage_path(),
            format!("nameserver {RESOLVER_BIND}\nport {port}\n"),
        );
        if let Err(objection) = write_frontdoor_artifacts() {
            // Before the prompt, so the operator is not asked for a password to install something
            // that would then be refused — and before `/etc/resolver`, so the route and the front
            // door are still installed together or not at all.
            eprintln!("adi: {objection}");
            return;
        }

        let stage = stage_path();
        let stage = stage.to_string_lossy();
        let resolver = resolver_file();
        let resolver = resolver.to_string_lossy();
        let plist_stage = frontdoor_plist_stage();
        let plist_stage = plist_stage.to_string_lossy();
        let program = install_program_shell();
        let shell = format!(
            "{program}\
             && mkdir -p /etc/resolver\
             && cp '{stage}' '{resolver}'\
             && chmod 644 '{resolver}'\
             && cp '{plist_stage}' '{frontdoor_plist}'\
             && chown root:wheel '{frontdoor_plist}'\
             && chmod 644 '{frontdoor_plist}'\
             && (launchctl bootout system/{frontdoor_label} 2>/dev/null || true)\
             && launchctl bootstrap system '{frontdoor_plist}'\
             && launchctl enable system/{frontdoor_label}\
             && dscacheutil -flushcache\
             && killall -HUP mDNSResponder"
        );
        report_admin(
            "installing the .adi route + front door",
            &proc::run_admin(&shell),
        );
    }

    /// Grant just the DNS route: `/etc/resolver/<domain>`, so names in this zone resolve here.
    ///
    /// Separate from [`Self::install_front_door`] because they are two different permissions to
    /// ask for and two different things to lose. [`Self::install_route`] still does both in one
    /// prompt and is what the CLI's `install-route` runs; these two exist for an onboarding that
    /// asks for one thing at a time, and each is idempotent, so granting one after the other
    /// leaves exactly the state `install_route` would have.
    #[cfg(target_os = "macos")]
    pub fn install_dns_route(self) {
        let port = port();
        let _ = std::fs::create_dir_all(service_dir());
        let _ = std::fs::write(
            stage_path(),
            format!("nameserver {RESOLVER_BIND}\nport {port}\n"),
        );
        let stage = stage_path();
        let stage = stage.to_string_lossy();
        let resolver = resolver_file();
        let resolver = resolver.to_string_lossy();
        let shell = format!(
            "mkdir -p /etc/resolver\
             && cp '{stage}' '{resolver}'\
             && chmod 644 '{resolver}'\
             && dscacheutil -flushcache\
             && killall -HUP mDNSResponder"
        );
        report_admin(
            "routing this zone to the local resolver",
            &proc::run_admin(&shell),
        );
    }

    /// Grant just the front door: the root `LaunchDaemon` that answers `:80`/`:443`.
    #[cfg(target_os = "macos")]
    pub fn install_front_door(self) {
        let frontdoor_label = frontdoor_label();
        let frontdoor_plist = frontdoor_plist();
        if let Err(objection) = write_frontdoor_artifacts() {
            eprintln!("adi: {objection}");
            return;
        }
        let plist_stage = frontdoor_plist_stage();
        let plist_stage = plist_stage.to_string_lossy();
        let program = install_program_shell();
        let shell = format!(
            "{program}\
             && cp '{plist_stage}' '{frontdoor_plist}'\
             && chown root:wheel '{frontdoor_plist}'\
             && chmod 644 '{frontdoor_plist}'\
             && (launchctl bootout system/{frontdoor_label} 2>/dev/null || true)\
             && launchctl bootstrap system '{frontdoor_plist}'\
             && launchctl enable system/{frontdoor_label}"
        );
        report_admin("installing the front door", &proc::run_admin(&shell));
    }

    /// Grant just the DNS route on Linux — the `systemd-resolved` drop-in.
    ///
    /// The Linux install was already two separate steps for its own reasons (see
    /// [`Self::install_route`] below); these name them so the same onboarding works here.
    #[cfg(not(target_os = "macos"))]
    pub fn install_dns_route(self) {
        self.install_route();
    }

    /// Grant just the front door on Linux and Windows.
    #[cfg(not(target_os = "macos"))]
    pub fn install_front_door(self) {
        self.install_route();
    }

    /// The privileged steps on Linux, each attempted through `sudo -n` and each **reported**.
    ///
    /// Two of them, deliberately not fused into one:
    ///
    /// 1. the `.adi` route — a `systemd-resolved` drop-in. Optional on a node: you reach its
    ///    services through the mesh from your own machine, where your own front door resolves the
    ///    name, so nothing on the node has to resolve anything (`apps/linux/README.md`). This is
    ///    for when you ssh in and want `curl http://app.adi/` to work *there*;
    /// 2. the front door's capability — what lets the unprivileged `adi-hive` bind `:80`/`:443`.
    ///
    /// macOS fuses its two because each fusion saves an Authorization *prompt*. `sudo -n` never
    /// prompts, so there is nothing to save and everything to gain from telling the operator which
    /// half of this failed.
    ///
    /// Idempotent, and meant to be re-run after an upgrade: replacing the `adi-hive` file discards
    /// its capability.
    #[cfg(target_os = "linux")]
    pub fn install_route(self) {
        let _ = std::fs::create_dir_all(service_dir());
        write_config();
        let _ = std::fs::write(
            stage_path(),
            linux_plan::resolved_drop_in(domain(), resolver_addr()),
        );

        let stage = stage_path();
        let drop_in = resolver_file();
        run_privileged(
            &format!("routing .{} to the local resolver", domain()),
            &linux_plan::route_install_steps(&stage.to_string_lossy(), &drop_in.to_string_lossy()),
        );

        let granted = run_privileged(
            "letting the front door bind :80/:443",
            &linux_plan::capability_steps(&hive_binary_path()),
        );

        // Same reason as `on_enable`: this file is the node's route table for the mesh gateway,
        // not just the front door's config, so it is written even when neither privileged step
        // above succeeded and no front door will run.
        write_frontdoor_config();

        // The front door itself is ordinary user work — but only worth starting if it can bind.
        if granted || frontdoor_can_bind() {
            install_frontdoor_unit();
        } else {
            report_frontdoor_blocked();
        }
    }

    /// The one privileged step on Windows: add the `.adi` NRPT rule (one UAC prompt). The
    /// front-door task itself is per-user and installed unprivileged.
    #[cfg(windows)]
    pub fn install_route(self) {
        write_config();
        install_frontdoor_task();
        // Idempotent NRPT install: drop any existing `.adi` rule, then add ours pointing the
        // whole `.adi` namespace at the local resolver, and flush the client cache.
        let domain = domain();
        let ps = format!(
            "$ErrorActionPreference='Stop';\n\
             Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -eq '.{domain}' }} | Remove-DnsClientNrptRule -Force -ErrorAction SilentlyContinue;\n\
             Add-DnsClientNrptRule -Namespace '.{domain}' -NameServers '127.0.0.1';\n\
             Clear-DnsClientCache;\n"
        );
        let out = proc::run_admin(&ps);
        if out.ok() {
            let _ = std::fs::create_dir_all(service_dir());
            let _ = std::fs::write(route_marker(), "1\n");
        }
    }

    /// Update the installed front door to the current config **and plist** and restart it (a
    /// single admin prompt). Needed when the on-disk front-door config or the daemon plist is
    /// stale; after this the front door is proxy-only and the toggle never touches it again.
    #[cfg(target_os = "macos")]
    pub fn update_frontdoor(self) {
        let frontdoor_label = frontdoor_label();
        let frontdoor_plist = frontdoor_plist();
        if let Err(objection) = write_frontdoor_artifacts() {
            // A refresh that would rewrite the daemon's program to something user-writable is
            // refused for the same reason a first install is — and leaving the running front door
            // alone is the safe half of the two.
            eprintln!("adi: {objection}");
            return;
        }
        let plist_stage = frontdoor_plist_stage();
        let plist_stage = plist_stage.to_string_lossy();
        // A plist change (env, args) only takes effect through bootout → bootstrap;
        // `kickstart -k` restarts the job but never re-reads the plist. bootout is
        // async, so the bootstrap is retried until the old job has fully unloaded and
        // :80 can be rebound.
        let program = install_program_shell();
        let shell = format!(
            "set -e\
             ; {program}\
             ; cp '{plist_stage}' '{frontdoor_plist}'\
             ; chown root:wheel '{frontdoor_plist}'\
             ; chmod 644 '{frontdoor_plist}'\
             ; launchctl bootout system/{frontdoor_label} 2>/dev/null || true\
             ; n=0\
             ; until launchctl bootstrap system '{frontdoor_plist}' 2>/dev/null; do n=$((n+1)); if [ \"$n\" -ge 25 ]; then exit 1; fi; sleep 0.2; done\
             ; launchctl enable system/{frontdoor_label}"
        );
        report_admin("updating the front door", &proc::run_admin(&shell));
    }

    /// Refresh the front door to the current config and restart it. **No elevation at all** — the
    /// unit belongs to this user's manager, and rewriting it is a file write.
    ///
    /// This is the payoff of not making the Linux front door a root daemon: on macOS the same
    /// operation costs an Authorization prompt every time the rendered config drifts.
    #[cfg(target_os = "linux")]
    pub fn update_frontdoor(self) {
        // `launchd::enable` on the systemd back-end rewrites the unit, reloads and restarts it, so
        // there is no separate kickstart to do (unlike Windows, where re-registering a task does
        // not restart the running instance).
        install_frontdoor_unit();
    }

    /// Refresh the front-door task to the current config and restart it. On Windows the front
    /// door is a per-user task, so no elevation is needed.
    #[cfg(windows)]
    pub fn update_frontdoor(self) {
        install_frontdoor_task();
        launchd::kickstart(&frontdoor_label());
    }

    /// Tear down both privileged bits, best-effort (incl. the `lo0` alias).
    #[cfg(target_os = "macos")]
    pub fn remove_route(self) {
        let frontdoor_addr = frontdoor_addr();
        let frontdoor_label = frontdoor_label();
        let frontdoor_plist = frontdoor_plist();
        let resolver = resolver_file();
        let resolver = resolver.to_string_lossy();
        let shell = format!(
            "(launchctl bootout system/{frontdoor_label} 2>/dev/null || true)\
             ; rm -f '{frontdoor_plist}'\
             ; rm -f '{resolver}'\
             ; (ifconfig lo0 -alias {frontdoor_addr} 2>/dev/null || true)\
             ; dscacheutil -flushcache\
             ; killall -HUP mDNSResponder"
        );
        report_admin(
            "removing the .adi route + front door",
            &proc::run_admin(&shell),
        );
    }

    /// Tear down all three pieces, best-effort and each reported: the front-door unit (no
    /// privilege), the drop-in, and the binary's capability.
    ///
    /// The capability goes too. Leaving it behind would be leaving a binary that may bind
    /// privileged ports on a machine whose operator has just said they do not want it to.
    #[cfg(target_os = "linux")]
    pub fn remove_route(self) {
        launchd::disable(&frontdoor_label());
        let drop_in = resolver_file();
        run_privileged(
            &format!("removing the .{} route", domain()),
            &linux_plan::route_remove_steps(&drop_in.to_string_lossy()),
        );
        run_privileged(
            "revoking the front door's port capability",
            &linux_plan::capability_remove_steps(&hive_binary_path()),
        );
    }

    /// Tear down the NRPT route (one UAC prompt) and the front-door task, best-effort.
    #[cfg(windows)]
    pub fn remove_route(self) {
        launchd::disable(&frontdoor_label());
        let domain = domain();
        let ps = format!(
            "Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -eq '.{domain}' }} | Remove-DnsClientNrptRule -Force -ErrorAction SilentlyContinue;\n\
             Clear-DnsClientCache;\n"
        );
        proc::run_admin(&ps);
        let _ = std::fs::remove_file(route_marker());
    }
}

impl Service for Dns {
    fn id(&self) -> &'static str {
        "dns"
    }
    fn name(&self) -> &'static str {
        "DNS"
    }
    fn label(&self) -> String {
        label()
    }
    fn status_path(&self) -> PathBuf {
        status_file()
    }
    fn log_path(&self) -> PathBuf {
        paths::logs_dir().join("adi-dns.log")
    }

    fn program(&self) -> Vec<String> {
        write_config();
        vec![binary_path(), config_path().to_string_lossy().into_owned()]
    }

    // Installed once and left in place, so toggling never re-prompts; removal is an explicit
    // action. The one exception is a stale front-door config or daemon plist (e.g. upgrading
    // from the old runner-based front door, or rolling out the self-watch env) — update it
    // once here. A hand-repointed daemon plist (dev machines) is never auto-migrated:
    // `install-route` stays the explicit way to reclaim it.
    #[cfg(target_os = "macos")]
    fn on_enable(&self) {
        if !self.route_installed() {
            self.install_route();
        } else if self.front_door_confirmed_dead() && may_repair_front_door() {
            // Installed and dead: the files say provisioned, the socket says nothing is there.
            // Asked *before* the drift comparison below, because those two only ever compare
            // files with other files — and files that agree with each other say nothing about a
            // daemon launchd forgot. A dead front door is also the bigger fault of the two, and
            // the repair re-stages the plist anyway, so it fixes any drift on its way past.
            //
            // Recorded before the prompt, so a cancelled one counts as an attempt and does not
            // come straight back on the next `up` in the same sitting.
            record_front_door_repair();
            if frontdoor_plist_managed() {
                // The narrow repair: re-stage the plist and the daemon's program, boot it out,
                // bootstrap it back, `enable` it (for a background item switched off in System
                // Settings) — without touching `/etc/resolver`, which was never the broken half.
                self.install_front_door();
            } else {
                // Somebody repointed the daemon at their own build. Rewriting their plist is
                // still off the table — but *loading* it rewrites nothing, and this branch used
                // to skip such a machine entirely, leaving the one failure it exists to fix.
                bootstrap_frontdoor_only();
            }
        } else if frontdoor_plist_managed()
            && (!frontdoor_config_current() || !frontdoor_plist_current())
        {
            self.update_frontdoor();
        }
    }

    /// Linux: bring up the half that needs no privilege, and *say* what the other half needs.
    ///
    /// Two deliberate departures from macOS, both because a node is not a laptop:
    ///
    /// * **`up` never installs the DNS route.** On macOS `.adi` names are the whole point, so the
    ///   first `up` earns its one prompt. A node's services are reached over the mesh under
    ///   `<service>.<node>.n.adi`, resolved on the *viewer's* machine — routing `.adi` on the node
    ///   itself is a convenience for someone ssh'd in, and touching `/etc` on a machine that never
    ///   asked is not something a routine `up` should do. `dns install-route` is where that lives.
    /// * **The front door is only started when it can bind.** `adi-hive` exits if it bound
    ///   nothing, and the unit restarts it forever; enabling it without the capability would leave
    ///   a crash loop as the node's welcome. So we check first, and if it cannot, we say so with
    ///   the command that fixes it — the case that used to be a silent no-op.
    #[cfg(target_os = "linux")]
    fn on_enable(&self) {
        // The rendered front-door config is also the node's **route table**: the mesh gateway
        // resolves an incoming service label against it (`docs/fleet.md` §6), falling back to this
        // generated file when there is no hand-managed `hive/hive.yaml`. So it is written whether
        // or not a front door can be supervised. Writing it only alongside the unit was the bug
        // that made a mesh-only node — the normal case, since binding :80 needs a capability the
        // node never has to grant — answer every request with `ServiceUnknown` while looking
        // perfectly healthy: paired, authorized, reachable, serving nothing.
        write_frontdoor_config();
        if !frontdoor_can_bind() {
            report_frontdoor_blocked();
            return;
        }
        if launchd::is_loaded(&frontdoor_label()) {
            if !frontdoor_config_current() {
                self.update_frontdoor();
            }
        } else {
            install_frontdoor_unit();
        }
    }

    // Windows: install the NRPT route + front-door task once; thereafter only refresh the
    // (unprivileged) front-door task when its config drifts.
    #[cfg(windows)]
    fn on_enable(&self) {
        if !self.route_installed() {
            self.install_route();
        } else if !frontdoor_config_current() {
            self.update_frontdoor();
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn detail(&self, status: Option<&DaemonStatus>) -> String {
        status.map_or_else(String::new, |s| {
            format!("Running · {RESOLVER_BIND}:{}", s.port)
        })
    }

    /// Linux: a running resolver is not the same thing as a working `.adi`, so say which.
    ///
    /// On macOS the route and the front door are installed together on the first `up` and are
    /// therefore a safe assumption. On a node neither is, by design — and "Running" on its own
    /// would then be read as "`http://app.adi/` works here", which it does not. Both clauses
    /// describe the ordinary mesh-only node; neither is phrased as a fault.
    #[cfg(target_os = "linux")]
    fn detail(&self, status: Option<&DaemonStatus>) -> String {
        status.map_or_else(String::new, |s| {
            format!(
                "Running · {RESOLVER_BIND}:{}{}",
                s.port,
                linux_plan::detail_suffix(
                    resolver_file().exists(),
                    launchd::unit_path(&frontdoor_label()).exists(),
                )
            )
        })
    }

    fn extra_actions(&self) -> Vec<Action> {
        let mut actions = vec![route_action(self.route_installed())];
        // Only when there is something to repair. A button that is always on screen teaches
        // nobody what it is for, and this one spends a password; offered next to a front door
        // that is installed and not answering, it says what happened by existing.
        if self.route_installed() && !self.front_door_answering() {
            actions.push(repair_action());
        } else if self.route_installed() {
            actions.extend(program_refresh_action());
        }
        actions
    }
}

/// The install/remove-route action for the current route state.
fn route_action(installed: bool) -> Action {
    let domain = domain();
    let (title, verb) = if installed {
        (format!("Remove .{domain} route + page"), "remove-route")
    } else {
        (format!("Install .{domain} route + page…"), "install-route")
    };
    Action {
        id: "route".to_string(),
        title,
        args: vec!["dns".to_string(), verb.to_string()],
    }
}

/// The repair offered when the front door is installed but silent.
///
/// `grant-network` rather than `install-route`: the route half is already there — re-copying
/// `/etc/resolver` would be asking for a password to rewrite a file that was never wrong — and
/// this is the half that reinstalls, bootstraps and enables the daemon.
fn repair_action() -> Action {
    Action {
        id: "front-door".to_string(),
        title: format!("Repair the front door (.{} not answering)…", domain()),
        args: vec!["dns".to_string(), "grant-network".to_string()],
    }
}

/// Offered when the front door answers but runs an older build than the bundle — the standing
/// cost of pointing a root daemon at a root-owned *copy* instead of at the app.
///
/// An auto-update swaps the bundle without a password and therefore cannot refresh the copy, so
/// the front door goes on proxying with the build it was installed with. Harmless most days,
/// which is exactly why it needs saying out loud rather than a prompt on every launch.
///
/// Never for a plist somebody repointed at their own build: that machine's front door is not the
/// copy, so "older than the bundle" says nothing true about it.
#[cfg(target_os = "macos")]
fn program_refresh_action() -> Option<Action> {
    (frontdoor_plist_managed() && frontdoor_program_stale()).then(|| Action {
        id: "front-door-refresh".to_string(),
        title: "Update the front door to this build…".to_string(),
        args: vec!["dns".to_string(), "grant-network".to_string()],
    })
}

/// Only macOS runs the front door from a copy; elsewhere the unit runs the binary itself and
/// there is nothing to fall behind.
#[cfg(not(target_os = "macos"))]
fn program_refresh_action() -> Option<Action> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolver config must describe the flavour this process belongs to — not the
    /// literals the release install happens to use. Asserting against the flavour is what
    /// makes the test say something on a `dev` build instead of simply failing on it.
    #[test]
    fn config_describes_this_flavour() {
        let flavour = Flavor::current();
        let cfg = render_config();
        assert!(
            cfg.contains(&format!("domain = \"{}\"", flavour.domain)),
            "{cfg}"
        );
        assert!(
            cfg.contains(&format!("preferred_port = {}", port())),
            "{cfg}"
        );
        assert!(
            cfg.contains(&format!("suffix = \"{}\"", flavour.domain)),
            "{cfg}"
        );
        assert!(
            cfg.contains(&format!("address = \"{}\"", flavour.frontdoor_addr)),
            "{cfg}"
        );
        assert!(cfg.contains("status_file = \""));
    }

    /// The route and the resolver are written from the same two accessors, so they cannot
    /// disagree — but only as long as nobody reintroduces a literal on one side of the pair.
    #[test]
    fn the_route_points_at_the_port_the_resolver_binds() {
        let cfg = render_config();
        assert!(cfg.contains(&format!("preferred_port = {}", port())));
        assert_eq!(resolver_addr().port(), port());
        assert_eq!(resolver_addr().ip().to_string(), RESOLVER_BIND);
    }

    /// A rendered front door with two hosts and whatever node list the caller wants.
    fn rendered(nodes: &[&str]) -> String {
        let hosts = vec!["app.adi".to_string(), "api.adi".to_string()];
        let nodes: Vec<String> = nodes.iter().map(|n| (*n).to_string()).collect();
        render_frontdoor_hive(&hosts, 8091, &nodes)
    }

    /// A throwaway store rooted in the temp dir. Never `Config::open()` in a test — that is the
    /// machine's live `~/.adi/mono`, front door and all.
    fn scratch_store(tag: &str) -> (adi_config::Config, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "adi-core-dns-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        (adi_config::Config::with_root(&root), root)
    }

    /// Seed a scratch store with a freshly rendered front door and hand back its path.
    fn seed_frontdoor(store: &adi_config::Config, nodes: &[&str]) -> PathBuf {
        let path = frontdoor_config_path_in(store);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, rendered(nodes)).expect("seed");
        path
    }

    #[test]
    fn frontdoor_hive_proxies_the_control_panel_and_is_valid_yaml() {
        let cfg = rendered(&[]);
        assert!(cfg.contains("- \"127.0.0.53:80\""));

        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&cfg).expect("valid YAML");
        assert_eq!(v["proxy"]["bind"][0].as_str(), Some("127.0.0.53:80"));
        // HTTPS is additive: :443 is offered *and* :80 stays, so nothing that speaks plain HTTP to
        // the front door breaks when TLS lands.
        assert_eq!(v["proxy"]["tls_bind"][0].as_str(), Some("127.0.0.53:443"));

        for (name, host) in [("app", "app.adi"), ("api", "api.adi")] {
            let svc = &v["services"][name];
            assert_eq!(svc["proxy"]["host"].as_str(), Some(host));
            assert_eq!(
                svc["rollout"]["recreate"]["ports"]["http"].as_u64(),
                Some(8091)
            );
            assert!(svc["runner"].is_null());
        }
    }

    // MARK: mesh (docs/fleet.md F1/F2)

    /// A node's front door has to fan in dashboards and projects, and route them without running
    /// them. Both halves were missing: dashboards were scaffolded and never reachable by any
    /// hostname, and the mesh gateway — which answers `<service>.<node>.n.adi` out of this very
    /// table — refused them with "no such service". Found by installing a node in a container.
    #[test]
    fn the_generated_front_door_routes_dashboards_without_running_them() {
        let cfg = rendered(&[]);
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&cfg).expect("valid YAML");

        assert_eq!(
            v["proxy"]["routes_only"].as_bool(),
            Some(true),
            "the front door must say it only routes; inferring it from uid is what broke on Linux"
        );

        let imports: Vec<&str> = v["imports"]
            .as_sequence()
            .expect("imports")
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .collect();
        assert!(
            imports.iter().any(|i| i.contains("ADI_DASHBOARDS_DIR")),
            "no dashboard import means no dashboard is ever routed: {imports:?}"
        );
        assert!(
            imports.iter().any(|i| i.contains("ADI_PROJECTS_DIR")),
            "projects declare hosts too: {imports:?}"
        );
    }

    /// The node list must land in the config the front door is *started with*. A machine with a
    /// hand-managed `hive/hive.yaml` runs that one, so writing the list into the generated file
    /// there is bookkeeping nobody reads — every pairing looked successful while the running
    /// front door kept minting a certificate without the node's name, and `https://` failed on a
    /// host the machine believed it had covered.
    #[test]
    fn the_node_list_follows_the_front_door_that_is_actually_running() {
        let (store, root) = scratch_store("frontdoor-in-use");
        let generated = frontdoor_config_path_in(&store);
        std::fs::create_dir_all(generated.parent().expect("dir")).expect("mkdir");
        std::fs::write(&generated, "proxy:\n  mesh_nodes: []\nservices: {}\n").expect("write");

        // Only the generated one exists: it is the front door.
        assert_eq!(front_door_in_use(&store), generated);

        // A hand-managed config appears — now *that* is what the daemon runs.
        let hand_managed = store.module("hive").raw_path("hive.yaml");
        std::fs::create_dir_all(hand_managed.parent().expect("dir")).expect("mkdir");
        std::fs::write(&hand_managed, "proxy:\n  mesh_nodes: []\nservices: {}\n").expect("write");
        assert_eq!(front_door_in_use(&store), hand_managed);

        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", true),
            MeshNodeChange::Changed
        );
        assert!(
            std::fs::read_to_string(&hand_managed)
                .expect("read")
                .contains("laptop-b"),
            "the live config is the one that gains the node"
        );
        assert!(
            !std::fs::read_to_string(&generated)
                .expect("read")
                .contains("laptop-b"),
            "and the one nobody runs is left alone"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// F1: the generated front door points `*.n.adi` at the local gateway. Without this the route
    /// exists in adi-hive's code but no real machine ever takes it.
    #[test]
    fn frontdoor_hive_points_the_mesh_zone_at_the_local_gateway() {
        let cfg = rendered(&[]);
        assert!(cfg.contains("mesh_gateway"), "{cfg}");

        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&cfg).expect("valid YAML");
        assert_eq!(
            v["proxy"]["mesh_gateway"].as_str(),
            Some("127.0.0.1:10080"),
            "the rendered address must match MESH_GATEWAY_PORT, which adi-mesh's gateway binds"
        );
        // Loopback only: the gateway holds the fleet's keys.
        assert!(mesh_gateway_addr().ip().is_loopback());
        // Clear of the ports manager's allocation range, so no project can ever be handed it.
        assert!(!(8000..=9999).contains(&MESH_GATEWAY_PORT));
    }

    /// The whole point of emitting these keys is that adi-hive reads them, so parse the rendered
    /// file with adi-hive's own config type rather than a generic YAML value.
    #[test]
    fn frontdoor_hive_parses_as_an_adi_hive_config() {
        let hive: adi_hive::config::Hive =
            serde_yaml_ng::from_str(&rendered(&["laptop-b", "tower"])).expect("hive config");
        assert_eq!(hive.proxy.mesh_gateway, Some(mesh_gateway_addr()));
        assert_eq!(hive.proxy.mesh_nodes, ["laptop-b", "tower"]);
        assert_eq!(hive.proxy.bind.len(), 1);
        assert_eq!(hive.proxy.tls_bind.len(), 1);
        assert_eq!(hive.services.len(), 2);

        // An empty list is still a list — a machine with no fleet must parse identically.
        let bare: adi_hive::config::Hive =
            serde_yaml_ng::from_str(&rendered(&[])).expect("hive config");
        assert!(bare.proxy.mesh_nodes.is_empty());
        assert_eq!(bare.proxy.mesh_gateway, Some(mesh_gateway_addr()));
    }

    /// The line the patcher writes has to be the line the renderer writes, or every pairing would
    /// leave the config looking stale and prompt for a password to "refresh" it.
    #[test]
    fn a_patched_config_is_byte_identical_to_a_fresh_render() {
        let patched = patch_mesh_nodes(&rendered(&[]), &["laptop-b".to_string()]);
        assert_eq!(patched, rendered(&["laptop-b"]));
    }

    #[test]
    fn mesh_nodes_round_trip_through_the_rendered_file() {
        assert!(parse_mesh_nodes(&rendered(&[])).is_empty());
        assert_eq!(parse_mesh_nodes(&rendered(&["a", "b-2"])), ["a", "b-2"]);
        // A file from before the mesh existed simply has no nodes.
        assert!(parse_mesh_nodes("proxy:\n  bind:\n    - \"127.0.0.53:80\"\n").is_empty());
        // Block form, in case an operator pre-seeds a node by hand.
        assert_eq!(
            parse_mesh_nodes("proxy:\n  mesh_nodes:\n    - laptop-b\n    - \"tower\"\nservices:\n"),
            ["laptop-b", "tower"]
        );
    }

    /// F2: pairing records a petname, and doing it twice is not a second entry.
    #[test]
    fn adding_a_mesh_node_is_idempotent() {
        let (store, root) = scratch_store("add");
        let path = seed_frontdoor(&store, &[]);

        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", true),
            MeshNodeChange::Changed
        );
        let after_first = std::fs::read_to_string(&path).expect("read");
        assert_eq!(parse_mesh_nodes(&after_first), ["laptop-b"]);

        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", true),
            MeshNodeChange::Unchanged
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            after_first,
            "a re-pair must not touch a byte"
        );

        // A second node joins the first rather than replacing it.
        assert_eq!(
            set_mesh_node_in(&store, "tower", true),
            MeshNodeChange::Changed
        );
        assert_eq!(
            parse_mesh_nodes(&std::fs::read_to_string(&path).expect("read")),
            ["laptop-b", "tower"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removing_a_mesh_node_drops_only_that_node() {
        let (store, root) = scratch_store("remove");
        let path = seed_frontdoor(&store, &["laptop-b", "tower"]);

        assert_eq!(
            set_mesh_node_in(&store, "tower", false),
            MeshNodeChange::Changed
        );
        assert_eq!(
            parse_mesh_nodes(&std::fs::read_to_string(&path).expect("read")),
            ["laptop-b"]
        );

        // Removing something that was never there is a no-op, not a failure — unpairing is
        // retried and a rename removes a petname that may already be gone.
        let before = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            set_mesh_node_in(&store, "tower", false),
            MeshNodeChange::Unchanged
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read"), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Neither direction may disturb anything else in the file: not the comments an operator
    /// reads, not the hosts, not the ports, not the gateway.
    #[test]
    fn editing_the_node_list_leaves_the_rest_of_the_file_alone() {
        let (store, root) = scratch_store("preserve");
        let path = seed_frontdoor(&store, &[]);
        let original = std::fs::read_to_string(&path).expect("read");

        let _ = set_mesh_node_in(&store, "laptop-b", true);
        let _ = set_mesh_node_in(&store, "tower", true);
        let _ = set_mesh_node_in(&store, "laptop-b", false);
        let _ = set_mesh_node_in(&store, "tower", false);

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "add-then-remove must return the file to exactly what it was"
        );

        // And with a node listed, everything but that one line still matches.
        let _ = set_mesh_node_in(&store, "laptop-b", true);
        let with_node = std::fs::read_to_string(&path).expect("read");
        let differing: Vec<_> = with_node
            .lines()
            .zip(original.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(differing.len(), 1, "{differing:?}");
        assert!(differing[0].0.starts_with(MESH_NODES_KEY));
        assert_eq!(with_node.lines().count(), original.lines().count());

        // Still an adi-hive config, and still routing the same hosts to the same port.
        let hive: adi_hive::config::Hive = serde_yaml_ng::from_str(&with_node).expect("hive");
        assert_eq!(hive.proxy.mesh_nodes, ["laptop-b"]);
        assert_eq!(hive.proxy.mesh_gateway, Some(mesh_gateway_addr()));
        assert_eq!(hive.services.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A missing or unusable front door degrades to "HTTPS warns", never to a panic or an error
    /// the pairing flow has to handle.
    #[test]
    fn an_unrecordable_node_fails_softly() {
        let (store, root) = scratch_store("soft");

        // No front door generated yet: recording a node is a real miss, dropping one is already
        // true — an unpair on a machine with no front door must not warn about nothing.
        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", true),
            MeshNodeChange::Failed
        );
        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", false),
            MeshNodeChange::Unchanged
        );

        // A config with nowhere to put the line.
        let path = frontdoor_config_path_in(&store);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "services: {}\n").expect("write");
        assert_eq!(
            set_mesh_node_in(&store, "laptop-b", true),
            MeshNodeChange::Failed
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "services: {}\n"
        );

        // A petname that could never be a certificate name is refused before anything is written.
        seed_frontdoor(&store, &[]);
        for bad in ["", "Laptop-B", "a.b", "-lead", "trail-", "under_score"] {
            assert_eq!(
                set_mesh_node_in(&store, bad, true),
                MeshNodeChange::Failed,
                "{bad:?}"
            );
        }
        assert!(parse_mesh_nodes(&std::fs::read_to_string(&path).expect("read")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn frontdoor_settings_default_to_app_and_api_dot_adi() {
        assert_eq!(FrontdoorSettings::default().hosts, ["app.adi", "api.adi"]);
    }

    #[test]
    fn route_action_reflects_installed_state() {
        assert_eq!(route_action(false).args, vec!["dns", "install-route"]);
        assert_eq!(route_action(true).args, vec!["dns", "remove-route"]);
    }

    // MARK: Linux — a node's route and front door (docs/fleet.md §6)
    //
    // None of this runs on the host that builds it, and it will not run on macOS or Windows at
    // all. So the tests are about *data*: the exact bytes that would be written and the exact argv
    // that would be spawned. Nothing below executes a command.

    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    /// The drop-in has to be the file `adi-dns` itself defines, byte for byte — adi-core writes it
    /// only because the resolver runs unprivileged and cannot. A drift here means two files, or a
    /// file `adi-dns`'s own uninstall would not recognise.
    #[test]
    fn the_linux_drop_in_is_adi_dnss_own_file() {
        assert_eq!(
            linux_plan::resolved_drop_in_path("adi"),
            "/etc/systemd/resolved.conf.d/adi-dns-adi.conf"
        );
        assert_eq!(
            linux_plan::resolved_drop_in("adi", loopback(10053)),
            "# Managed by adi-dns. Split-DNS: route only .adi to this resolver.\n\
             [Resolve]\n\
             DNS=127.0.0.1:10053\n\
             Domains=~adi\n"
        );
    }

    /// `~` is what makes it a *routing-only* domain: `.adi` goes to the local resolver and every
    /// other name on the machine is untouched. Without the tilde this would make adi-dns the
    /// search domain and could take the node's whole DNS with it.
    #[test]
    fn the_linux_route_is_routing_only_and_names_the_resolvers_port() {
        let drop_in = linux_plan::resolved_drop_in(domain(), resolver_addr());
        assert!(drop_in.contains("Domains=~adi\n"), "{drop_in}");
        assert!(!drop_in.contains("Domains=adi"), "{drop_in}");

        // The route must point at the socket the generated adi-dns.toml actually binds — asked of
        // `resolver_addr` rather than of a literal, so the two cannot drift apart per platform.
        let addr = resolver_addr();
        let cfg = render_config();
        assert!(
            cfg.contains(&format!("bind_addr = \"{}\"", addr.ip())),
            "{cfg}"
        );
        assert!(
            cfg.contains(&format!("preferred_port = {}", addr.port())),
            "{cfg}"
        );
        // The `DNS=` line names that whole socket. Restated here rather than reused, so the rule
        // is asserted independently of the renderer that implements it.
        let expected = if addr.port() == 53 {
            format!("DNS={}\n", addr.ip())
        } else {
            format!("DNS={}:{}\n", addr.ip(), addr.port())
        };
        assert!(drop_in.contains(&expected), "{drop_in}");

        // On 53 the port is omitted — adi-dns's rule, kept so the two renderers stay identical.
        let on_53 = linux_plan::resolved_drop_in("adi", loopback(53));
        assert!(on_53.contains("DNS=127.0.0.1\n"), "{on_53}");
    }

    /// The privileged half is a copy of a file staged unprivileged, so nothing about the
    /// resolver's address is ever interpolated into a root shell.
    #[test]
    fn linux_route_steps_only_copy_a_staged_file_and_reload() {
        let steps = linux_plan::route_install_steps(
            "/home/n/.adi/mono/dns/resolved-adi.conf",
            "/etc/systemd/resolved.conf.d/adi-dns-adi.conf",
        );
        assert_eq!(
            steps,
            [
                "mkdir -p /etc/systemd/resolved.conf.d",
                "cp '/home/n/.adi/mono/dns/resolved-adi.conf' '/etc/systemd/resolved.conf.d/adi-dns-adi.conf'",
                "chmod 644 '/etc/systemd/resolved.conf.d/adi-dns-adi.conf'",
                "systemctl restart systemd-resolved",
            ]
        );
        // `&&`, so a failed copy never gets to look like a successful reload.
        assert_eq!(linux_plan::script(&steps), steps.join(" && "));
        assert!(!linux_plan::script(&steps).contains(';'));
    }

    #[test]
    fn linux_route_removal_undoes_exactly_the_install() {
        assert_eq!(
            linux_plan::route_remove_steps("/etc/systemd/resolved.conf.d/adi-dns-adi.conf"),
            [
                "rm -f '/etc/systemd/resolved.conf.d/adi-dns-adi.conf'",
                "systemctl restart systemd-resolved",
            ]
        );
    }

    /// The whole privileged story of the Linux front door is this one line. If it ever grows a
    /// second capability, or `+ei`, that is a privilege decision and this test is where it is made
    /// visible.
    #[test]
    fn the_front_door_is_granted_exactly_one_capability() {
        let bin = "/home/n/.local/adi/bin/adi-hive";
        assert_eq!(
            linux_plan::capability_steps(bin),
            ["setcap 'cap_net_bind_service=+ep' '/home/n/.local/adi/bin/adi-hive'"]
        );
        assert_eq!(
            linux_plan::capability_remove_steps(bin),
            ["setcap -r '/home/n/.local/adi/bin/adi-hive'"]
        );
    }

    /// `-n` is the difference between a node that reports and a node that hangs: an install over
    /// ssh has no terminal for `sudo` to prompt on.
    #[test]
    fn the_privileged_invocation_can_never_prompt() {
        let script = linux_plan::script(&linux_plan::capability_steps("/opt/adi-hive"));
        assert_eq!(
            linux_plan::privileged_argv(&script, false),
            ["sudo", "-n", "/bin/sh", "-c", script.as_str()]
        );
        // Already root: `sudo` is not required to be installed at all.
        assert_eq!(
            linux_plan::privileged_argv(&script, true),
            ["/bin/sh", "-c", script.as_str()]
        );
    }

    /// Read one `/bin/sh` word back, exactly as the shell would, and refuse anything that would
    /// in fact have been *more* than one word — an unquoted space — or a second command — an
    /// unquoted `;`, `&` or `|`. That refusal is the property under test: these words are pasted
    /// into a root shell, so "it still parses" is not enough, it has to still be one argument.
    fn sh_unquote_word(word: &str) -> String {
        let mut out = String::new();
        let mut quoted = false;
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            match c {
                '\'' => quoted = !quoted,
                '\\' if !quoted => out.push(chars.next().expect("dangling escape")),
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '$' | '`' if !quoted => {
                    panic!("{c:?} escaped its quoting in {word:?}")
                }
                _ => out.push(c),
            }
        }
        assert!(!quoted, "unterminated quote in {word:?}");
        out
    }

    /// A home directory with an apostrophe in it is ordinary; ending the quoting mid-path and
    /// handing the rest to a root shell is not.
    #[test]
    fn a_quote_in_a_path_cannot_escape_into_the_root_shell() {
        for path in [
            "/home/o'brien/adi-hive",
            "/home/x'; rm -rf /; :'/adi-hive",
            "/opt/adi tools/adi-hive",
            "/home/$USER/`id`/adi-hive",
        ] {
            let step = linux_plan::capability_steps(path).remove(0);
            let quoted = step
                .strip_prefix("setcap 'cap_net_bind_service=+ep' ")
                .expect("the path is the last word");
            assert_eq!(sh_unquote_word(quoted), path, "{step}");
        }
        // And the spelling itself, so the escape is legible here and not only inside a helper.
        assert_eq!(
            linux_plan::capability_steps("/home/o'brien/adi-hive")[0],
            r"setcap 'cap_net_bind_service=+ep' '/home/o'\''brien/adi-hive'"
        );
    }

    /// When elevation is refused the operator gets commands, not an apology.
    #[test]
    fn the_manual_form_is_every_step_ready_to_paste() {
        let steps = linux_plan::route_install_steps("/stage.conf", "/etc/drop.conf");
        let manual = linux_plan::manual(&steps);
        assert_eq!(manual.lines().count(), steps.len());
        for (line, step) in manual.lines().zip(&steps) {
            assert_eq!(line, format!("    sudo {step}"));
        }
        assert_eq!(
            linux_plan::port_floor_alternative(),
            "sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80"
        );
    }

    /// libcap changed how `getcap` prints, and a file with no capabilities prints nothing at all.
    /// Reading "nothing" as "granted" would mean a front door enabled into a permanent crash loop.
    #[test]
    fn getcap_is_read_in_both_libcap_spellings() {
        assert!(linux_plan::capability_granted(
            "/opt/adi-hive cap_net_bind_service=ep\n"
        ));
        assert!(linux_plan::capability_granted(
            "/opt/adi-hive = cap_net_bind_service+ep\n"
        ));
        assert!(!linux_plan::capability_granted(""));
        assert!(!linux_plan::capability_granted(
            "/opt/adi-hive cap_net_raw=ep\n"
        ));
    }

    /// Mesh-only is a node's *normal* state, so the status line describes it rather than
    /// complaining — but it must never let "Running" be read as "`http://app.adi` works here".
    #[test]
    fn the_status_line_says_what_a_node_is_missing() {
        assert_eq!(linux_plan::detail_suffix(true, true), "");
        assert_eq!(
            linux_plan::detail_suffix(false, true),
            " · .adi not routed locally"
        );
        assert_eq!(linux_plan::detail_suffix(true, false), " · no front door");
        assert_eq!(
            linux_plan::detail_suffix(false, false),
            " · no front door, .adi not routed locally"
        );
    }

    // What a root daemon may be asked to run. The verdict is a pure function of an owner and a
    // mode precisely so it can be tested without a root-owned file to point at — the walk below
    // then only has to prove that it looks at every component and stops at the first bad one.

    #[cfg(unix)]
    #[test]
    fn only_a_root_owned_component_that_nobody_else_may_write_is_accepted() {
        assert_eq!(component_verdict(0, 0o755), None);
        assert_eq!(component_verdict(0, 0o700), None);
        // Sticky and setuid bits are none of this check's business.
        assert_eq!(component_verdict(0, 0o1755), None);

        // An ordinary user's file, whatever its mode says: the owner can change the mode.
        assert_eq!(component_verdict(501, 0o755), Some(Unsafe::OwnedBy(501)));
        assert_eq!(component_verdict(501, 0o700), Some(Unsafe::OwnedBy(501)));

        // Root-owned, but handed to a group or to the world — on macOS `root:admin 0775` means
        // every administrator account on the machine.
        assert_eq!(component_verdict(0, 0o775), Some(Unsafe::Writable(0o775)));
        assert_eq!(component_verdict(0, 0o757), Some(Unsafe::Writable(0o757)));
    }

    /// A directory above the program counts as much as the program: whoever may write it can
    /// rename a different file into place.
    #[cfg(unix)]
    #[test]
    fn a_user_owned_program_or_parent_is_refused_and_named() {
        let dir = std::env::temp_dir().join(format!("adi-dns-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("adi-hive");
        std::fs::write(&program, b"#!/bin/sh\n").unwrap();

        // A temp dir belongs to the user running the tests, so this is the user-owned case.
        let (component, verdict) = first_unsafe_component(&program).expect("must object");
        assert!(matches!(verdict, Unsafe::OwnedBy(_)), "{verdict:?}");
        // The first offending component going *up* is the program itself.
        assert_eq!(component, std::fs::canonicalize(&program).unwrap());

        let objection = root_program_objection(&program).expect("must refuse");
        assert!(objection.contains("adi-hive"), "{objection}");
        assert!(objection.contains(ALLOW_UNSAFE_PROGRAM_ENV), "{objection}");
        assert!(objection.contains("run code as root"), "{objection}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The path the check is meant to accept: root-owned all the way up, writable by nobody else.
    /// `/bin/sh` is `root:wheel 0555` on macOS and root-owned `0755` on every Linux this builds on.
    #[cfg(unix)]
    #[test]
    fn a_root_owned_program_is_accepted() {
        assert_eq!(
            first_unsafe_component(std::path::Path::new("/bin/sh")),
            None
        );
        assert_eq!(
            root_program_objection(std::path::Path::new("/bin/sh")),
            None
        );
    }

    /// A path that does not exist yet is judged by the directories above it — which is the case
    /// that matters, since a writable directory is where a missing program comes from.
    #[cfg(unix)]
    #[test]
    fn a_missing_program_is_judged_by_its_parents() {
        assert_eq!(
            first_unsafe_component(std::path::Path::new("/bin/adi-hive-not-here")),
            None
        );
        let home = std::env::var("HOME").expect("a home directory");
        let (component, _) = first_unsafe_component(&PathBuf::from(home).join("adi-hive-not-here"))
            .expect("a user's home is not root-owned");
        assert!(
            component.exists(),
            "{} must be a real component",
            component.display()
        );
    }

    /// The probe has to knock where the resolver sends the browser. Asserted against the
    /// flavour rather than `127.0.0.53:80`, so a `dev` build tests its own address — and so
    /// that pointing the zone somewhere new can never leave the liveness check behind,
    /// silently reporting a healthy front door on an address nothing uses.
    #[test]
    fn the_probe_knocks_where_the_zone_points() {
        let flavour = Flavor::current();
        let addr = frontdoor_probe_addr();
        assert_eq!(addr.ip().to_string(), flavour.frontdoor_addr.to_string());
        assert_eq!(addr.port(), FRONTDOOR_PORT);
        // The rendered front door must bind the address the probe asks, or a working machine
        // reads as broken on every status poll.
        assert!(rendered(&[]).contains(&format!("{}:{}", flavour.frontdoor_addr, FRONTDOOR_PORT)));
    }

    /// The budget is the whole point of the probe: an unaliased loopback address *drops*
    /// packets rather than refusing them, so an unbounded connect would hang the status poll
    /// on exactly the machine it is meant to describe.
    #[test]
    fn the_probe_is_bounded_and_the_confirmation_is_more_patient() {
        assert!(FRONTDOOR_PROBE_TIMEOUT <= Duration::from_millis(500));
        assert!(FRONTDOOR_CONFIRM_TIMEOUT > FRONTDOOR_PROBE_TIMEOUT);
    }

    /// A machine that has never tried must try. Every way of not knowing — no file, no store,
    /// unreadable JSON — has to resolve towards the repair, never away from it.
    #[test]
    fn a_store_with_no_stamp_may_repair() {
        let (store, root) = scratch_store("repair-fresh");
        let path = repair_stamp_path_in(&store);
        assert!(may_repair_at(&path, 1_700_000_000));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "{ this is not json").expect("seed");
        assert!(may_repair_at(&path, 1_700_000_000));
        let _ = std::fs::remove_dir_all(root);
    }

    /// One gesture, one prompt: `up` runs from the app launch, the CLI and the updater, and a
    /// burst of those must not become a burst of password prompts.
    #[test]
    fn a_just_recorded_attempt_holds_the_next_one_off() {
        let (store, root) = scratch_store("repair-cooldown");
        let path = repair_stamp_path_in(&store);
        let now = 1_700_000_000;
        record_repair_at(&path, now);
        assert!(!may_repair_at(&path, now));
        assert!(!may_repair_at(&path, now + REPAIR_COOLDOWN.as_secs() - 1));
        // …and reopening the app later is meant to retry, which is the whole fix.
        assert!(may_repair_at(&path, now + REPAIR_COOLDOWN.as_secs()));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A clock that moved, or a store carried over from another machine, must not lock the
    /// repair out until the present catches up with the stamp.
    #[test]
    fn a_stamp_from_the_future_does_not_lock_the_repair_out() {
        let (store, root) = scratch_store("repair-future");
        let path = repair_stamp_path_in(&store);
        record_repair_at(&path, 2_000_000_000);
        assert!(may_repair_at(&path, 1_700_000_000));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The regression that cost a release. A root daemon may not be pointed at a program an
    /// ordinary user can rewrite — and from 1.0.0 it was pointed at the binary inside the app
    /// bundle, which belongs to whoever dragged the app into `/Applications`. So
    /// `write_frontdoor_artifacts` refused, and *every* install and repair path returned before
    /// raising its prompt: silently, on every Mac, for four releases.
    ///
    /// This asserts the objection against the real path on the machine running the test, which
    /// is the only way it can say anything about the machines it is meant to protect.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_daemon_is_pointed_at_a_program_it_is_allowed_to_run() {
        let program = frontdoor_program_path();
        assert!(
            !program.starts_with("/Applications"),
            "the daemon's program is back inside the bundle: {program}"
        );
        assert_eq!(
            root_program_objection(std::path::Path::new(&program)),
            None,
            "the front door cannot be installed on this machine"
        );
        // Namespaced, or a second install's update swaps the binary under this one's root daemon.
        assert!(
            program.contains(&Flavor::current().app_name),
            "{program} is shared between flavours"
        );
    }

    /// What launchd is actually handed. The plist is the artifact that outlives every decision
    /// above it, so it is asserted directly: the program is the root copy, never the bundle, and
    /// the rendering succeeds at all — for four releases it did not, and the only sign was a
    /// front door that never appeared.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_rendered_daemon_definition_runs_the_root_copy() {
        let plist = render_frontdoor_plist().expect("the front door must be installable");
        assert!(plist.contains(&frontdoor_program_path()), "{plist}");
        assert!(
            !plist.contains(&hive_binary_path()),
            "the daemon is still being pointed into the app bundle: {plist}"
        );
        assert!(plist.contains("ADI_WATCH_SELF"), "{plist}");
        // It has to name the rendered config, or `frontdoor_plist_managed` stops recognising
        // our own daemon and `up` starts treating it as somebody else's.
        assert!(
            plist.contains(frontdoor_config_path().to_string_lossy().as_ref()),
            "{plist}"
        );
    }

    /// The copy is made as root, and moved rather than written: `rename(2)` over a running
    /// executable is allowed on macOS, writing into one is `ETXTBSY` — and this runs while the
    /// front door may well be up.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_program_is_copied_root_owned_and_moved_into_place() {
        let shell = install_program_shell();
        assert!(shell.contains("install -o root -g wheel -m 755"), "{shell}");
        assert!(shell.contains("mv -f"), "{shell}");
        assert!(shell.contains(&hive_binary_path()), "{shell}");
        assert!(shell.contains(&frontdoor_program_path()), "{shell}");
        // The directory has to be root's too: a root-owned binary somebody else may rename over
        // is not a root-owned binary.
        assert!(shell.contains("chown root:wheel"), "{shell}");
    }

    /// The button has to be the *narrow* repair. `install-route` would ask for a password to
    /// rewrite `/etc/resolver`, which in this failure was never the broken half.
    #[test]
    fn the_repair_button_asks_for_the_front_door_only() {
        let action = repair_action();
        assert_eq!(action.args, vec!["dns".to_string(), "grant-network".into()]);
        assert!(action.title.contains(domain()), "{}", action.title);
    }
}
