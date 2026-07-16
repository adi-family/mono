//! Bundled port of `adi.workforce.capability.shell` — the `Shell` tool.
//!
//! Differences from the old plugin: the optional LLM safety checker
//! (`prompt_runner` setting) is not ported — its PromptRunner registry was
//! part of the dlopen plugin infrastructure. Regex `allowed`/`denied` rules
//! and the prompt-only descriptions carry over unchanged. The
//! `SwitchToBranch` tool and pre/post-loop lifetimes (GitLab MR workflow)
//! stay behind too — lifetimes now live on the TS side.

use std::sync::Arc;

use crate::config_value::ConfigValue;
use crate::loop_run_context::LoopRunContext;
use crate::plugin::PluginError;
use crate::tool_def::{Tool, ToolCallError, ToolResult};

// ── Settings (hand-rolled port of the tsp-gen structs) ──

#[derive(Debug, Clone)]
pub struct ShellPattern {
    pub pattern: String,
}

#[derive(Debug, Clone)]
pub struct ShellRule {
    pub patterns: Vec<ShellPattern>,
    pub reason: String,
}

impl ShellRule {
    fn from_config(v: &ConfigValue) -> Self {
        let patterns = v
            .get("patterns")
            .and_then(ConfigValue::as_list)
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        p.get("pattern")
                            .and_then(|s| s.as_str())
                            .map(|s| ShellPattern {
                                pattern: s.to_string(),
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let reason = v
            .get("reason")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        Self { patterns, reason }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ShellSettings {
    pub allowed: Option<Vec<ShellRule>>,
    pub denied: Option<Vec<ShellRule>>,
    pub allowed_prompt: Option<String>,
    pub denied_prompt: Option<String>,
}

impl ShellSettings {
    fn from_config(cfg: &ConfigValue) -> Self {
        let rules = |key: &str| {
            cfg.get(key).and_then(ConfigValue::as_list).map(|list| {
                list.iter()
                    .map(ShellRule::from_config)
                    .collect::<Vec<_>>()
            })
        };
        let prompt = |key: &str| {
            cfg.get(key)
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        };
        Self {
            allowed: rules("allowed"),
            denied: rules("denied"),
            allowed_prompt: prompt("allowedPrompt"),
            denied_prompt: prompt("deniedPrompt"),
        }
    }
}

// ── Shell tool ──

pub struct ShellTool {
    settings: ShellSettings,
    description: String,
    /// Monotonic counter used to name spill files when shell output is
    /// large enough to be truncated. Starts at 0 and increments per
    /// call across all runs using this tool instance.
    shell_seq: std::sync::atomic::AtomicUsize,
}

impl ShellTool {
    /// Factory registered under `adi.workforce.capability.shell` / `Shell`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::ToolCreateFn`].
    pub fn create(config: ConfigValue) -> Result<Arc<dyn Tool>, PluginError> {
        let settings = ShellSettings::from_config(&config);

        let mut desc =
            "Execute a shell command in the workdir. Returns stdout and stderr.".to_string();

        if let Some(ref prompt) = settings.allowed_prompt {
            desc.push_str(&format!("\n\nAllowed: {prompt}"));
        } else if let Some(ref allowed) = settings.allowed {
            let reasons: Vec<&str> = allowed.iter().map(|r| r.reason.as_str()).collect();
            desc.push_str(&format!("\n\nAllowed: {}", reasons.join(", ")));
        }

        if let Some(ref prompt) = settings.denied_prompt {
            desc.push_str(&format!("\n\nDenied: {prompt}"));
        } else if let Some(ref denied) = settings.denied {
            let reasons: Vec<&str> = denied.iter().map(|r| r.reason.as_str()).collect();
            desc.push_str(&format!("\n\nDenied: {}", reasons.join(", ")));
        }

        Ok(Arc::new(Self {
            settings,
            description: desc,
            shell_seq: std::sync::atomic::AtomicUsize::new(0),
        }))
    }
}

impl Tool for ShellTool {
    fn name(&self) -> String {
        "shell".to_string()
    }
    fn description(&self) -> String {
        self.description.clone()
    }

    fn system_prompt(&self) -> Option<String> {
        Some(
            "### Shell tool\n\
             All shell commands run inside the project working directory.\n\
             NEVER use absolute paths outside the workdir (no /Users/..., no ~/..., no /..).\n\
             Use relative paths only. The workdir is set per-loop — you don't need to cd into it."
                .to_string(),
        )
    }

    fn parameters_json(&self) -> String {
        r#"{"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute in the project workdir. Use relative paths only — never absolute paths outside the project.","minLength":1,"maxLength":10000}},"required":["command"]}"#.to_string()
    }

    fn parse(&self, raw: &str) -> Result<ConfigValue, ToolCallError> {
        let args = ConfigValue::from_json(raw)
            .map_err(|e| ToolCallError::Internal(format!("invalid JSON: {e}")))?;
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::BadRequest("missing 'command'".to_string()))?;
        if cmd.is_empty() {
            return Err(ToolCallError::BadRequest(
                "'command' must not be empty".to_string(),
            ));
        }
        Ok(args)
    }

    fn execute(&self, ctx: &LoopRunContext, raw_args: ConfigValue) -> Result<String, PluginError> {
        let command = raw_args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.handle(ctx, &command)
    }
}

impl ShellTool {
    fn handle(&self, ctx: &LoopRunContext, command: &str) -> ToolResult {
        // The filesystem sandbox is the boundary; we don't re-check
        // path traversal or absolute paths here — the sandbox prevents
        // escapes, and token-level checks produced false positives
        // (e.g. Go's `./...`) that blocked legit commands.

        // 1. Regex denied rules
        if let Some(ref denied) = self.settings.denied {
            if let Some(rule) = find_matching_rule(denied, command) {
                return Err(PluginError::new(format!("command denied: {}", rule.reason)));
            }
        }

        // 2. Regex allowed rules
        if let Some(ref allowed) = self.settings.allowed {
            if find_matching_rule(allowed, command).is_none() {
                let reasons: Vec<&str> = allowed.iter().map(|r| r.reason.as_str()).collect();
                return Err(PluginError::new(format!(
                    "command not allowed. Permitted: {}",
                    reasons.join(", ")
                )));
            }
        }

        let output = run_sh(command, &ctx.workdir)?;
        let preview = if command.len() > 80 {
            let end = command
                .char_indices()
                .nth(80)
                .map_or(command.len(), |(i, _)| i);
            &command[..end]
        } else {
            command
        };
        eprintln!(
            "[shell] {} exit={} cmd={}",
            ctx.employee, output.exit_code, preview
        );
        let seq = self
            .shell_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(output.format_with_spill(ctx, seq))
    }
}

fn find_matching_rule<'a>(rules: &'a [ShellRule], command: &str) -> Option<&'a ShellRule> {
    rules.iter().find(|rule| {
        rule.patterns.iter().any(|p| {
            regex::Regex::new(&p.pattern)
                .map(|re| re.is_match(command))
                .unwrap_or(false)
        })
    })
}

// ── Output formatting ──

struct ShellOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

/// Truncation thresholds. If combined (stdout + stderr) is <= INLINE_MAX,
/// the tool returns everything inline. Above that, we spill full output
/// to files under `.adi/shell/<run>/call-<seq>.*.txt` inside the sandbox
/// and return head + tail in the tool result so the model has both a
/// prefix and (critically) the tail, where failure summaries usually
/// live. The model is expected to inspect the spill files via
/// `shell grep/sed` or `file_read` when it needs more detail — NOT
/// by re-running the command.
const SHELL_INLINE_MAX: usize = 8000;
const SHELL_HEAD_LEN: usize = 3000;
const SHELL_TAIL_LEN: usize = 3000;

impl ShellOutput {
    /// Inline format used when output fits under the threshold.
    fn format_inline(&self) -> String {
        let mut r = String::new();
        if !self.stdout.is_empty() {
            r.push_str(&self.stdout);
        }
        if !self.stderr.is_empty() {
            if !r.is_empty() {
                r.push('\n');
            }
            r.push_str("[stderr]\n");
            r.push_str(&self.stderr);
        }
        if self.exit_code != 0 {
            r.push_str(&format!("\n[exit code: {}]", self.exit_code));
        }
        if r.is_empty() {
            r.push_str("[no output]");
        }
        r
    }

    fn format_with_spill(&self, ctx: &LoopRunContext, shell_seq: usize) -> String {
        let combined_len = self.stdout.len() + self.stderr.len();
        if combined_len <= SHELL_INLINE_MAX {
            return self.format_inline();
        }

        // Spill path. Short directory per-run (hash of ctx.id to keep
        // the path tidy); per-call file with monotonic seq.
        let run_tag = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            ctx.id.hash(&mut h);
            format!("{:x}", h.finish())
        };
        let dir = ctx.workdir.join(".adi").join("shell").join(&run_tag);
        let mkdir_ok = std::fs::create_dir_all(&dir).is_ok();

        let stdout_path = dir.join(format!("call-{shell_seq}.stdout.txt"));
        let stderr_path = dir.join(format!("call-{shell_seq}.stderr.txt"));
        let combined_path = dir.join(format!("call-{shell_seq}.combined.txt"));

        let mut spill_ok = false;
        if mkdir_ok {
            let wrote_stdout = std::fs::write(&stdout_path, &self.stdout).is_ok();
            let wrote_stderr = std::fs::write(&stderr_path, &self.stderr).is_ok();
            // combined = stdout + "\n[stderr]\n" + stderr, matching the
            // inline format so greps behave the same as on short calls.
            let mut combined = self.stdout.clone();
            if !self.stderr.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str("[stderr]\n");
                combined.push_str(&self.stderr);
            }
            let wrote_combined = std::fs::write(&combined_path, &combined).is_ok();
            spill_ok = wrote_stdout && wrote_stderr && wrote_combined;
        }

        // Build head + tail from combined output (same conceptual
        // content as the spilled combined file). Char-safe slicing.
        let mut combined = String::with_capacity(combined_len + 16);
        combined.push_str(&self.stdout);
        if !self.stderr.is_empty() {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str("[stderr]\n");
            combined.push_str(&self.stderr);
        }

        let head = {
            let end = combined
                .char_indices()
                .nth(SHELL_HEAD_LEN)
                .map_or(combined.len(), |(i, _)| i);
            &combined[..end]
        };
        let tail = {
            let start = if combined.len() > SHELL_TAIL_LEN {
                // Find a char boundary at or after len-TAIL_LEN.
                let approx = combined.len() - SHELL_TAIL_LEN;
                let mut i = approx;
                while i < combined.len() && !combined.is_char_boundary(i) {
                    i += 1;
                }
                i
            } else {
                0
            };
            &combined[start..]
        };

        let total_bytes = combined.len();
        let omitted = total_bytes.saturating_sub(head.len() + tail.len());

        // Render. Relative paths from the sandbox root — model can pass
        // them to `file_read` / `shell grep` / `shell sed` directly.
        let rel_stdout = format!(".adi/shell/{run_tag}/call-{shell_seq}.stdout.txt");
        let rel_combined = format!(".adi/shell/{run_tag}/call-{shell_seq}.combined.txt");
        let rel_stderr = format!(".adi/shell/{run_tag}/call-{shell_seq}.stderr.txt");

        let spill_block = if spill_ok {
            format!(
                "\n\n=== FULL OUTPUT SAVED ===\n  {} ({} bytes)\n  {} ({} bytes)\n  {} ({} bytes)\n\nTo drill in: grep/sed on these paths via shell, or file_read. Examples:\n  grep -n \"error\" {} | head -20\n  sed -n '5000,5100p' {}\n\nDO NOT re-run the original command to get more output — use these files.",
                rel_combined, total_bytes,
                rel_stdout, self.stdout.len(),
                rel_stderr, self.stderr.len(),
                rel_combined, rel_stdout,
            )
        } else {
            format!(
                "\n\n=== FULL OUTPUT NOT SAVED ===\n  (spill dir {} could not be created — output only available inline above)",
                dir.display()
            )
        };

        let exit_note = if self.exit_code == 0 {
            String::new()
        } else {
            format!("\n[exit code: {}]", self.exit_code)
        };

        format!(
            "[stdout+stderr truncated — {total} bytes total, showing first {head} + last {tail} chars]\n\n=== HEAD (first {head} chars) ===\n{head_content}\n\n... {omitted} bytes omitted ...\n\n=== TAIL (last {tail} chars) ===\n{tail_content}{exit}{spill}",
            total = total_bytes,
            head = head.len(),
            tail = tail.len(),
            head_content = head,
            tail_content = tail,
            omitted = omitted,
            exit = exit_note,
            spill = spill_block,
        )
    }
}

fn run_sh(cmd: &str, workdir: &std::path::Path) -> Result<ShellOutput, PluginError> {
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .output()
        .map_err(|e| PluginError::new(format!("shell exec failed: {e}")))?;
    Ok(ShellOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}
