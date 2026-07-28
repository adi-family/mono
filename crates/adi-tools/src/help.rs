//! A tool's own help text — what an agent is *told* about the CLIs sitting on its PATH.
//!
//! A tool documents itself by answering `llm help` (help written for a model: what it's for, how to
//! call it, what it prints) or, failing that, the plain `help` / `--help` an ordinary CLI already
//! prints. Whichever answers first is what an agent's system prompt carries, so a tool becomes
//! usable by writing its help rather than by editing prompts.
//!
//! Capture is **best-effort and bounded**. A tool that hangs, crashes, or prints nothing costs the
//! launch a timeout and is left out; nothing here can fail a run. Because the result is folded into
//! the prompt on *every* turn, it is cached per tool under `tools/.help/<id>` and re-captured only
//! when the tool's script changes or the entry ages out.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::tool::Tool;

/// The argument lists tried, in order: help written for a model first, then the two ways an
/// ordinary CLI prints its own. The first that exits cleanly with something to say wins.
///
/// Both plain forms are needed, because which one works is a property of the command's shape: a CLI
/// with subcommands answers `help` (clap generates it), while a leaf command has no such subcommand
/// and errors — but answers `--help`. `adi-tasks` is the first; `adi-status` is the second.
const HELP_ARGS: [&[&str]; 3] = [&["llm", "help"], &["help"], &["--help"]];

/// How long one tool gets to answer before it's killed and skipped. Help is a print-and-exit
/// command; a tool taking longer than this is malfunctioning, and a launch must not wait on it.
const PER_TOOL_TIMEOUT: Duration = Duration::from_secs(3);

/// The whole capture's budget across every tool. Reached, the remaining tools are left out of this
/// launch (their cache entries, if any, are still used — only fresh captures are skipped).
pub(crate) const TOTAL_BUDGET: Duration = Duration::from_secs(10);

/// How long a captured help stays fresh. A tool's script mtime already invalidates its entry; this
/// bounds the other direction — a system tool whose script is a stable one-line shim over
/// `adi-mono <subcommand>`, whose *help* changes when adi-mono is upgraded underneath it.
const TTL: Duration = Duration::from_secs(60 * 60);

/// The most of one tool's help that reaches a prompt. Long enough for a real command listing, short
/// enough that a tool with a book for a manual can't crowd out the agent's actual instructions.
const MAX_HELP_CHARS: usize = 3_000;

/// The cache directory, `tools/.help`. Dot-prefixed so [`Tools::list`](crate::Tools::list) skips it,
/// like `.bin` and `.agent-bin` beside it.
const HELP_DIR: &str = ".help";

/// The first line of a cache entry — a format tag, then what the entry is keyed on. A mismatch (or
/// an unreadable line) is a miss, so the format can change without a migration.
const CACHE_TAG: &str = "adi-tool-help 1";

/// One tool as an agent is told about it: the name it runs the tool by, the registry's one-line
/// description, and the tool's own help — `None` when it had none to give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolHelp {
    /// The `.bin` name the agent invokes — the same slug the shim is written under.
    pub name: String,
    /// The registry's one-line description, when the manifest carries one.
    pub description: Option<String>,
    /// What `llm help` (or `help`) printed, trimmed and capped; `None` when neither answered.
    pub help: Option<String>,
}

/// One tool's help: the cache entry when it's still valid, otherwise a fresh capture (written back
/// to the cache). Returns `None` when the tool has no help to give — or when `deadline` has passed
/// and it isn't already cached, so a slow fleet of tools degrades to "fewer documented" rather than
/// to a slow launch.
pub(crate) fn text(
    tools_dir: &Path,
    tool: &Tool,
    script: &Path,
    mut cmd: impl FnMut(&[&str]) -> Option<Command>,
    deadline: Instant,
) -> Option<String> {
    let entry = cache_path(tools_dir, &tool.id);
    let key = key_of(script);
    if let Some(cached) = read_cache(&entry, key) {
        // An entry recording "this tool answers nothing" is a real answer — it saves the next
        // launch two spawns — so an empty body caches as absent rather than as a miss.
        return (!cached.is_empty()).then_some(cached);
    }
    let mut captured = None;
    // Every convention gets asked, but the budget outranks them: a tool that hangs on the first is
    // cut off there rather than hanging again on each of the rest.
    let mut asked_all = true;
    for args in HELP_ARGS {
        if Instant::now() >= deadline {
            asked_all = false;
            break;
        }
        let Some(command) = cmd(args) else { continue };
        captured = run_once(command, (Instant::now() + PER_TOOL_TIMEOUT).min(deadline));
        if captured.is_some() {
            break;
        }
    }
    match captured {
        Some(text) => {
            write_cache(&entry, key, &text);
            Some(text)
        }
        // Asked every way and got nothing: worth remembering, so the next launch spawns nothing.
        None if asked_all => {
            write_cache(&entry, key, "");
            None
        }
        // Out of budget mid-ask — the tool was never given its say, so nothing is concluded about
        // it and the next launch tries again.
        None => None,
    }
}

/// Run one help invocation, cut off at `deadline`. `Some` only for a clean exit with something
/// printed — a tool that fails (`llm help` on a CLI that has no such subcommand exits non-zero) is
/// how the fallback to the next convention is decided.
fn run_once(mut command: Command, deadline: Instant) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let out = child.wait_with_output().ok()?;
                return cap(&String::from_utf8_lossy(&out.stdout));
            }
            // Still running. Nothing reads the pipe until it exits, so a tool that prints more than
            // a pipe buffer before exiting blocks and is killed at the deadline — acceptable for a
            // help text, and the cap below says such output wasn't wanted whole anyway.
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }
}

/// Trim to what a prompt should carry: blank output is nothing at all, and an overlong one is cut
/// on a line boundary with a marker, so the agent can tell it is reading a fragment.
fn cap(output: &str) -> Option<String> {
    let text = output.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= MAX_HELP_CHARS {
        return Some(text.to_string());
    }
    let mut kept: String = text.chars().take(MAX_HELP_CHARS).collect();
    if let Some(last_line) = kept.rfind('\n') {
        kept.truncate(last_line);
    }
    Some(format!("{kept}\n… (help truncated)"))
}

/// What a cache entry is keyed on: the script's modification time and size. A tool whose script is
/// edited (or re-linked to a different file) re-captures on its next launch.
fn key_of(script: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::metadata(script) else {
        return (0, 0);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (mtime, meta.len())
}

fn cache_path(tools_dir: &Path, id: &str) -> PathBuf {
    tools_dir.join(HELP_DIR).join(id)
}

/// The cached help for `key`, or `None` when the entry is missing, malformed, keyed on a different
/// script, or older than [`TTL`]. An entry that exists and matches may legitimately be empty — that
/// is the cached "this tool answers nothing".
fn read_cache(entry: &Path, key: (u64, u64)) -> Option<String> {
    let raw = std::fs::read_to_string(entry).ok()?;
    let (header, body) = raw.split_once('\n')?;
    let mut fields = header.strip_prefix(CACHE_TAG)?.split_whitespace();
    let mtime: u64 = fields.next()?.parse().ok()?;
    let len: u64 = fields.next()?.parse().ok()?;
    let captured_at: u64 = fields.next()?.parse().ok()?;
    if (mtime, len) != key || now_unix().saturating_sub(captured_at) > TTL.as_secs() {
        return None;
    }
    Some(body.to_string())
}

/// Write the entry back, best-effort: a cache that can't be written just means the next launch
/// captures again.
fn write_cache(entry: &Path, key: (u64, u64), text: &str) {
    let (mtime, len) = key;
    let body = format!("{CACHE_TAG} {mtime} {len} {}\n{text}", now_unix());
    if let Some(dir) = entry.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(entry, body);
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Manifest;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-tools-help-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn tool() -> Tool {
        Tool {
            id: "t1".to_string(),
            manifest: Manifest {
                name: "greet".to_string(),
                ..Manifest::default()
            },
        }
    }

    /// A shell command printing `text` for exactly the argument list `want`, failing otherwise —
    /// stands in for a tool that implements one of the two help conventions and not the other.
    fn answering(want: &'static [&'static str], text: &'static str) -> impl FnMut(&[&str]) -> Option<Command> {
        move |args: &[&str]| {
            let mut cmd = Command::new("sh");
            if args == want {
                cmd.args(["-c", &format!("printf '%s' '{text}'")]);
            } else {
                cmd.args(["-c", "exit 1"]);
            }
            Some(cmd)
        }
    }

    #[test]
    fn llm_help_is_preferred_over_plain_help() {
        let dir = scratch("prefers-llm");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        // Both conventions answer; the model-facing one is the one that reaches the prompt.
        let both = |args: &[&str]| {
            let mut cmd = Command::new("sh");
            let text = if args == ["llm", "help"] { "for the model" } else { "for a human" };
            cmd.args(["-c", &format!("printf '%s' '{text}'")]);
            Some(cmd)
        };
        let got = text(&dir, &tool(), &script, both, far_future());
        assert_eq!(got.as_deref(), Some("for the model"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_help_answers_when_llm_help_is_not_implemented() {
        let dir = scratch("falls-back");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let got = text(&dir, &tool(), &script, answering(&["help"], "usage: greet"), far_future());
        assert_eq!(got.as_deref(), Some("usage: greet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A leaf CLI has no `help` subcommand — it errors on one — but prints its usage for `--help`.
    /// Without this third try, every command without subcommands would go undocumented.
    #[test]
    fn a_leaf_cli_is_reached_through_its_help_flag() {
        let dir = scratch("help-flag");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let got = text(
            &dir,
            &tool(),
            &script,
            answering(&["--help"], "Usage: greet [OPTIONS]"),
            far_future(),
        );
        assert_eq!(got.as_deref(), Some("Usage: greet [OPTIONS]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tool_that_answers_neither_is_left_out() {
        let dir = scratch("silent");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let got = text(&dir, &tool(), &script, answering(&["nope"], "x"), far_future());
        assert_eq!(got, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_look_is_served_from_cache_and_spawns_nothing() {
        let dir = scratch("cached");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let first = text(&dir, &tool(), &script, answering(&["help"], "usage: greet"), far_future());
        assert_eq!(first.as_deref(), Some("usage: greet"));

        // The tool now answers something else. The cache is what the prompt gets, so nothing ran.
        let got = text(&dir, &tool(), &script, answering(&["help"], "different"), far_future());
        assert_eq!(got.as_deref(), Some("usage: greet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_the_script_re_captures() {
        let dir = scratch("invalidates");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let _ = text(&dir, &tool(), &script, answering(&["help"], "old help"), far_future());

        // A different size is a different key, so the stale entry is not reused.
        std::fs::write(&script, "echo hi there, at length\n").expect("rewrite");
        let got = text(&dir, &tool(), &script, answering(&["help"], "new help"), far_future());
        assert_eq!(got.as_deref(), Some("new help"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_passed_deadline_skips_an_uncached_tool_but_still_serves_a_cached_one() {
        let dir = scratch("deadline");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let past = Instant::now();
        assert_eq!(
            text(&dir, &tool(), &script, answering(&["help"], "usage"), past),
            None
        );
        // Capture it, then ask again past the deadline: the cached answer needs no spawn.
        let _ = text(&dir, &tool(), &script, answering(&["help"], "usage"), far_future());
        assert_eq!(
            text(&dir, &tool(), &script, answering(&["help"], "usage"), past).as_deref(),
            Some("usage")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tool that never exits is killed at the budget, not waited on — and, having never had its
    /// say, is not remembered as silent: the next launch asks it again.
    #[test]
    fn a_hanging_tool_is_killed_at_the_budget_and_not_cached() {
        let dir = scratch("hangs");
        let script = dir.join("script.sh");
        std::fs::write(&script, "echo hi\n").expect("write");
        let hang = |_: &[&str]| {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 30"]);
            Some(cmd)
        };
        let budget = Duration::from_millis(700);
        let started = Instant::now();
        assert_eq!(text(&dir, &tool(), &script, hang, Instant::now() + budget), None);
        assert!(
            started.elapsed() < budget * 3,
            "waited on the tool: {:?}",
            started.elapsed()
        );
        // Nothing was concluded, so a later launch still finds the tool's real help.
        let got = text(&dir, &tool(), &script, answering(&["help"], "usage"), far_future());
        assert_eq!(got.as_deref(), Some("usage"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_overlong_help_is_cut_on_a_line_boundary() {
        let long = "a line of help\n".repeat(500);
        let capped = cap(&long).expect("some");
        assert!(capped.chars().count() <= MAX_HELP_CHARS + 32, "{}", capped.len());
        assert!(capped.ends_with("… (help truncated)"));
        assert!(cap("   ").is_none());
    }

    fn far_future() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }
}
