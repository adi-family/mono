//! Enumerate the TCP ports currently in the `LISTEN` state on this machine, via `lsof`.
//! This is read-only observation — it never binds a socket — so it's safe to run against a
//! live system. Best-effort: if `lsof` is missing or errors, the scan yields an empty list
//! rather than failing the request. macOS/Linux only (this app targets macOS).

use std::collections::BTreeMap;
use std::process::Command;

use adi_webapp_api::types::UsedPort;

/// Every distinct listening TCP port, with the owning process where `lsof` reports one.
/// Sorted by port; deduplicated (a port listening on both IPv4 and IPv6 appears once).
#[must_use]
pub fn listening_ports() -> Vec<UsedPort> {
    // `-nP` skips host/port name lookups (fast, numeric); `+c0` keeps full (untruncated)
    // command names; `-Fpcn` emits machine-readable fields: p<pid>, c<command>, n<addr:port>.
    let Ok(output) = Command::new("lsof")
        .args(["+c0", "-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `lsof -Fpcn` field output into one entry per listening port. Each line is a single
/// field tagged by its first character; `p`/`c` are process-scoped, `n` is per-socket.
fn parse_lsof(out: &str) -> Vec<UsedPort> {
    let mut by_port: BTreeMap<u16, UsedPort> = BTreeMap::new();
    let mut pid: Option<u32> = None;
    let mut command: Option<String> = None;

    for line in out.lines() {
        let mut chars = line.chars();
        let Some(tag) = chars.next() else { continue };
        let rest = chars.as_str();
        match tag {
            'p' => {
                pid = rest.parse().ok();
                command = None;
            }
            'c' => command = Some(rest.to_string()),
            'n' => {
                if let Some(port) = port_of(rest) {
                    by_port.entry(port).or_insert_with(|| UsedPort {
                        port,
                        process: command.clone(),
                        pid,
                    });
                }
            }
            _ => {}
        }
    }
    by_port.into_values().collect()
}

/// The port from an lsof name field like `127.0.0.1:8080`, `*:443`, or `[::1]:631`.
/// A wildcard port (`*`) or otherwise unparseable tail yields `None`.
fn port_of(name: &str) -> Option<u16> {
    name.rsplit(':').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pid_command_and_ports_deduped_and_sorted() {
        // nginx listens on v4:8080 and v6:8080 (one port), sshd on 22.
        let out = "p1234\ncnginx\nn127.0.0.1:8080\nn[::1]:8080\np22\ncsshd\nn*:22\n";
        let ports = parse_lsof(out);
        assert_eq!(ports.len(), 2);
        // Sorted by port: 22 then 8080.
        assert_eq!(ports[0].port, 22);
        assert_eq!(ports[0].process.as_deref(), Some("sshd"));
        assert_eq!(ports[0].pid, Some(22));
        assert_eq!(ports[1].port, 8080);
        assert_eq!(ports[1].process.as_deref(), Some("nginx"));
        assert_eq!(ports[1].pid, Some(1234));
    }

    #[test]
    fn wildcard_and_junk_ports_are_skipped() {
        assert_eq!(port_of("*:*"), None);
        assert_eq!(port_of("127.0.0.1:0"), Some(0));
        assert_eq!(port_of("[::1]:631"), Some(631));
        assert_eq!(port_of("*:443"), Some(443));
    }

    #[test]
    fn empty_output_is_empty() {
        assert!(parse_lsof("").is_empty());
    }
}
