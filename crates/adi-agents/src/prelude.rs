//! Commands run *before* a conversation's first message reaches the model.
//!
//! A launcher very often already knows the first thing the agent will have to do. The bug-bounty
//! filer knows the task it just wrote (`adi-mono tasks show BUGBOUNTY-465`); a target agent always
//! opens by reading its brief. Left to the agent, each of those costs a whole round trip: the model
//! reads the instruction, emits a tool call, and the turn ends having learned only what the
//! launcher could have told it. Two turns and two prompt-loads to get to the first line of work.
//!
//! So the launch runs them itself and carries the output in. **Really runs them** — same shell as
//! the agent's own `Bash` tool, same directory, same `PATH` and environment, real exit status, real
//! stdout and stderr. Nothing here fabricates, predicts, or summarizes output; a command that fails
//! reports its failure, and one that cannot start says so.
//!
//! # Why it is framed as a tool call rather than folded into the words
//!
//! The output has to arrive labelled as *machine output the platform obtained*, not as something
//! the person said. A model that reads a wall of task detail inside a human's message treats it as
//! the human's claims — quotable, arguable, possibly stale — where the same text under a tool
//! banner is read as the ground truth it is, and is not re-fetched "to check". [`block`] therefore
//! renders each command as an explicit pre-run call with its status, and [`steps`] records the same
//! calls into the transcript as [`Step::Tool`] so a reader sees them where the agent's own calls
//! appear.
//!
//! # Where it sits
//!
//! The agent layer, not the runner. Every engine gets the same block appended to its opening
//! message, so this needs no per-engine support and a new backend inherits it. The runner still
//! knows nothing but "here is the message to send" — see `docs/agent-runner.md`.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use crate::backends::harness::tools::{truncate, wait_with_timeout};
use crate::backends::shell::Shell;
use crate::progress::{Step, ToolStatus};
use crate::runner::RunSpec;

/// How long one pre-run command may take before it is killed. The same budget the `Bash` tool
/// gives a call, for the same reason: this *is* that tool, run early. It is a backstop rather than
/// an expectation — a prelude is an orientation read, and a launch waits inside it.
const TIMEOUT_MS: u64 = 120_000;

/// How many commands one launch may pre-run. Nothing sensible needs more, and a launch is a
/// synchronous call somebody is waiting on — an API caller that sends two hundred would otherwise
/// hold the request open for as long as they take. What is dropped is named in the block rather
/// than silently cut.
pub(crate) const MAX_COMMANDS: usize = 16;

/// The tool name a pre-run command is recorded under.
///
/// `Bash` because that is the tool it is: the same shell, the same conversation state, the same
/// output conventions. A name of its own would put a tool in the transcript that the model has
/// never been declared and cannot call itself, and a reader would have to learn a second thing that
/// means "a command ran".
const TOOL: &str = "Bash";

/// One command that was run, and what it actually produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    /// The command, as written.
    pub(crate) command: String,
    /// Its combined stdout and stderr, truncated and prefixed with `(exit N)` on a non-zero
    /// status — byte for byte what the `Bash` tool would have returned for the same command.
    ///
    /// The status lives *in* the output rather than beside it so that a [`Ran`] survives a round
    /// trip through a transcript step and back ([`block_of_steps`]), which is how an engine that
    /// replays the transcript rather than taking the message on its command line is shown the same
    /// thing every other engine is.
    pub(crate) output: String,
    /// Whether it exited zero.
    pub(crate) ok: bool,
}

impl Ran {
    /// How this call's status is stated on its tag: the two words a model needs, rather than a
    /// number it would have to know a convention to read. The number is still in the output.
    fn status(&self) -> &'static str {
        if self.ok { "ok" } else { "failed" }
    }
}

/// Run `commands` in this launch's own context, in order, and report what each produced.
///
/// `spec` supplies the directory, `PATH`, and environment the agent's own commands will run in, so
/// a pre-run resolves the same binaries its `Bash` will — including the agent's `.bin` shims, which
/// exist on no other `PATH`. `agent_dir` and `conv` locate the conversation's shell, which is
/// deliberately shared: a prelude that `cd`s or exports leaves the agent's first `Bash` call
/// standing where it put it.
///
/// Never fails as a whole. A command that exits non-zero, times out, or cannot start is one
/// [`Ran`] with the bad news in it — the run still starts, and the model is told what happened
/// rather than being handed a launch that silently did less than it said.
pub(crate) fn run(commands: &[String], spec: &RunSpec, agent_dir: &Path, conv: &str) -> Vec<Ran> {
    let shell = Shell::new(agent_dir, conv);
    commands
        .iter()
        .map(|command| command.trim())
        .filter(|command| !command.is_empty())
        .take(MAX_COMMANDS)
        .map(|command| run_one(command, spec, &shell))
        .collect()
}

/// One command, through the conversation's shell.
fn run_one(command: &str, spec: &RunSpec, shell: &Shell) -> Ran {
    let start = shell.start_dir(&spec.cwd);
    let script = shell.script(command);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&script);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&script);
        c
    };
    cmd.current_dir(&start)
        // The run's own environment, exactly as the engine's child will get it — this process is
        // the CLI or the app server, whose `PATH` has never heard of the agent's `.bin`. `PATH`
        // goes last so nothing in `env` can strand the command, which is the order `spawn_child`
        // uses for the engine itself.
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .env("PATH", &spec.path);

    match wait_with_timeout(cmd, TIMEOUT_MS) {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if output.status.success() {
                return Ran {
                    command: command.to_string(),
                    output: truncate(if text.trim().is_empty() {
                        "(no output)"
                    } else {
                        &text
                    }),
                    ok: true,
                };
            }
            // The `Bash` tool's own spelling of a failure, because this is that tool: a model that
            // has learned to read `(exit 3)` from its own calls reads it here without being taught
            // a second convention.
            let code = output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string());
            Ran {
                command: command.to_string(),
                output: truncate(&format!("(exit {code})\n{text}")),
                ok: false,
            }
        }
        // The shell never started, or ran past the timeout and was killed. Reported as the command's
        // result rather than swallowed: an agent that is told its brief could not be fetched goes and
        // fetches it, where one told nothing works from the gap without knowing there is one.
        Err(why) => Ran {
            command: command.to_string(),
            output: format!("(not run)\n{why}"),
            ok: false,
        },
    }
}

/// What gets appended to the opening message, or `None` when nothing was run.
///
/// The heading and the sentence under it are doing real work: they say the calls already happened,
/// that the text is the command's own output, and that re-running is unnecessary. Without that last
/// part a model reliably re-runs the command anyway to "verify" it, which is the round trip this
/// exists to save.
#[must_use]
pub(crate) fn block(ran: &[Ran], dropped: usize) -> Option<String> {
    if ran.is_empty() && dropped == 0 {
        return None;
    }
    let mut out = String::from(
        "# Already run for you\n\n\
         Before this message reached you, the commands below were run as `Bash` tool calls — in \
         this run's own shell, in its working directory. Each block holds that command's real \
         output and its real exit status. Treat it as a tool result you already have: use it, and \
         do not run the command again unless you need a fresher answer or it failed.\n",
    );
    for one in ran {
        // Tagged rather than fenced: the output routinely contains code fences of its own, and a
        // fence inside a fence ends the wrong one. The status rides on the open tag so it cannot be
        // read as part of the output.
        // Writing into a `String` cannot fail, so the result is dropped rather than unwrapped.
        let _ = write!(
            out,
            "\n<pre-run command=\"{}\" status=\"{}\">\n{}\n</pre-run>\n",
            one.command.replace('"', "'"),
            one.status(),
            one.output.trim_end(),
        );
    }
    if dropped > 0 {
        let _ = write!(
            out,
            "\n({dropped} further command(s) were not run — a launch pre-runs at most \
             {MAX_COMMANDS}. Run them yourself if you need them.)\n"
        );
    }
    Some(out)
}

/// The same calls as transcript steps, so a reader sees them on the opening turn where the agent's
/// own tool calls appear.
///
/// The transcript keeps the message a person actually wrote — the block above is engine-facing
/// text, for the same reason an image's file paths are (see `Agents::for_engine`). Recording the
/// calls as steps is how the opening turn still shows what was run, without a reader having to find
/// it inside a wall of quoted output.
/// The block again, rebuilt from a recorded turn's steps — for an engine that reads the words back
/// out of the transcript instead of taking them on a command line.
///
/// The `harness:adi` loop is that engine: its turn child is handed an agent and a conversation id
/// and replays the stored transcript, so anything appended to the message at launch never reaches
/// it. Rebuilding here rather than recording a second copy of the text is what keeps one truth in
/// the store — and the round trip is lossless, because a step carries the command, the output, and
/// the status the same way [`Ran`] does.
///
/// `None` when the turn has no pre-run steps, which is every turn of every agent that uses none.
#[must_use]
pub(crate) fn block_of_steps(steps: &[Step]) -> Option<String> {
    let ran: Vec<Ran> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Tool {
                name,
                input,
                status,
                output,
            } if name == TOOL => Some(Ran {
                command: input.clone(),
                output: output.clone(),
                ok: *status == ToolStatus::Ok,
            }),
            _ => None,
        })
        .collect();
    block(&ran, 0)
}

#[must_use]
pub(crate) fn steps(ran: &[Ran]) -> Vec<Step> {
    ran.iter()
        .map(|one| Step::Tool {
            name: TOOL.to_string(),
            input: one.command.clone(),
            status: if one.ok {
                ToolStatus::Ok
            } else {
                ToolStatus::Error
            },
            output: one.output.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunSpec;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-prelude-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn spec_in(cwd: &Path) -> RunSpec {
        RunSpec {
            cwd: cwd.to_path_buf(),
            path: std::env::var("PATH").unwrap_or_default(),
            env: vec![("ADI_PRELUDE_TEST".to_string(), "carried".to_string())],
            arguments: serde_json::Value::Null,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
        }
    }

    /// The whole promise of the feature: the command is executed and what comes back is what it
    /// printed. A stub, a prediction, or an echo of the command would pass a weaker test.
    #[test]
    #[cfg(unix)]
    fn a_command_really_runs_and_its_real_output_comes_back() {
        let dir = scratch("really-runs");
        std::fs::write(dir.join("brief.txt"), "the actual file contents\n").expect("write");

        let ran = run(
            &["cat brief.txt".to_string()],
            &spec_in(&dir),
            &dir,
            "conv-1",
        );

        assert_eq!(ran.len(), 1);
        assert!(ran[0].ok, "a successful command reports success");
        assert!(
            ran[0].output.contains("the actual file contents"),
            "the file's real contents, not a description of them: {}",
            ran[0].output
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The run's environment reaches the command. Without this a pre-run resolves binaries against
    /// the *launcher's* `PATH`, and every agent `.bin` shim — which exists nowhere else — is
    /// missing exactly when the launch says it ran.
    #[test]
    #[cfg(unix)]
    fn a_command_runs_with_the_runs_own_environment() {
        let dir = scratch("env");
        let ran = run(
            &["printf %s \"$ADI_PRELUDE_TEST\"".to_string()],
            &spec_in(&dir),
            &dir,
            "conv-1",
        );
        assert_eq!(ran[0].output, "carried");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failure is information the model needs, not a launch error. It arrives with its status so
    /// the agent can decide to run the thing itself.
    #[test]
    #[cfg(unix)]
    fn a_failing_command_reports_its_status_and_output() {
        let dir = scratch("failing");
        let ran = run(
            &["echo nope >&2; exit 3".to_string()],
            &spec_in(&dir),
            &dir,
            "conv-1",
        );
        assert!(!ran[0].ok);
        assert!(
            ran[0].output.starts_with("(exit 3)"),
            "the real exit status, spelled as the Bash tool spells it: {}",
            ran[0].output
        );
        assert!(ran[0].output.contains("nope"));

        let steps = steps(&ran);
        assert!(matches!(
            &steps[0],
            Step::Tool { status, .. } if *status == ToolStatus::Error
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One shell across the prelude and the turn that follows it: a `cd` in a pre-run command is
    /// where the agent's first `Bash` call starts. This is why the conversation's shell is shared
    /// rather than a fresh one being opened per launch.
    #[test]
    #[cfg(unix)]
    fn the_shell_carries_from_one_command_to_the_next() {
        let dir = scratch("shell");
        std::fs::create_dir_all(dir.join("sub")).expect("subdir");

        let ran = run(
            &[
                "cd sub && export MARK=here".to_string(),
                "printf '%s %s' \"$(basename \"$PWD\")\" \"$MARK\"".to_string(),
            ],
            &spec_in(&dir),
            &dir,
            "conv-1",
        );

        assert_eq!(ran[1].output, "sub here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The block must read as a tool result. The three things a model has to take from it: it
    /// already happened, this is the command's own output, and it need not be repeated.
    #[test]
    fn the_block_frames_the_output_as_a_tool_call_that_already_ran() {
        let ran = vec![Ran {
            command: "adi-mono tasks show BUGBOUNTY-465".to_string(),
            output: "Title: probe the auth flow".to_string(),
            ok: true,
        }];
        let block = block(&ran, 0).expect("a block");

        assert!(block.contains("Already run for you"));
        assert!(block.contains("`Bash` tool calls"));
        assert!(block.contains("do not run the command again"));
        assert!(block.contains("<pre-run command=\"adi-mono tasks show BUGBOUNTY-465\" status=\"ok\">"));
        assert!(block.contains("Title: probe the auth flow"));
    }

    /// Nothing run, nothing appended — an agent with no prelude must get exactly the message that
    /// was typed.
    #[test]
    fn no_commands_means_no_block() {
        assert!(block(&[], 0).is_none());
    }

    /// A cap that quietly dropped commands would read as "everything ran". It says what it left.
    #[test]
    fn dropped_commands_are_named_not_silently_cut() {
        let block = block(&[], 3).expect("a block");
        assert!(block.contains("3 further command(s) were not run"));
    }

    #[test]
    #[cfg(unix)]
    fn blank_commands_are_skipped_and_the_cap_holds() {
        let dir = scratch("cap");
        let mut commands = vec!["   ".to_string()];
        commands.extend((0..MAX_COMMANDS + 4).map(|i| format!("echo {i}")));

        let ran = run(&commands, &spec_in(&dir), &dir, "conv-1");
        assert_eq!(ran.len(), MAX_COMMANDS);
        assert_eq!(
            ran[0].output.trim(),
            "0",
            "the blank one was skipped, so the first result is the first real command"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
