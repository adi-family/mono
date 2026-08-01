//! The tools an `harness:adi` turn can call, and the code that runs them.
//!
//! These are the file and shell tools every coding agent has — `Read`, `Write`, `Edit`, `Bash`,
//! `Glob`, `Grep` — declared once here in a provider-neutral shape and translated into each
//! provider's own function-calling dialect by [`super::adi_loop`]. The names match the ones the
//! Claude backends use, because an agent's prompt (and the person writing it) shouldn't have to
//! learn a second vocabulary to move between backends.
//!
//! **There is no path jail, deliberately.** `Bash` runs a shell, so confining `Read` to the run's
//! working directory would stop nothing and only mislead whoever read the code. What the tools do
//! instead is resolve every relative path against the run's own directory — the one `workspace`
//! picked and the child was spawned into — so an agent that says `src/lib.rs` means the file its
//! run is about, and an agent that means something else says so in full.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// A tool as the model sees it: a name, a sentence about when to reach for it, and the JSON Schema
/// of its arguments. Providers disagree only about where these three go on the wire.
pub(super) struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The JSON Schema `object` describing this tool's arguments.
    pub schema: fn() -> Value,
}

/// Everything a turn may call. Order is the order they're advertised, which is the order a model
/// tends to consider them in: look before you write, and shell out only when nothing else fits.
pub(super) const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "Read",
        description: "Read a file from the filesystem. Prefer this over `cat` in Bash: it returns \
                      the text directly and says so when a file was too large to return whole.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to read. Relative paths resolve against the working directory." },
                    "offset": { "type": "integer", "description": "First line to return (1-based). Omit to start at the beginning." },
                    "limit": { "type": "integer", "description": "How many lines to return. Omit for as much as fits." },
                },
                "required": ["path"],
            })
        },
    },
    ToolSpec {
        name: "Write",
        description: "Write a file, creating it and any missing parent directories, and replacing \
                      it whole if it already exists. To change part of a file, use Edit instead.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to write." },
                    "content": { "type": "string", "description": "The file's complete new contents." },
                },
                "required": ["path", "content"],
            })
        },
    },
    ToolSpec {
        name: "Edit",
        description: "Replace an exact string in a file. `old_string` must appear exactly once \
                      unless `replace_all` is true — include enough surrounding text to make it \
                      unique. Read the file first; an edit against text you have not seen fails.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to edit." },
                    "old_string": { "type": "string", "description": "Text to replace, copied exactly from the file." },
                    "new_string": { "type": "string", "description": "Text to put in its place." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of requiring exactly one." },
                },
                "required": ["path", "old_string", "new_string"],
            })
        },
    },
    ToolSpec {
        name: "Bash",
        description: "Run a shell command in the working directory and return its output (stdout \
                      and stderr together, with the exit status when it is not zero).",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to run." },
                    "timeout_ms": { "type": "integer", "description": "Give up after this long. Defaults to 120000." },
                },
                "required": ["command"],
            })
        },
    },
    ToolSpec {
        name: "Glob",
        description: "List files matching a glob pattern (`*`, `?`, and `**` for any depth), \
                      newest first. Use it to find files by name when you don't know where they are.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob to match against paths, e.g. `**/*.rs`." },
                    "path": { "type": "string", "description": "Directory to search. Defaults to the working directory." },
                },
                "required": ["pattern"],
            })
        },
    },
    ToolSpec {
        name: "Grep",
        description: "Search file contents with a regular expression and return matching lines as \
                      `path:line: text`. Use it to find code by what it says rather than its name.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Rust/PCRE-style regular expression." },
                    "path": { "type": "string", "description": "File or directory to search. Defaults to the working directory." },
                    "glob": { "type": "string", "description": "Only search paths matching this glob, e.g. `**/*.toml`." },
                    "case_insensitive": { "type": "boolean", "description": "Ignore case while matching." },
                },
                "required": ["pattern"],
            })
        },
    },
];

/// How much of a tool's output goes back to the model. A turn replays its whole transcript on every
/// round, so an unbounded `Read` of a large file is paid for again on each one — the cap is what
/// keeps one careless call from crowding out the rest of the conversation.
const MAX_OUTPUT: usize = 32_000;
/// Directories never worth walking: build output and version-control internals, which are large,
/// uninteresting, and would drown a `Glob` in noise.
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", "dist", ".build", "__pycache__"];
/// How long a `Bash` command may run before it is killed, when the call names no timeout.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Run one tool call in `cwd`.
///
/// The error half of the result is not a failure of the loop — it is the tool's *answer*, handed
/// back to the model as a failed result so it can correct itself and try again, which is why every
/// message here is written to be read by the model rather than by a log reader.
pub(super) fn run(name: &str, input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    match name {
        "Read" => read(input, cwd),
        "Write" => write(input, cwd),
        "Edit" => edit(input, cwd),
        "Bash" => bash(input, cwd),
        "Glob" => glob(input, cwd),
        "Grep" => grep(input, cwd),
        other => Err(format!(
            "no tool named {other} — the tools you have are: {}",
            TOOLS
                .iter()
                .map(|t| t.name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ---- the tools ---------------------------------------------------------------------

fn read(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;

    // Saturating rather than casting: a model that asks for line 2^40 of a file gets the end of
    // it, not a number that wrapped into something else entirely.
    let offset = usize::try_from(arg_u64(input, "offset").unwrap_or(1).max(1)).unwrap_or(usize::MAX);
    let limit = usize::try_from(arg_u64(input, "limit").unwrap_or(u64::MAX)).unwrap_or(usize::MAX);
    let lines: Vec<&str> = text.lines().skip(offset - 1).take(limit).collect();
    if lines.is_empty() {
        return Ok(format!(
            "{} has no lines at offset {offset} (the file has {}).",
            path.display(),
            text.lines().count()
        ));
    }
    Ok(truncate(&lines.join("\n")))
}

fn write(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let content = arg_str(input, "content")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(format!(
        "wrote {} ({} bytes)",
        path.display(),
        content.len()
    ))
}

fn edit(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let path = resolve(arg_str(input, "path")?, cwd);
    let old = arg_str(input, "old_string")?;
    let new = arg_str(input, "new_string")?;
    let all = input
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if old == new {
        return Err("old_string and new_string are identical — nothing to change".to_string());
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let hits = text.matches(old).count();
    let edited = match (hits, all) {
        (0, _) => {
            return Err(format!(
                "old_string is not in {} — read the file and copy the text exactly, including \
                 whitespace",
                path.display()
            ));
        }
        (n, false) if n > 1 => {
            return Err(format!(
                "old_string appears {n} times in {} — add surrounding text until it is unique, or \
                 pass replace_all",
                path.display()
            ));
        }
        (_, true) => text.replace(old, new),
        (_, false) => text.replacen(old, new, 1),
    };
    std::fs::write(&path, &edited)
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(format!(
        "edited {} ({} replacement{})",
        path.display(),
        if all { hits } else { 1 },
        if all && hits != 1 { "s" } else { "" }
    ))
}

fn bash(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let command = arg_str(input, "command")?;
    let timeout = arg_u64(input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);

    // The shell is the platform's own: `sh -c` where there is one, `cmd /C` on Windows, matching
    // what the process backends hand their CLIs.
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(cwd);

    let output = wait_with_timeout(cmd, timeout)?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if !output.status.success() {
        // A non-zero exit is information, not a loop failure: the model usually wants to read the
        // output and try something else, so it comes back as a result with the status attached.
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Ok(truncate(&format!("(exit {code})\n{text}")));
    }
    Ok(truncate(if text.trim().is_empty() {
        "(no output)"
    } else {
        &text
    }))
}

fn glob(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let pattern = arg_str(input, "pattern")?;
    let root = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(p, cwd),
        _ => cwd.to_path_buf(),
    };

    let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    walk(&root, &mut |path| {
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        if glob_match(pattern, &rel.to_string_lossy()) {
            let at = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            hits.push((at, path.to_path_buf()));
        }
    });
    if hits.is_empty() {
        return Ok(format!("no files match {pattern} under {}", root.display()));
    }
    // Newest first: when a pattern matches broadly, what changed recently is what is being worked on.
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    let listed: Vec<String> = hits
        .iter()
        .map(|(_, p)| p.display().to_string())
        .collect();
    Ok(truncate(&listed.join("\n")))
}

fn grep(input: &Value, cwd: &Path) -> std::result::Result<String, String> {
    let pattern = arg_str(input, "pattern")?;
    let insensitive = input
        .get("case_insensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(insensitive)
        .build()
        .map_err(|e| format!("{pattern} is not a valid regular expression: {e}"))?;
    let only = input
        .get("glob")
        .and_then(Value::as_str)
        .filter(|g| !g.trim().is_empty());
    let root = match input.get("path").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => resolve(p, cwd),
        _ => cwd.to_path_buf(),
    };

    let mut out = String::new();
    let mut search = |path: &Path, rel: &str| {
        if let Some(g) = only
            && !glob_match(g, rel)
        {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return; // a binary or unreadable file is simply not a match
        };
        for (n, line) in text.lines().enumerate() {
            if re.is_match(line) && out.len() < MAX_OUTPUT {
                let _ = writeln!(out, "{}:{}: {}", path.display(), n + 1, line.trim());
            }
        }
    };
    if root.is_file() {
        let rel = root.file_name().map(|n| n.to_string_lossy().into_owned());
        search(&root, rel.as_deref().unwrap_or(""));
    } else {
        walk(&root, &mut |path| {
            let rel = path
                .strip_prefix(&root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default();
            search(path, &rel);
        });
    }
    if out.trim().is_empty() {
        return Ok(format!("no matches for {pattern} under {}", root.display()));
    }
    Ok(truncate(&out))
}

// ---- shared helpers ----------------------------------------------------------------

/// A relative path means "in the directory this run is about"; an absolute one means itself.
fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn arg_str<'a>(input: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("this tool needs a `{key}` string argument"))
}

/// A number argument, however the model spelled it — `2` and `2.0` mean the same line here.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the float is range-checked against u64 before it is cast"
)]
fn arg_u64(input: &Value, key: &str) -> Option<u64> {
    let v = input.get(key)?;
    v.as_u64().or_else(|| {
        v.as_f64()
            // 2^63 rather than u64::MAX: the comparison is in floats, and u64::MAX has no exact
            // float form. No line number or timeout is anywhere near either.
            .filter(|f| f.is_finite() && *f >= 0.0 && *f < 9.223_372_036_854_776e18)
            .map(|f| f.round() as u64)
    })
}

/// Cut an over-long result down, saying so where the model will see it — silence here reads as
/// "that was the whole file", which is how an agent ends up editing text that isn't there.
fn truncate(text: &str) -> String {
    if text.len() <= MAX_OUTPUT {
        return text.to_string();
    }
    let mut cut = MAX_OUTPUT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n\n[… {} more bytes. Narrow the request — an offset/limit, a tighter pattern, or a \
         more specific path.]",
        &text[..cut],
        text.len() - cut
    )
}

/// Depth-first walk, skipping the directories nobody means ([`SKIP_DIRS`]) and anything hidden.
fn walk(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}

/// Match `path` against a glob: `?` is one character, `*` any run within a segment, `**` any number
/// of segments. A pattern with no `/` matches the file name alone, so `*.rs` finds Rust files at
/// any depth — which is what everyone means by it.
fn glob_match(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return glob_segments(pattern, name);
    }
    glob_segments(pattern, path)
}

fn glob_segments(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_here(&p, &t)
}

/// Backtracking matcher over the two character slices. Small inputs (a pattern and one path), so
/// the simple recursive form is the honest one — no table, nothing to get subtly wrong.
fn glob_here(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            // `**` crosses `/`; a single `*` stops at one.
            let (rest, crosses) = if p.get(1) == Some(&'*') {
                // Swallow the `/` that usually follows `**`, so `**/x` also matches a bare `x`.
                let after = if p.get(2) == Some(&'/') { &p[3..] } else { &p[2..] };
                (after, true)
            } else {
                (&p[1..], false)
            };
            if glob_here(rest, t) {
                return true;
            }
            for i in 0..t.len() {
                if !crosses && t[i] == '/' {
                    break;
                }
                if glob_here(rest, &t[i + 1..]) {
                    return true;
                }
            }
            false
        }
        '?' if !t.is_empty() && t[0] != '/' => glob_here(&p[1..], &t[1..]),
        c if !t.is_empty() && t[0] == c => glob_here(&p[1..], &t[1..]),
        _ => false,
    }
}

/// Run `cmd` to completion, killing it if it outstays `timeout_ms`. `std` has no timed wait, so
/// this polls — cheaply, and only while a command is actually running.
fn wait_with_timeout(
    mut cmd: Command,
    timeout_ms: u64,
) -> std::result::Result<std::process::Output, String> {
    use std::io::Read as _;
    use std::process::Stdio;

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the shell: {e}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_end(&mut stdout);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_end(&mut stderr);
                }
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the command was still running after {timeout_ms}ms and was killed — run it in \
                     the background, or raise timeout_ms"
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return Err(format!("couldn't wait for the shell: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_bare_pattern_matches_the_file_name_at_any_depth() {
        assert!(glob_match("*.rs", "src/deep/lib.rs"));
        assert!(glob_match("**/*.rs", "src/deep/lib.rs"));
        assert!(glob_match("src/**/*.rs", "src/deep/lib.rs"));
        // `**/` also matches nothing at all, so a file at the root is still found.
        assert!(glob_match("**/lib.rs", "lib.rs"));
        // A single `*` stays inside its segment.
        assert!(!glob_match("src/*.rs", "src/deep/lib.rs"));
        assert!(!glob_match("*.toml", "src/lib.rs"));
    }

    #[test]
    fn edit_refuses_an_ambiguous_match_and_says_how_many() {
        let dir = scratch("edit");
        let file = dir.join("f.txt");
        std::fs::write(&file, "a\na\n").expect("write");
        let input = json!({"path": "f.txt", "old_string": "a", "new_string": "b"});
        let err = edit(&input, &dir).expect_err("ambiguous edit must fail");
        assert!(err.contains("appears 2 times"), "{err}");

        // …and takes them all when told to.
        let all = json!({"path": "f.txt", "old_string": "a", "new_string": "b", "replace_all": true});
        edit(&all, &dir).expect("replace_all");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "b\nb\n");
    }

    #[test]
    fn a_missing_old_string_tells_the_model_what_to_do_about_it() {
        let dir = scratch("edit-missing");
        std::fs::write(dir.join("f.txt"), "hello\n").expect("write");
        let input = json!({"path": "f.txt", "old_string": "nope", "new_string": "x"});
        let err = edit(&input, &dir).expect_err("missing text must fail");
        assert!(err.contains("read the file"), "{err}");
    }

    #[test]
    fn read_resolves_a_relative_path_against_the_runs_own_directory() {
        let dir = scratch("read");
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").expect("write");
        let all = read(&json!({"path": "f.txt"}), &dir).expect("read");
        assert_eq!(all, "one\ntwo\nthree");
        let windowed = read(&json!({"path": "f.txt", "offset": 2, "limit": 1}), &dir).expect("read");
        assert_eq!(windowed, "two");
    }

    #[test]
    fn a_nonzero_exit_comes_back_as_a_result_not_an_error() {
        let dir = scratch("bash");
        let out = bash(&json!({"command": "echo out; exit 3"}), &dir).expect("nonzero is a result");
        assert!(out.contains("exit 3"), "{out}");
        assert!(out.contains("out"), "{out}");
    }

    #[test]
    fn a_command_that_never_finishes_is_killed_and_says_so() {
        let dir = scratch("bash-timeout");
        let err = bash(&json!({"command": "sleep 5", "timeout_ms": 200}), &dir)
            .expect_err("a hung command must fail");
        assert!(err.contains("still running"), "{err}");
    }

    #[test]
    fn grep_reports_path_and_line_and_honours_its_glob() {
        let dir = scratch("grep");
        std::fs::write(dir.join("a.rs"), "fn wanted() {}\n").expect("write");
        std::fs::write(dir.join("b.txt"), "wanted\n").expect("write");
        let hits = grep(&json!({"pattern": "wanted", "glob": "*.rs"}), &dir).expect("grep");
        assert!(hits.contains("a.rs:1:"), "{hits}");
        assert!(!hits.contains("b.txt"), "{hits}");
    }

    #[test]
    fn an_unknown_tool_names_the_ones_that_exist() {
        let err = run("Frobnicate", &json!({}), Path::new(".")).expect_err("unknown tool");
        assert!(err.contains("Read"), "{err}");
    }
}
