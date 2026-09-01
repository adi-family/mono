//! `adi-harness-lab` — a measuring rig for the `harness:claude-sdk` backend.
//!
//! It exists to answer one question with numbers instead of guesses: when a harness run takes
//! twenty minutes to do thirty seconds of work, *where did the time go*? It drives the real
//! [`adi_agents`] API — the same `force_run_in` / `peek_run` the app server calls — so whatever it
//! measures is the production path, not a re-implementation of it.
//!
//! Three probes, matching the three places time can hide:
//!
//! * `spawn`  — launch N concurrent runs of a fixed, trivial workload and report the per-Bash-call
//!              latency each one saw. Trivial commands cost nothing to execute, so anything above
//!              the ~0.2s floor is harness overhead, not the command.
//! * `poll`   — hammer `peek_run` from several threads while runs are live. The app server polls an
//!              open chat twice a second, and every poll of a growing log misses the memo cache and
//!              re-parses the whole thing. If that path blocks — on the memo mutex, or just on
//!              megabytes of `serde_json` — it shows up here as peek latency.
//! * `parse`  — time `peek_run` against logs that already exist, so the cost of a memo miss can be
//!              read directly off the real files a long run leaves behind.
//!
//! Only `harness:claude-sdk` is wired today; the other backends come later.

mod watch;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adi_agents::{Agents, Backend, StoredAgent};

/// The workload every probe uses: commands that do nothing, so their measured cost *is* the
/// harness's cost. One Bash call each — the prompt is explicit about not combining them.
const TRIVIAL: &[&str] = &[
    "echo probe-1",
    "pwd",
    "echo probe-2",
    "date +%s",
    "echo probe-3",
    "whoami",
    "echo probe-4",
    "echo probe-5",
];

/// A command whose output is enormous, followed by commands that do nothing. If a single fat
/// tool result blocks the CLI's event loop, the *trivial calls after it* are what show the cost —
/// which is exactly the production signature: a timeout set to 3s that fired at 106s.
const BIGOUT: &[&str] = &[
    "echo warmup",
    "seq 1 3000000",
    "echo after-big-1",
    "echo after-big-2",
    "seq 1 6000000",
    "echo after-big-3",
    "echo after-big-4",
];

/// What the app server does to an open chat, from `memo`'s doc comment.
const POLL_HZ: f64 = 2.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let opts = Opts::parse(&args);

    match cmd {
        "spawn" => cmd_spawn(&opts),
        "poll" => cmd_poll(&opts),
        "parse" => cmd_parse(&opts),
        "conv" => cmd_conv(&opts),
        "probe" => cmd_probe(&opts),
        "mem" => cmd_mem(&opts),
        "watch" => cmd_watch(&opts),
        _ => usage(),
    }
}

fn usage() {
    eprintln!(
        "adi-harness-lab — measure where a harness:claude-sdk run spends its wall clock

USAGE
  adi-harness-lab spawn [--agent NAME] [--runs N] [--dir PATH]
        Launch N concurrent runs of a trivial workload; report per-Bash-call latency.

  adi-harness-lab poll  [--agent NAME] [--runs N] [--pollers M] [--dir PATH]
        Same, but M threads also poll peek_run at {POLL_HZ} Hz throughout — the app server's
        read path. Reports the peek latency distribution alongside the tool latency.

  adi-harness-lab parse --log FILE [--repeat N]
        Time a cold parse of an existing run log: what one memo miss costs.

OPTIONS
  --agent NAME   agent to drive (default: zz-lat, backend must be harness:claude-sdk)
  --runs N       concurrent runs (default: 1)
  --pollers M    polling threads (default: 4)
  --dir PATH     working directory for the runs (default: the agent's own)
  --repeat N     parse repetitions (default: 5)"
    );
}

// ---------------------------------------------------------------- options

struct Opts {
    agent: String,
    runs: usize,
    pollers: usize,
    dir: Option<String>,
    log: Option<PathBuf>,
    repeat: usize,
    load: usize,
    io: usize,
    execs: usize,
    bigout: bool,
    out: Option<PathBuf>,
    interval: u64,
    factor: f64,
    minutes: u64,
    no_sample: bool,
    root: Option<PathBuf>,
    replay: bool,
}

/// CPU hogs held for the length of a probe, so a run can be measured against a machine that is
/// actually busy — the one condition the production pathology always appeared under and that an
/// otherwise-idle laptop never reproduces. Dropping the guard kills every hog.
struct LoadGuard(Vec<std::process::Child>);

impl LoadGuard {
    /// `cpu` holds cores down; `io` hammers the filesystem — the kind of pressure `npm ci` and a
    /// production build actually apply, and which a pure busy-loop never reproduces.
    fn spawn_kinds(cpu: usize, io: usize) -> Self {
        let mut kids = Vec::new();
        let tmp = std::env::temp_dir().join("adi-harness-lab-io");
        let _ = std::fs::create_dir_all(&tmp);
        for _ in 0..cpu {
            kids.extend(hog("while :; do :; done"));
        }
        for i in 0..io {
            // Write, sync and delete a 64 MB file in a loop: sustained write pressure plus the
            // metadata churn a package install produces.
            let f = tmp.join(format!("hog-{i}"));
            kids.extend(hog(&format!(
                "while :; do dd if=/dev/zero of={} bs=1m count=64 conv=fsync 2>/dev/null; \
                 rm -f {}; done",
                f.display(),
                f.display()
            )));
        }
        if !kids.is_empty() {
            println!("load: {cpu} CPU hog(s) + {io} IO hog(s) for the length of this probe");
        }
        Self(kids)
    }

    fn spawn(n: usize) -> Self {
        Self::spawn_kinds(n, 0)
    }
}

fn hog(script: &str) -> Option<std::process::Child> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
        if !self.0.is_empty() {
            println!("load: hogs stopped");
        }
    }
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut o = Self {
            agent: "zz-lat".into(),
            runs: 1,
            pollers: 4,
            dir: None,
            log: None,
            repeat: 5,
            load: 0,
            io: 0,
            execs: 0,
            bigout: false,
            out: None,
            interval: 5,
            factor: 1.5,
            minutes: 0,
            no_sample: false,
            root: None,
            replay: false,
        };
        let mut it = args.iter().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--agent" => o.agent = it.next().cloned().unwrap_or(o.agent),
                "--runs" => o.runs = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.runs),
                "--pollers" => {
                    o.pollers = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.pollers);
                }
                "--dir" => o.dir = it.next().cloned(),
                "--log" => o.log = it.next().map(PathBuf::from),
                "--repeat" => o.repeat = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.repeat),
                "--load" => o.load = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.load),
                "--io" => o.io = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.io),
                "--exec" => o.execs = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.execs),
                "--bigout" => o.bigout = true,
                "--out" => o.out = it.next().map(PathBuf::from),
                "--interval" => {
                    o.interval = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.interval);
                }
                "--factor" => o.factor = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.factor),
                "--minutes" => {
                    o.minutes = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.minutes)
                }
                "--no-sample" => o.no_sample = true,
                "--root" => o.root = it.next().map(PathBuf::from),
                "--replay" => o.replay = true,
                _ => {}
            }
        }
        o
    }
}

fn prompt_for(cmds: &[&str]) -> String {
    let numbered: Vec<String> = cmds
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {c}", i + 1))
        .collect();
    format!(
        "Run these {} shell commands, ONE Bash tool call each, in this exact order. Never \
         combine two commands into one call. Do not explain anything between the calls.\n\n{}\n\n\
         Then reply with exactly: DONE",
        cmds.len(),
        numbered.join("\n")
    )
}

fn prompt() -> String {
    prompt_for(TRIVIAL)
}

// ---------------------------------------------------------------- probes

/// Launch `runs` concurrent runs, wait for every one to drop its pid, then read the logs back.
fn cmd_spawn(o: &Opts) {
    let agents = Agents::open();
    let Some(agent) = resolve(&agents, &o.agent) else {
        return;
    };

    println!("spawn: {} concurrent run(s) of `{}`", o.runs, o.agent);
    let _load = LoadGuard::spawn_kinds(o.load, o.io);
    let _execs = ExecGuard::spawn(o.execs);
    let started = Instant::now();
    let mut launched = Vec::new();
    for i in 0..o.runs {
        let p = if o.bigout {
            prompt_for(BIGOUT)
        } else {
            prompt()
        };
        match agents.force_run_in(&o.agent, &p, o.dir.as_deref()) {
            Ok(launch) => {
                let id = run_id_of(&launch);
                println!("  [{i}] run {id}");
                launched.push(id);
            }
            Err(e) => eprintln!("  [{i}] launch failed: {e}"),
        }
    }
    if launched.is_empty() {
        return;
    }

    wait_for_idle(&agents, &agent, &launched);
    let wall = started.elapsed();
    println!("\nall runs finished in {:.1}s\n", wall.as_secs_f64());
    report_runs(&agents, &agent, &launched);
}

/// The read path under load: poll `peek_run` at {POLL_HZ} Hz per thread while the runs stream.
fn cmd_poll(o: &Opts) {
    let agents = Arc::new(Agents::open());
    let Some(agent) = resolve(&agents, &o.agent) else {
        return;
    };

    println!(
        "poll: {} run(s), {} poller thread(s) at {POLL_HZ} Hz",
        o.runs, o.pollers
    );
    let mut launched = Vec::new();
    for _ in 0..o.runs {
        if let Ok(launch) = agents.force_run_in(&o.agent, &prompt(), o.dir.as_deref()) {
            launched.push(run_id_of(&launch));
        }
    }
    if launched.is_empty() {
        eprintln!("nothing launched");
        return;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let peeks = Arc::new(Mutex::new(Vec::<u64>::new()));
    let polls = Arc::new(AtomicU64::new(0));
    let ids = Arc::new(launched.clone());

    let mut threads = Vec::new();
    for _ in 0..o.pollers {
        let (agents, agent, stop, peeks, polls, ids) = (
            Arc::clone(&agents),
            agent.clone(),
            Arc::clone(&stop),
            Arc::clone(&peeks),
            Arc::clone(&polls),
            Arc::clone(&ids),
        );
        threads.push(std::thread::spawn(move || {
            let interval = Duration::from_secs_f64(1.0 / POLL_HZ);
            while !stop.load(Ordering::Relaxed) {
                for id in ids.iter() {
                    let t = Instant::now();
                    let _ = agents.peek_run(&agent, id);
                    let micros = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
                    peeks.lock().map(|mut v| v.push(micros)).ok();
                    polls.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(interval);
            }
        }));
    }

    let started = Instant::now();
    wait_for_idle(&agents, &agent, &launched);
    let wall = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        let _ = t.join();
    }

    println!("\nall runs finished in {:.1}s", wall.as_secs_f64());
    let mut v = peeks.lock().map(|g| g.clone()).unwrap_or_default();
    v.sort_unstable();
    if v.is_empty() {
        println!("no peeks recorded");
    } else {
        println!(
            "\npeek_run: n={} median={:.2}ms p90={:.2}ms p99={:.2}ms max={:.2}ms",
            v.len(),
            ms(v[v.len() / 2]),
            ms(v[v.len() * 9 / 10]),
            ms(v[v.len() * 99 / 100]),
            ms(*v.last().unwrap_or(&0)),
        );
        let slow = v.iter().filter(|&&x| x > 100_000).count();
        println!("          peeks over 100ms: {slow}");
    }
    println!();
    report_runs(&agents, &agent, &launched);
}

/// Raw process-spawn latency — no model, no API, no tokens.
///
/// A harness run that reports `echo alive; pwd` taking 106 seconds is not waiting on `echo`; it is
/// waiting on `fork`/`exec` of the shell. This times exactly that, so spawn cost can be measured
/// against different kinds of machine pressure without paying for an agent run each time.
fn cmd_probe(o: &Opts) {
    let _load = LoadGuard::spawn_kinds(o.load, o.io);
    let exec_load = ExecGuard::spawn(o.execs);
    // Let the load settle so the first samples are not measured against a still-ramping machine.
    std::thread::sleep(Duration::from_secs(3));

    let n = o.repeat.max(1);
    println!("probe: {n} shell spawns");
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg("echo hi")
            .output()
            .is_ok();
        if ok {
            times.push(t.elapsed().as_secs_f64());
        }
    }
    drop(exec_load);
    if times.is_empty() {
        println!("  every spawn failed");
        return;
    }
    times.sort_by(f64::total_cmp);
    let sum: f64 = times.iter().sum();
    println!(
        "  n={} median={:.1}ms mean={:.1}ms p90={:.1}ms p99={:.1}ms max={:.1}ms",
        times.len(),
        times[times.len() / 2] * 1e3,
        sum / times.len() as f64 * 1e3,
        times[times.len() * 9 / 10] * 1e3,
        times[times.len() * 99 / 100] * 1e3,
        times.last().copied().unwrap_or(0.0) * 1e3,
    );
    for thr in [0.1_f64, 1.0, 5.0, 30.0] {
        let over = times.iter().filter(|&&t| t > thr).count();
        if over > 0 {
            println!("  spawns over {thr}s: {over}");
        }
    }
}

/// Spawn latency as memory fills up — the last axis, and the one a busy-loop cannot reach.
///
/// A `fork` has to find pages. When the machine is compressing and swapping to serve them, that is
/// where a shell that "is alive but very slow to spawn" comes from. This walks memory up in steps,
/// timing shell spawns at each level, and **stops the moment the kernel leaves normal pressure** —
/// well short of jetsam, which must never get a chance to touch the services on this machine.
fn cmd_mem(o: &Opts) {
    const STEP_GB: usize = 4;
    let cap_gb = if o.repeat > 5 { o.repeat } else { 48 };
    println!(
        "mem: stepping {STEP_GB} GB at a time up to {cap_gb} GB, stopping at first non-normal pressure"
    );
    println!(
        "  {:>7} {:>9} {:>10} {:>10} {:>10}",
        "held_GB", "pressure", "median_ms", "p90_ms", "max_ms"
    );

    let mut held: Vec<Vec<u8>> = Vec::new();
    let base = spawn_stats(40);
    println!(
        "  {:>7} {:>9} {:>10.1} {:>10.1} {:>10.1}",
        0,
        pressure_level(),
        base.0,
        base.1,
        base.2
    );

    let mut gb = 0;
    while gb < cap_gb {
        // Touch every page so the allocation is resident, not just reserved.
        let mut chunk = vec![0u8; STEP_GB << 30];
        for i in (0..chunk.len()).step_by(16 * 1024) {
            chunk[i] = 1;
        }
        held.push(chunk);
        gb += STEP_GB;

        let level = pressure_level();
        let (med, p90, max) = spawn_stats(40);
        println!("  {gb:>7} {level:>9} {med:>10.1} {p90:>10.1} {max:>10.1}");
        if level > 1 {
            println!("  pressure left normal — stopping here and releasing");
            break;
        }
    }
    drop(held);
    println!("  released; pressure back to {}", pressure_level());
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical.
fn pressure_level() -> i32 {
    std::process::Command::new("sysctl")
        .args(["-n", "kern.memorystatus_vm_pressure_level"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(-1)
}

/// (median, p90, max) milliseconds over `n` shell spawns.
fn spawn_stats(n: usize) -> (f64, f64, f64) {
    let mut t: Vec<f64> = (0..n)
        .filter_map(|_| {
            let s = Instant::now();
            std::process::Command::new("sh")
                .arg("-c")
                .arg("echo hi")
                .output()
                .ok()
                .map(|_| s.elapsed().as_secs_f64() * 1e3)
        })
        .collect();
    if t.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    t.sort_by(f64::total_cmp);
    (t[t.len() / 2], t[t.len() * 9 / 10], t[t.len() - 1])
}

/// Load that forces macOS to validate a *fresh* Mach-O on every exec — what a package install
/// does when it unpacks thousands of new binaries, and the one kind of pressure a CPU busy-loop
/// cannot imitate. Each iteration copies a signed system binary to a new inode and runs it, so
/// `amfid`/`syspolicyd` cannot serve the check from cache.
struct ExecGuard(Vec<std::process::Child>, PathBuf);

impl ExecGuard {
    fn spawn(n: usize) -> Self {
        let tmp = std::env::temp_dir().join("adi-harness-lab-exec");
        let _ = std::fs::create_dir_all(&tmp);
        let mut kids = Vec::new();
        for i in 0..n {
            let d = tmp.join(format!("w{i}"));
            let _ = std::fs::create_dir_all(&d);
            kids.extend(hog(&format!(
                "i=0; while :; do f={}/b$i; cp /bin/echo $f 2>/dev/null; \
                 $f x >/dev/null 2>&1; rm -f $f; i=$(((i+1)%256)); done",
                d.display()
            )));
        }
        if n > 0 {
            println!("load: {n} fresh-binary exec hog(s) — forcing signature validation per exec");
        }
        Self(kids, tmp)
    }
}

impl Drop for ExecGuard {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.1);
        if !self.0.is_empty() {
            println!("load: exec hogs stopped");
        }
    }
}

/// The part that is unique to `harness:claude-sdk`: a conversation carried across turns.
///
/// Turn 1 is a fresh `claude --print --session-id <uuid>`; every later turn is a `reply`, which
/// spawns `claude --resume <uuid>` against a session that keeps growing. This walks a conversation
/// `--turns` deep and reports, per turn, the three costs that could each grow with depth: the time
/// the child spends before it even issues its API request (session load / resume), the model's own
/// time, and the Bash floor. If the backend gets slower the longer a conversation runs, it shows
/// up here as a rising `to_request` or a rising floor.
fn cmd_conv(o: &Opts) {
    let agents = Agents::open();
    let Some(agent) = resolve(&agents, &o.agent) else {
        return;
    };
    let _load = LoadGuard::spawn(o.load);

    println!("conv: walking `{}` {} turns deep", o.agent, o.repeat);
    let Ok(launch) = agents.force_run_in(&o.agent, &prompt(), o.dir.as_deref()) else {
        eprintln!("launch failed");
        return;
    };
    let conv = run_id_of(&launch);
    let dir = sessions_dir(&agents, &agent);
    println!("  conversation {conv}\n");

    println!(
        "  {:>4} {:>10} {:>10} {:>9} {:>9} {:>7} {:>9} {:>10} {:>9}",
        "turn", "reply→done", "to_request", "ttft", "api", "bash", "floor", "log_kb", "sess_kb"
    );

    for turn in 1..=o.repeat {
        // Turn 1 is already in flight from the launch above; later turns need a reply.
        let t0 = Instant::now();
        if turn > 1 {
            // The reply guard refuses a second turn while one is in flight, so settle first.
            wait_for_idle(&agents, &agent, std::slice::from_ref(&conv));
            let _ = agents.transcript(&agent, &conv); // drives `settle`, as a UI read would
            if let Err(e) = agents.reply(&o.agent, &conv, &prompt()) {
                eprintln!("  turn {turn}: reply refused: {e}");
                break;
            }
        }
        wait_for_idle(&agents, &agent, std::slice::from_ref(&conv));
        let wall = t0.elapsed();

        let log = dir.join(format!("{conv}.log"));
        let calls = tool_calls(&log);
        let result = result_event(&log);
        let num = |k: &str| {
            result
                .as_ref()
                .and_then(|r| r.get(k))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
        };
        let mut floor: Vec<f64> = calls.iter().map(|c| c.secs).filter(|&s| s < 1.0).collect();
        floor.sort_by(f64::total_cmp);
        let median = floor.get(floor.len() / 2).copied().unwrap_or(0.0);

        let log_kb = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) as f64 / 1024.0;
        let sess_kb = session_bytes(&agents, &agent, &conv) as f64 / 1024.0;

        println!(
            "  {:>4} {:>9.1}s {:>9.0}ms {:>8.0}ms {:>8.1}s {:>7} {:>8.3}s {:>10.0} {:>9.0}",
            turn,
            wall.as_secs_f64(),
            num("time_to_request_ms"),
            num("ttft_ms"),
            num("duration_api_ms") / 1e3,
            calls.len(),
            median,
            log_kb,
            sess_kb,
        );
    }

    let turns = agents.transcript(&agent, &conv);
    println!("\n  transcript settled to {} turns", turns.len());
}

/// Bytes of the `claude` session file this conversation resumes from — the thing `--resume` has to
/// load before it can do anything, and the most obvious candidate for a cost that grows with depth.
fn session_bytes(agents: &Agents, agent: &StoredAgent, conv: &str) -> u64 {
    let meta = sessions_dir(agents, agent).join(format!("{conv}.json"));
    let Ok(text) = std::fs::read_to_string(&meta) else {
        return 0;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    let (Some(sid), Some(cwd)) = (
        v.get("session_id").and_then(serde_json::Value::as_str),
        v.get("working_dir").and_then(serde_json::Value::as_str),
    ) else {
        return 0;
    };
    // `claude` keeps sessions under ~/.claude/projects/<cwd with separators flattened>/<uuid>.jsonl
    let Some(home) = std::env::var_os("HOME") else {
        return 0;
    };
    let flat = cwd.replace(['/', '.', '_'], "-");
    let path = PathBuf::from(home)
        .join(".claude/projects")
        .join(flat)
        .join(format!("{sid}.jsonl"));
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// What one memo miss costs, measured on a log that already exists.
fn cmd_parse(o: &Opts) {
    let Some(log) = o.log.as_ref() else {
        eprintln!("--log FILE is required");
        return;
    };
    let bytes = std::fs::metadata(log).map(|m| m.len()).unwrap_or(0);
    println!("parse: {} ({:.1} MB)", log.display(), bytes as f64 / 1e6);

    // Copying to a fresh path each round defeats the memo: every read is a cold parse, which is
    // exactly what the app server does to a log that is still growing.
    let tmp = std::env::temp_dir().join("adi-harness-lab-parse");
    let _ = std::fs::create_dir_all(&tmp);
    let mut times = Vec::new();
    for i in 0..o.repeat {
        let dest = tmp.join(format!("copy-{i}.log"));
        if std::fs::copy(log, &dest).is_err() {
            continue;
        }
        let t = Instant::now();
        let steps = count_steps(&dest);
        let el = t.elapsed();
        times.push(el.as_secs_f64());
        println!(
            "  round {i}: {:.1}ms  ({steps} steps)",
            el.as_secs_f64() * 1e3
        );
    }
    if !times.is_empty() {
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        println!("\n  mean {:.1}ms per cold parse", mean * 1e3);
        println!(
            "  at {POLL_HZ} Hz that is {:.1}% of one core, per open conversation",
            mean * POLL_HZ * 100.0
        );
    }
}

// ---------------------------------------------------------------- reporting

/// Per-Bash-call latency for each run, read out of the stream-json log the child wrote.
fn report_runs(agents: &Agents, agent: &StoredAgent, ids: &[String]) {
    let dir = sessions_dir(agents, agent);
    let mut all = Vec::new();
    for id in ids {
        let log = dir.join(format!("{id}.log"));
        let calls = tool_calls(&log);
        let result = result_event(&log);
        println!("run {id}");
        if let Some(r) = &result {
            println!(
                "  wall={:.1}s api={:.1}s turns={} err={}",
                r.get("duration_ms")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0)
                    / 1e3,
                r.get("duration_api_ms")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0)
                    / 1e3,
                r.get("num_turns")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                r.get("is_error")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
        }
        for c in &calls {
            println!("    {:7.3}s  {}", c.secs, truncate(&c.cmd, 58));
        }
        all.extend(calls.iter().map(|c| c.secs));
        // A pid left behind means the reaper never ran — the launcher exited before the child did.
        if dir.join(format!("{id}.pid")).exists() {
            println!("    !! pid file left behind (reaper never fired)");
        }
    }
    if all.is_empty() {
        return;
    }
    all.sort_by(f64::total_cmp);
    println!(
        "\nBash calls across all runs: n={} median={:.3}s p90={:.3}s max={:.3}s",
        all.len(),
        all[all.len() / 2],
        all[all.len() * 9 / 10],
        all.last().copied().unwrap_or(0.0),
    );
    // The first call of every session pays shell warm-up; the rest is the steady-state floor.
    let steady: Vec<f64> = all.iter().copied().filter(|&s| s < 1.0).collect();
    if !steady.is_empty() {
        let mean = steady.iter().sum::<f64>() / steady.len() as f64;
        println!(
            "steady-state floor (calls under 1s): n={} mean={mean:.3}s",
            steady.len()
        );
    }
}

struct Call {
    secs: f64,
    cmd: String,
}

/// Pair each `tool_use` with its `tool_result` by id, and take the gap between their timestamps.
fn tool_calls(log: &Path) -> Vec<Call> {
    let Ok(text) = std::fs::read_to_string(log) else {
        return Vec::new();
    };
    let mut pending: BTreeMap<String, (f64, String)> = BTreeMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let ts = v
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_ts);
        let content = v
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array);
        match v.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                let (Some(ts), Some(blocks)) = (ts, content) else {
                    continue;
                };
                for b in blocks {
                    if b.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                        let id = b
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        let cmd = b
                            .pointer("/input/command")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        pending.insert(id.to_string(), (ts, cmd.to_string()));
                    }
                }
            }
            Some("user") => {
                let (Some(ts), Some(blocks)) = (ts, content) else {
                    continue;
                };
                for b in blocks {
                    if b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result") {
                        let id = b
                            .get("tool_use_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        if let Some((start, cmd)) = pending.remove(id) {
                            out.push(Call {
                                secs: ts - start,
                                cmd,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn result_event(log: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("type").and_then(serde_json::Value::as_str) == Some("result"))
}

/// Deserialize every event and count the progress-bearing blocks — the work a memo miss redoes.
fn count_steps(log: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(log) else {
        return 0;
    };
    let mut steps = 0;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(blocks) = v
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array)
        {
            steps += blocks.len();
        }
    }
    steps
}

/// `2026-07-29T08:03:22.206Z` as unix seconds. Hand-rolled: the field is fixed-width, and the lab
/// has no business pulling a date crate into the workspace for one format.
fn parse_ts(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 24 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, sec) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    let millis = n(20, 23)?;
    // Days since the unix epoch, via the civil-from-days algorithm (Howard Hinnant's).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3_600 + mi * 60 + sec) as f64 + millis as f64 / 1e3)
}

// ---------------------------------------------------------------- helpers

fn resolve(agents: &Agents, name: &str) -> Option<StoredAgent> {
    match agents.get(name) {
        Ok(Some(a)) => {
            if a.manifest.backend != Backend::HarnessClaudeSdk {
                eprintln!(
                    "agent `{name}` is `{:?}`; this lab only drives harness:claude-sdk today",
                    a.manifest.backend
                );
                return None;
            }
            Some(a)
        }
        Ok(None) => {
            eprintln!("no agent `{name}` — create one with:");
            eprintln!(
                "  adi-mono agents save {name} --backend harness:claude-sdk --model haiku --command-scope Bash"
            );
            None
        }
        Err(e) => {
            eprintln!("cannot read agent `{name}`: {e}");
            None
        }
    }
}

fn run_id_of(launch: &adi_agents::Launch) -> String {
    match launch {
        adi_agents::Launch::Process { run_id, .. } => run_id.clone(),
        other => format!("{other:?}"),
    }
}

/// `<store>/sessions/harness/<agent>` — where a harness run keeps its `.log`/`.pid`/`.jsonl`.
/// [`Agents::dir`] points at the *agents* module (`<store>/agents`), so the store root is its
/// parent; the sessions module sits beside it.
fn sessions_dir(agents: &Agents, agent: &StoredAgent) -> PathBuf {
    let agents_dir = agents.dir();
    let root = agents_dir.parent().unwrap_or(&agents_dir);
    root.join("sessions").join("harness").join(&agent.name)
}

/// Block until every launched run has dropped its pid file, printing a dot per second so a long
/// wait does not look like a hang.
fn wait_for_idle(agents: &Agents, agent: &StoredAgent, ids: &[String]) {
    let dir = sessions_dir(agents, agent);
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let live = ids
            .iter()
            .filter(|id| pid_alive(&dir.join(format!("{id}.pid"))))
            .count();
        if live == 0 || Instant::now() > deadline {
            return;
        }
        print!(".");
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// A pid file only counts as "running" when the process it names is actually alive — a stale file
/// left by an exited launcher must not read as a live turn.
fn pid_alive(pid_file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    // SAFETY: `kill(pid, 0)` performs the permission/existence check without sending a signal.
    unsafe { libc_kill(pid, 0) == 0 }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

fn ms(micros: u64) -> f64 {
    micros as f64 / 1e3
}

fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= n {
        return one_line;
    }
    one_line.chars().take(n).collect::<String>() + "…"
}

/// Stand over the live harness until a real overshoot happens, and photograph it.
fn cmd_watch(o: &Opts) {
    let root = o.root.clone().unwrap_or_else(|| {
        let agents = Agents::open();
        let agents_dir = agents.dir();
        agents_dir.parent().unwrap_or(&agents_dir).to_path_buf()
    });
    let out = o
        .out
        .clone()
        .unwrap_or_else(|| root.join("harness-lab-watch"));
    watch::run(&watch::Watch {
        root,
        out,
        interval: Duration::from_secs(o.interval.max(1)),
        factor: o.factor,
        duration: (o.minutes > 0).then(|| Duration::from_secs(o.minutes * 60)),
        sample_pid: !o.no_sample,
        replay: o.replay,
    });
}
