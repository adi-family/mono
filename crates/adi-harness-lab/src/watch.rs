//! `watch` — stand over the live harness and catch a timeout overshoot in the act.
//!
//! The overshoot is real in production (a 120s deadline reported at 885s, corroborated by the
//! CLI's own `elapsed_time_seconds` heartbeat) but does not reproduce synthetically: no amount of
//! CPU, disk, memory or real-build pressure applied in the lab pushed it past 1.7x. So instead of
//! trying to recreate the conditions, this waits for the next real one and photographs it.
//!
//! It does two things at once:
//!
//! * **Every tick** it appends one line of machine state to `machine.jsonl` — load, memory
//!   pressure, swap and compressor counters. Cheap enough to leave running for days, and it gives
//!   the "what was the machine doing" context that the harness logs alone cannot answer.
//!
//! * **Continuously** it tails every live harness log, pairs each `tool_use` with its deadline,
//!   and watches the `tool_progress` heartbeats that follow. The moment a heartbeat exceeds that
//!   call's own deadline by more than `--factor`, it writes a full snapshot: the command, the
//!   deadline, the elapsed the CLI itself reported, the whole process table, and — the part a
//!   vendor actually needs — a `sample` of that conversation's `claude` process, showing where its
//!   event loop was standing while its own timer was late.
//!
//! Nothing here touches a service: it reads files, reads `sysctl`, and samples only the `claude`
//! pid recorded by the conversation being watched.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The CLI's own defaults, read out of the 2.1.220 binary: a call with no `timeout` gets 120s, and
/// whatever it asks for is capped at 600s. Keeping them here means a heartbeat can be judged
/// against the deadline the call actually had, not a guess.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

pub struct Watch {
    pub root: PathBuf,
    pub out: PathBuf,
    pub interval: Duration,
    pub factor: f64,
    pub duration: Option<Duration>,
    pub sample_pid: bool,
    /// Read existing log content too. Off by default: only bytes written while the watch is up
    /// can be a live finding. Useful to validate the detector against a known-bad historical log.
    pub replay: bool,
}

/// One in-flight tool call: what it asked for, so its heartbeats can be judged.
struct Pending {
    command: String,
    deadline_ms: u64,
    agent: String,
    conv: String,
    seen_at: u64,
    reported: bool,
}

pub fn run(w: &Watch) {
    if let Err(e) = std::fs::create_dir_all(w.out.join("stalls")) {
        eprintln!("cannot create {}: {e}", w.out.display());
        return;
    }
    let machine_path = w.out.join("machine.jsonl");
    println!(
        "watch: {} every {:?}, stall factor {}x -> {}",
        w.root.display(),
        w.interval,
        w.factor,
        w.out.display()
    );
    println!("       Ctrl-C to stop\n");

    let mut offsets: HashMap<PathBuf, u64> = HashMap::new();
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut stalls = 0usize;
    let started = Instant::now();

    loop {
        let sample = machine_sample();
        append_line(&machine_path, &sample);

        for log in harness_logs(&w.root) {
            scan_log(&log, &mut offsets, &mut pending, w, &mut stalls);
        }
        // A call whose result never arrives would leak; drop anything older than an hour.
        let now = now_ms();
        pending.retain(|_, p| now.saturating_sub(p.seen_at) < 3_600_000);

        if let Some(d) = w.duration
            && started.elapsed() >= d
        {
            println!("\nwatch: duration reached; {stalls} stall(s) captured");
            return;
        }
        std::thread::sleep(w.interval);
    }
}

// ---------------------------------------------------------------- machine state

/// One tick of machine state, as a JSON line. Deliberately flat so it can be loaded straight into
/// a dataframe next to the harness timings.
fn machine_sample() -> String {
    let (l1, l5, l15) = loadavg();
    let vm = vm_stat();
    let get = |k: &str| vm.get(k).copied().unwrap_or(0);
    format!(
        r#"{{"at":{},"load1":{l1:.2},"load5":{l5:.2},"load15":{l15:.2},"pressure":{},"pages_free":{},"swapins":{},"swapouts":{},"compressor_pages":{},"decompressions":{},"pageouts":{}}}"#,
        now_ms(),
        pressure_level(),
        get("Pages free"),
        get("Swapins"),
        get("Swapouts"),
        get("Pages stored in compressor"),
        get("Decompressions"),
        get("Pageouts"),
    )
}

fn loadavg() -> (f64, f64, f64) {
    let mut v = [0f64; 3];
    // SAFETY: `getloadavg` fills at most `nelem` doubles into the caller's array.
    let n = unsafe { getloadavg(v.as_mut_ptr(), 3) };
    if n < 3 { (0.0, 0.0, 0.0) } else { (v[0], v[1], v[2]) }
}

unsafe extern "C" {
    fn getloadavg(loadavg: *mut f64, nelem: i32) -> i32;
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical.
fn pressure_level() -> i32 {
    sh_out("sysctl", &["-n", "kern.memorystatus_vm_pressure_level"])
        .trim()
        .parse()
        .unwrap_or(-1)
}

/// `vm_stat` as a map of its labels to counts, with the trailing period stripped.
fn vm_stat() -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for line in sh_out("vm_stat", &[]).lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_end_matches('.');
        if let Ok(n) = v.parse::<u64>() {
            out.insert(k.trim().to_string(), n);
        }
    }
    out
}

// ---------------------------------------------------------------- log tailing

/// Every harness log under the store, live or not — cheap enough to re-glob each tick, and it
/// picks up conversations that start after the watch does.
fn harness_logs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let base = root.join("sessions").join("harness");
    let Ok(agents) = std::fs::read_dir(&base) else {
        return out;
    };
    for a in agents.flatten() {
        let Ok(files) = std::fs::read_dir(a.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "log") {
                out.push(p);
            }
        }
    }
    out
}

/// Read whatever is new in one log, tracking tool calls and judging their heartbeats.
fn scan_log(
    log: &Path,
    offsets: &mut HashMap<PathBuf, u64>,
    pending: &mut HashMap<String, Pending>,
    w: &Watch,
    stalls: &mut usize,
) {
    let Ok(meta) = std::fs::metadata(log) else {
        return;
    };
    let len = meta.len();
    // A log first seen is history, not news: start at its end. Replaying what is already on disk
    // would pair a months-old heartbeat with today's machine state — a capture that reads like a
    // live event and is worth nothing. Only bytes written while the watch is up can be a finding.
    let first_sight = !offsets.contains_key(log);
    let seen = offsets.entry(log.to_path_buf()).or_insert(0);
    if first_sight && !w.replay {
        *seen = len;
        return;
    }
    // A turn recreates the log, so a shrink means "start over", not "nothing new".
    if len < *seen {
        *seen = 0;
    }
    if len == *seen {
        return;
    }
    let Ok(text) = std::fs::read_to_string(log) else {
        return;
    };
    let from = usize::try_from(*seen).unwrap_or(0);
    let tail = text.get(from..).unwrap_or("");
    // Only whole lines; leave a partial last line for the next tick.
    let end = tail.rfind('\n').map_or(0, |i| i + 1);
    if end == 0 {
        return;
    }
    *seen += end as u64;

    let (agent, conv) = agent_conv(log);
    for line in tail[..end].lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                let blocks = v.pointer("/message/content").and_then(serde_json::Value::as_array);
                for b in blocks.into_iter().flatten() {
                    if b.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    if b.get("name").and_then(serde_json::Value::as_str) != Some("Bash") {
                        continue;
                    }
                    let id = b.get("id").and_then(serde_json::Value::as_str).unwrap_or_default();
                    let asked = b
                        .pointer("/input/timeout")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(DEFAULT_TIMEOUT_MS);
                    let command = b
                        .pointer("/input/command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    pending.insert(
                        id.to_string(),
                        Pending {
                            command,
                            deadline_ms: asked.min(MAX_TIMEOUT_MS),
                            agent: agent.clone(),
                            conv: conv.clone(),
                            seen_at: now_ms(),
                            reported: false,
                        },
                    );
                }
            }
            Some("tool_progress") => {
                let Some(parent) = v.get("parent_tool_use_id").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let elapsed_s = v
                    .get("elapsed_time_seconds")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                let Some(p) = pending.get_mut(parent) else {
                    continue;
                };
                if p.reported {
                    continue;
                }
                let deadline_s = p.deadline_ms as f64 / 1e3;
                if elapsed_s > deadline_s * w.factor {
                    p.reported = true;
                    *stalls += 1;
                    capture(w, p, elapsed_s, deadline_s, *stalls);
                }
            }
            Some("user") => {
                let blocks = v.pointer("/message/content").and_then(serde_json::Value::as_array);
                for b in blocks.into_iter().flatten() {
                    if b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
                        && let Some(id) = b.get("tool_use_id").and_then(serde_json::Value::as_str)
                    {
                        pending.remove(id);
                    }
                }
            }
            _ => {}
        }
    }
}

fn agent_conv(log: &Path) -> (String, String) {
    let conv = log
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let agent = log
        .parent()
        .and_then(Path::file_name)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (agent, conv)
}

// ---------------------------------------------------------------- the capture

/// Photograph the machine, and the stalled CLI, while the overshoot is still happening.
fn capture(w: &Watch, p: &Pending, elapsed_s: f64, deadline_s: f64, n: usize) {
    let at = now_ms();
    let stem = format!("{at}-{}-{}", p.agent, p.conv);
    let dir = w.out.join("stalls");

    println!(
        "!! STALL #{n}  {}/{}  deadline {deadline_s:.0}s, CLI reports {elapsed_s:.0}s elapsed ({:.1}x)",
        p.agent,
        p.conv,
        elapsed_s / deadline_s
    );
    println!("   {}", p.command.replace('\n', " ").chars().take(90).collect::<String>());

    let (l1, l5, l15) = loadavg();
    let vm = vm_stat();
    let pid = conv_pid(&w.root, &p.agent, &p.conv);
    let record = serde_json::json!({
        "at": at,
        "agent": p.agent,
        "conv": p.conv,
        "command": p.command,
        "deadline_s": deadline_s,
        "cli_reported_elapsed_s": elapsed_s,
        "overshoot_factor": elapsed_s / deadline_s,
        "load": [l1, l5, l15],
        "pressure": pressure_level(),
        "vm_stat": vm,
        "claude_pid": pid,
        "processes": sh_out("ps", &["-Ao", "pid,ppid,%cpu,%mem,rss,stat,etime,comm"]),
    });
    let _ = std::fs::write(
        dir.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&record).unwrap_or_default(),
    );

    // The one artifact a vendor cannot get any other way: where the CLI's own event loop was
    // standing while its own timer was late. Only the conversation's `claude` pid is sampled.
    if w.sample_pid && let Some(pid) = pid {
        println!("   sampling claude pid {pid} for 5s…");
        let out = sh_out("sample", &[&pid.to_string(), "5", "-mayDie", "-f", "/dev/stdout"]);
        let _ = std::fs::write(dir.join(format!("{stem}-sample.txt")), out);
    }
    println!("   written to {}", dir.join(format!("{stem}.json")).display());
}

/// The `claude` pid this conversation recorded, when it is still alive.
fn conv_pid(root: &Path, agent: &str, conv: &str) -> Option<i32> {
    let p = root
        .join("sessions")
        .join("harness")
        .join(agent)
        .join(format!("{conv}.pid"));
    let pid: i32 = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
    // SAFETY: signal 0 only performs the existence/permission check.
    (unsafe { kill(pid, 0) } == 0).then_some(pid)
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

// ---------------------------------------------------------------- small helpers

fn sh_out(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn append_line(path: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
