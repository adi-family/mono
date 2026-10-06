//! Observe listening TCP ports with `lsof` and process-tree usage with `ps`.
//! Missing tools yield empty results or usage; scans never bind sockets.
//! Cache for [`SCAN_TTL`] to share expensive whole-machine subprocesses across requests.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use adi_webapp_api::types::{ProcessUsage, UsedPort};

const SCAN_TTL: Duration = Duration::from_millis(1_500);

static MEMO: Mutex<Option<(Instant, Vec<UsedPort>)>> = Mutex::new(None);

/// Return distinct listening ports in port order, with process-tree usage.
/// Hold the cache lock through the scan so concurrent misses start only one `lsof`.
#[must_use]
pub fn listening_ports() -> Vec<UsedPort> {
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((taken, ports)) = memo.as_ref()
        && is_fresh(*taken, Instant::now())
    {
        return ports.clone();
    }
    let ports = scan();
    *memo = Some((Instant::now(), ports.clone()));
    ports
}

/// Invalidate after starting or stopping a service so the next status read is fresh.
pub fn invalidate() {
    *MEMO.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn is_fresh(taken: Instant, now: Instant) -> bool {
    now.duration_since(taken) < SCAN_TTL
}

fn scan() -> Vec<UsedPort> {
    // `-nP` disables name lookups; `+c0` keeps full command names.
    // `-Fpcn` emits tagged pid, command, and socket-name fields.
    let Ok(output) = Command::new("lsof")
        .args(["+c0", "-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        .output()
    else {
        return Vec::new();
    };
    let mut ports = parse_lsof(&String::from_utf8_lossy(&output.stdout));
    let table = process_table();
    for used in &mut ports {
        used.usage = used.pid.and_then(|pid| table.usage_of(pid));
    }
    ports
}

/// In `lsof -Fpcn`, `p`/`c` fields are process-scoped and `n` is per-socket.
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

/// Parse the tail of an lsof address (`127.0.0.1:8080`, `*:443`, or `[::1]:631`).
fn port_of(name: &str) -> Option<u16> {
    name.rsplit(':').next()?.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Proc {
    ppid: u32,
    /// Percentage of one core, reported by `ps` as a decaying recent average.
    cpu_percent: f32,
    memory_bytes: u64,
    uptime_secs: u64,
}

/// Index parent-child relationships so usage includes a listener's worker processes.
#[derive(Debug, Default)]
struct ProcessTable {
    procs: BTreeMap<u32, Proc>,
    children: BTreeMap<u32, Vec<u32>>,
}

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
    /// Parse headerless `ps` columns: pid, ppid, rss, pcpu, etime.
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

    /// Include descendants; return `None` if the listener exited before this snapshot.
    fn usage_of(&self, pid: u32) -> Option<ProcessUsage> {
        let root = self.procs.get(&pid)?;
        let mut usage = ProcessUsage {
            pid,
            cpu_percent: 0.0,
            memory_bytes: 0,
            processes: 0,
            uptime_secs: root.uptime_secs,
        };
        // Reparenting during a ps snapshot can create cycles; count each process once.
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
    fn a_scan_is_fresh_only_inside_the_ttl() {
        let now = Instant::now();
        let just_under_ttl = SCAN_TTL
            .checked_sub(Duration::from_millis(1))
            .expect("the TTL is longer than a millisecond");
        assert!(is_fresh(now, now), "the scan we just took");
        assert!(
            is_fresh(now, now + just_under_ttl),
            "still inside the window"
        );
        assert!(!is_fresh(now, now + SCAN_TTL), "the window is half-open");
        assert!(!is_fresh(now, now + Duration::from_secs(60)), "long stale");
    }

    #[test]
    fn etime_covers_every_ps_shape() {
        assert_eq!(parse_etime("01:30"), Some(90));
        assert_eq!(parse_etime("02:00:00"), Some(7_200));
        assert_eq!(parse_etime("2-07:22:35"), Some(199_355));
        assert_eq!(parse_etime("junk"), None);
    }

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

    #[test]
    fn a_parentage_cycle_terminates() {
        let out = "100 200 1024 1.0 01:00\n200 100 1024 1.0 01:00\n";
        let table = ProcessTable::parse(out);
        let usage = table.usage_of(100).expect("in the table");
        assert_eq!(usage.processes, 2, "each process counted exactly once");
    }
}
