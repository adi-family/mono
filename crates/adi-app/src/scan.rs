//! Enumerate the TCP ports currently in the `LISTEN` state on this machine, via `lsof`, and
//! charge each one what its owning process tree costs, via `ps`. This is read-only observation
//! — it never binds a socket — so it's safe to run against a live system. Best-effort: if
//! `lsof`/`ps` is missing or errors, the scan yields an empty list (or portless usage) rather
//! than failing the request. macOS/Linux only (this app targets macOS).

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use adi_webapp_api::types::{ProcessUsage, UsedPort};

/// Every distinct listening TCP port, with the owning process where `lsof` reports one and
/// what that process tree currently costs. Sorted by port; deduplicated (a port listening on
/// both IPv4 and IPv6 appears once).
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
    let mut ports = parse_lsof(&String::from_utf8_lossy(&output.stdout));
    // One `ps` for the whole machine, then roll it up per port — cheaper and far more
    // consistent than sampling each listener separately.
    let table = process_table();
    for used in &mut ports {
        used.usage = used.pid.and_then(|pid| table.usage_of(pid));
    }
    ports
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
                        usage: None,
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

// MARK: process usage — what the tree behind a port costs

/// One process in the [`ProcessTable`] snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Proc {
    ppid: u32,
    /// CPU as a percentage of one core, as `ps` reports it (a decaying recent average).
    cpu_percent: f32,
    /// Resident set size in bytes.
    memory_bytes: u64,
    /// Seconds since the process started, from `ps` `etime`.
    uptime_secs: u64,
}

/// A single snapshot of the machine's processes, with a parent→children index so a listener's
/// whole tree can be rolled up. A service's port is usually held by a child of the command the
/// hive supervises (`sh -c "bun run …"`), and servers fork workers, so per-process numbers on
/// their own describe almost nothing.
#[derive(Debug, Default)]
struct ProcessTable {
    procs: BTreeMap<u32, Proc>,
    children: BTreeMap<u32, Vec<u32>>,
}

/// Sample every process on the machine once. An unavailable or unparseable `ps` yields an empty
/// table, which reports no usage rather than failing the scan.
fn process_table() -> ProcessTable {
    let Ok(output) = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,rss=,pcpu=,etime="])
        .output()
    else {
        return ProcessTable::default();
    };
    ProcessTable::parse(&String::from_utf8_lossy(&output.stdout))
}

impl ProcessTable {
    /// Parse `ps -Ao pid=,ppid=,rss=,pcpu=,etime=` output — five whitespace-separated columns,
    /// no header. Rows that don't parse are skipped, not fatal.
    fn parse(out: &str) -> Self {
        let mut procs = BTreeMap::new();
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for line in out.lines() {
            let mut fields = line.split_whitespace();
            let (Some(pid), Some(parent), Some(rss), Some(cpu), Some(etime)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>()) else {
                continue;
            };
            procs.insert(
                pid,
                Proc {
                    ppid: parent,
                    cpu_percent: cpu.parse().unwrap_or(0.0),
                    // `rss` is in KiB on both macOS and Linux.
                    memory_bytes: rss.parse::<u64>().unwrap_or(0).saturating_mul(1024),
                    uptime_secs: parse_etime(etime).unwrap_or(0),
                },
            );
            children.entry(parent).or_default().push(pid);
        }
        Self { procs, children }
    }

    /// Roll `pid` and every descendant into one sample, or `None` if the pid is not in the
    /// snapshot (it exited between the port scan and this one).
    fn usage_of(&self, pid: u32) -> Option<ProcessUsage> {
        let root = self.procs.get(&pid)?;
        let mut usage = ProcessUsage {
            pid,
            cpu_percent: 0.0,
            memory_bytes: 0,
            processes: 0,
            uptime_secs: root.uptime_secs,
        };
        // Iterative walk with a visited set: `ps` snapshots can disagree with themselves about
        // parentage (a reparented process sampled mid-flight), and a cycle here would hang.
        let mut seen = BTreeSet::new();
        let mut stack = vec![pid];
        while let Some(next) = stack.pop() {
            if !seen.insert(next) {
                continue;
            }
            let Some(p) = self.procs.get(&next) else {
                continue;
            };
            usage.cpu_percent += p.cpu_percent;
            usage.memory_bytes = usage.memory_bytes.saturating_add(p.memory_bytes);
            usage.processes += 1;
            if let Some(kids) = self.children.get(&next) {
                stack.extend(kids.iter().copied());
            }
        }
        Some(usage)
    }
}

/// Parse a `ps` `etime` (`[[dd-]hh:]mm:ss`) into seconds.
fn parse_etime(raw: &str) -> Option<u64> {
    let (days, clock) = match raw.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, raw),
    };
    // Read the clock right-to-left, so `mm:ss` and `hh:mm:ss` share one path.
    let mut secs = 0;
    for (i, part) in clock.rsplit(':').enumerate() {
        let value = part.parse::<u64>().ok()?;
        secs += value * [1, 60, 3_600].get(i).copied()?;
    }
    Some(days * 86_400 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pid_command_and_ports_deduped_and_sorted() {
        let out = "p1234\ncnginx\nn127.0.0.1:8080\nn[::1]:8080\np22\ncsshd\nn*:22\n";
        let ports = parse_lsof(out);
        assert_eq!(ports.len(), 2);
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

    #[test]
    fn etime_covers_every_ps_shape() {
        assert_eq!(parse_etime("01:30"), Some(90));
        assert_eq!(parse_etime("02:00:00"), Some(7_200));
        assert_eq!(parse_etime("2-07:22:35"), Some(199_355));
        assert_eq!(parse_etime("junk"), None);
    }

    /// The core of the feature: a listener's numbers include its children, not just itself.
    #[test]
    fn usage_rolls_up_the_whole_process_tree() {
        let out = concat!(
            // pid ppid  rss   cpu  etime
            "  100    1  1024   1.5 01:00\n",
            "  200  100  2048  10.0 00:30\n",
            "  300  200  1024   0.5 00:10\n",
            "  400    1  9999  99.0 05:00\n",
        );
        let table = ProcessTable::parse(out);

        let usage = table.usage_of(100).expect("the listener is in the table");
        assert_eq!(usage.pid, 100);
        assert_eq!(usage.processes, 3, "the listener plus both descendants");
        assert!((usage.cpu_percent - 12.0).abs() < 0.001);
        assert_eq!(usage.memory_bytes, 4096 * 1024);
        assert_eq!(
            usage.uptime_secs, 60,
            "uptime is the listener's own, not the tree's"
        );

        // A leaf is charged only for itself; an unrelated tree never leaks in.
        let leaf = table.usage_of(300).expect("a leaf is still a tree of one");
        assert_eq!(leaf.processes, 1);
        assert_eq!(leaf.memory_bytes, 1024 * 1024);
    }

    #[test]
    fn usage_of_an_unknown_or_unsampled_pid_is_none() {
        let table = ProcessTable::parse("100 1 1024 1.5 01:00\n");
        assert!(table.usage_of(999).is_none(), "a pid that already exited");
        assert!(ProcessTable::default().usage_of(100).is_none(), "no `ps`");
    }

    /// A snapshot that claims a process is its own ancestor must not spin forever.
    #[test]
    fn a_parentage_cycle_terminates() {
        let out = "100 200 1024 1.0 01:00\n200 100 1024 1.0 01:00\n";
        let table = ProcessTable::parse(out);
        let usage = table.usage_of(100).expect("in the table");
        assert_eq!(usage.processes, 2, "each process counted exactly once");
    }
}
