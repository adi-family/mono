//! The shell a conversation keeps between `Bash` calls — on every engine, however it runs them.
//!
//! A `Bash` call used to be a shell of its own, so nothing a command set outlived it. The runs
//! showed what that cost: an agent working across two checkouts wrote the same
//! `FE=/Users/x/.adi/mono/projects/<uuid>/workspaces/main` prefix onto command after command,
//! paying for the path again on every line, because the variable it had just set was already gone.
//! [`crate::workspace::block`] meanwhile told it the opposite — that its working directory carries
//! over — which was a promise no engine actually kept for a whole conversation.
//!
//! So the state is carried across calls instead: the directory the last command ended in, and the
//! variables it exported. Two files per conversation, sidecars in the session's own `<id>.*`
//! namespace, which is what makes them disappear when the session is deleted without this module
//! being told.
//!
//! # One shell, however the run is answered
//!
//! [`Shell::script`] is what a `Bash` call actually runs, and there is one caller shape:
//! [`crate::backends::harness::tools`]. The adi loop calls those tools directly; every Claude
//! engine reaches the same ones over MCP (see [`crate::backends::mcp`]), because that CLI's own
//! `Bash` is never granted to a run and ours is served in its place.
//!
//! This replaced a `PreToolUse` hook that rewrote the CLI's `Bash` command in flight. The hook
//! worked, but it could only bend a *command* — never the tool's working directory, and never a
//! background call, whose recording would have landed after later commands had already made theirs
//! and left the conversation in the directory of a job that finished minutes ago. Owning the tool
//! outright has neither problem.
//!
//! # What carries, and what deliberately does not
//!
//! Only what the *conversation* changed. Each call dumps the environment it was launched with
//! before it loads the carried one, and keeps the difference — so a variable the runner refreshes
//! between turns (a rotated secret, a re-composed prompt) reaches the command as the runner meant
//! it, while `export FE=…` from three calls ago is still set. A plain `FE=…` assignment is not
//! exported and does not carry; the tool description says so, because `export` is the one-word fix.
//!
//! What is carried is the shell's own state and nothing else's: the file tools keep resolving
//! relative paths against the run directory, whatever the shell has since been moved to. A `cd` is
//! therefore reported back when it happens, so the model is never guessing which of the two it just
//! moved.
//!
//! Unix only. Windows runs `cmd /C`, which has neither `export -p` nor a usable `trap`, so a call
//! there stays what every call used to be — a shell of its own.

use std::path::{Path, PathBuf};

/// One conversation's shell state, as the two files holding it.
pub(crate) struct Shell {
    /// The directory the last command ended in.
    cwd: PathBuf,
    /// The variables this conversation exported, as sourceable `export` lines.
    env: PathBuf,
}

impl Shell {
    /// The shell belonging to one session, as sidecars of that session's files.
    ///
    /// Keyed by *adi's* session id rather than the engine's, so one conversation has one shell
    /// whichever engine answers its turns, and the files sit where the session's own cleanup
    /// already looks.
    ///
    /// Named `<id>.shell-*` for the reason `crate::store` documents: a session owns its whole
    /// `<id>.*` namespace and is deleted by sweeping the prefix, so state parked inside it is
    /// cleaned up by machinery that has never heard of it. Files rather than a directory, because
    /// that sweep removes files.
    pub(crate) fn new(agent_dir: &Path, session_id: &str) -> Self {
        Self {
            cwd: agent_dir.join(format!("{session_id}.shell-cwd")),
            env: agent_dir.join(format!("{session_id}.shell-env")),
        }
    }

    /// Where the next command starts: where the last one ended, or `home` — the run's own directory
    /// — when there was no last one, or when what it recorded is no longer a directory.
    ///
    /// Falling back rather than failing is deliberate: a conversation whose `cd` target has since
    /// been deleted is still a conversation, and the run directory is the one place it can always
    /// be resumed in.
    pub(crate) fn start_dir(&self, home: &Path) -> PathBuf {
        std::fs::read_to_string(&self.cwd)
            .map(|dir| PathBuf::from(dir.trim()))
            .ok()
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(|| home.to_path_buf())
    }

    /// Where the shell ended up, when it recorded anything.
    pub(super) fn ended_in(&self) -> Option<PathBuf> {
        std::fs::read_to_string(&self.cwd)
            .map(|dir| PathBuf::from(dir.trim()))
            .ok()
            .filter(|dir| !dir.as_os_str().is_empty())
    }

    /// Where the shell ended up, but only when that is somewhere other than `start` — the answer to
    /// "did this command move me", which is the only time it is worth saying anything.
    ///
    /// Compared as resolved paths, because the recorded one is physical (`pwd -P`) while `start` is
    /// whatever the caller was handed. On a machine whose run directory reaches through a symlink —
    /// every macOS `/tmp` and `/var` path, to begin with — comparing them as written would report a
    /// move on the first command of every run, which is exactly the noise this is meant to replace.
    pub(super) fn moved_from(&self, start: &Path) -> Option<PathBuf> {
        let ended = self.ended_in()?;
        let resolve = |dir: &Path| std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        (resolve(&ended) != resolve(start)).then_some(ended)
    }

    /// `command` with the conversation's state loaded in front of it and **nothing recorded
    /// behind** — what a background job runs under.
    ///
    /// A job inherits where the conversation stands, which is what makes `cd /checkout` and then
    /// `Bash(background) make` mean what it reads like. It must not record, and that asymmetry is
    /// the whole point: a job outlives the call that started it, so its recording would land after
    /// however many later commands had already made theirs, and the conversation would silently
    /// inherit the directory of a job that finished minutes ago.
    #[cfg(unix)]
    pub(crate) fn inherit(&self, command: &str) -> String {
        format!("{}\n{command}", self.prologue(false))
    }

    /// Windows has no session shell — see the module docs.
    #[cfg(windows)]
    pub(crate) fn inherit(&self, command: &str) -> String {
        command.to_string()
    }

    /// Load the carried variables and walk to the recorded directory — the half both callers share.
    ///
    /// `baseline` dumps the launch environment to a scratch file so the recording at the end can
    /// keep the *difference*. Only [`Self::script`] wants it: it is the one that records, and it is
    /// the one whose trap deletes the file again. A job asking for it would leave one behind on
    /// every call, since nothing it runs ever cleans up — which is exactly what it did until this
    /// was a parameter.
    #[cfg(unix)]
    fn prologue(&self, baseline: bool) -> String {
        let env = quote(&self.env);
        let cwd = quote(&self.cwd);
        // Dumped *before* the carried state is loaded, so the difference kept at the end is
        // everything this conversation has ever exported, not just this call's.
        let base = if baseline {
            "__adi_base=\"$__adi_env.$$.base\"\n\
             __adi_next=\"$__adi_env.$$.next\"\n\
             export -p > \"$__adi_base\" 2>/dev/null\n"
        } else {
            ""
        };
        format!(
            // `sh -n` first: a sourced file with a syntax error takes the whole shell down with it,
            // and this one is written from whatever the model exported.
            "__adi_env={env}\n\
             __adi_cwd={cwd}\n\
             {base}\
             if [ -f \"$__adi_env\" ] && sh -n \"$__adi_env\" 2>/dev/null; then . \"$__adi_env\"; fi\n\
             if [ -f \"$__adi_cwd\" ]; then __adi_to=$(cat \"$__adi_cwd\"); \
             if [ -d \"$__adi_to\" ]; then cd \"$__adi_to\" || true; fi; fi"
        )
    }

    /// `command` with the state around it: the carried variables loaded in front, the directory and
    /// the new variables recorded behind.
    ///
    /// The recording hangs off an `EXIT` trap so that it still happens when the command itself
    /// exits, and `$?` is captured as the trap's first act so the command's status is what the call
    /// returns. A command killed on timeout records nothing — it was killed precisely because it
    /// was not going to finish, and half of a shell's state is worse than none.
    ///
    /// The walk to the recorded directory is part of the script rather than the caller's business,
    /// because the tool that runs it has no say in where its own child starts.
    #[cfg(unix)]
    pub(crate) fn script(&self, command: &str) -> String {
        format!(
            "{}\n\
             __adi_keep() {{\n\
             __adi_status=$?\n\
             pwd -P > \"$__adi_cwd\" 2>/dev/null\n\
             export -p 2>/dev/null \
             | grep -vxF -f \"$__adi_base\" \
             | grep -Ev '^(export|declare -x) (PWD|OLDPWD|SHLVL|_)=' \
             > \"$__adi_next\" 2>/dev/null\n\
             mv -f \"$__adi_next\" \"$__adi_env\" 2>/dev/null\n\
             rm -f \"$__adi_base\" \"$__adi_next\" 2>/dev/null\n\
             return $__adi_status\n\
             }}\n\
             trap __adi_keep EXIT\n\
             {command}",
            self.prologue(true)
        )
    }

    /// Windows has no session shell — see the module docs — so the command runs as written.
    #[cfg(windows)]
    pub(crate) fn script(&self, command: &str) -> String {
        command.to_string()
    }
}

/// `path` as one single-quoted `sh` word. Everything inside single quotes is literal, so the only
/// character needing work is the quote itself.
#[cfg(unix)]
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adi-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Run one command through the session shell, the way `tools::bash` does.
    #[cfg(unix)]
    fn call(shell: &Shell, home: &Path, command: &str) -> String {
        let out = Command::new("sh")
            .arg("-c")
            .arg(shell.script(command))
            .current_dir(shell.start_dir(home))
            .output()
            .expect("sh");
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        text
    }

    /// The whole point: a path named once is still named on the next call, and so is the directory
    /// the last command walked to.
    #[test]
    #[cfg(unix)]
    fn an_exported_variable_and_a_cd_reach_the_next_call() {
        let home = scratch("carry");
        let elsewhere = home.join("workspaces/main");
        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        let shell = Shell::new(&home, "conv");

        let first = call(
            &shell,
            &home,
            "export FE=\"$PWD/workspaces/main\"; cd \"$FE\"; echo set",
        );
        assert!(first.contains("set"), "{first}");

        // `$FE` was set from `$PWD`, which the shell reports physically — so is the directory it
        // recorded, and so is what the next call has to agree with.
        let there = std::fs::canonicalize(&elsewhere).expect("canonicalize");
        let there = there.to_str().expect("utf8");
        let second = call(&shell, &home, "echo \"$FE\"; pwd -P");
        assert!(second.contains(there), "{second}");
        assert_eq!(
            shell.ended_in(),
            Some(PathBuf::from(there)),
            "the recorded directory is where the command ended"
        );
        assert!(shell.moved_from(&shell.start_dir(&home)).is_none());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Only what the conversation changed is carried. The launch environment is dumped fresh on
    /// every call, so a variable the runner refreshes between turns is not overwritten by a copy
    /// frozen three calls ago.
    #[test]
    #[cfg(unix)]
    fn the_launch_environment_is_not_frozen_into_the_carried_state() {
        let home = scratch("delta");
        let shell = Shell::new(&home, "conv");
        call(&shell, &home, "export FE=/somewhere");

        let kept = std::fs::read_to_string(&shell.env).expect("the env file");
        assert!(kept.contains("FE="), "{kept}");
        for inherited in ["PATH=", "HOME=", "PWD="] {
            assert!(
                !kept.contains(inherited),
                "{inherited} was inherited, not set here: {kept}"
            );
        }

        let _ = std::fs::remove_dir_all(&home);
    }

    /// The command's own exit status is what the call reports — the recording runs after it, and
    /// must not become the answer.
    #[test]
    #[cfg(unix)]
    fn the_commands_exit_status_survives_the_recording() {
        let home = scratch("status");
        let shell = Shell::new(&home, "conv");
        let status = Command::new("sh")
            .arg("-c")
            .arg(shell.script("exit 3"))
            .current_dir(&home)
            .status()
            .expect("sh");
        assert_eq!(status.code(), Some(3));

        assert!(shell.ended_in().is_some());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A conversation whose directory has been deleted starts where it can always start.
    #[test]
    fn a_vanished_directory_falls_back_to_the_run_directory() {
        let home = scratch("gone");
        let shell = Shell::new(&home, "conv");
        assert_eq!(shell.start_dir(&home), home, "nothing recorded yet");
        std::fs::write(&shell.cwd, "/no/such/place\n").expect("write");
        assert_eq!(shell.start_dir(&home), home);

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A broken env file is skipped rather than taking the shell down with it — a sourced syntax
    /// error would otherwise end every command in the conversation, not just the one that wrote it.
    #[test]
    #[cfg(unix)]
    fn an_unparseable_carried_environment_is_ignored() {
        let home = scratch("broken");
        let shell = Shell::new(&home, "conv");
        std::fs::write(&shell.env, "export BROKEN='unterminated\n").expect("write");
        let out = call(&shell, &home, "echo still here");
        assert!(out.contains("still here"), "{out}");

        let _ = std::fs::remove_dir_all(&home);
    }
}
