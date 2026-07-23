//! The environment a supervised runner is launched with.
//!
//! A process spawned under **launchd** (a LaunchAgent/LaunchDaemon) inherits a minimal environment
//! — a bare `PATH` like `/usr/bin:/bin`, without the user's `~/.bun/bin`, Homebrew, or nvm dirs, and
//! sometimes without `HOME`. So a runner command like `~/.bun/bin/bun run start:dev` fails there
//! even though it works in a normal shell.
//!
//! This is the one place that compensates for that — the "env parity" every launcher applies so a
//! service resolves its tools the same way whoever starts it: adi-hive's supervisor
//! ([`crate`] consumer `adi-hive`) and adi-app's start endpoint (`adi-webapp-api`) both call
//! [`launch_env`]. Keep it here (not copied per crate) so the two never drift.

/// A `PATH` with the user's common tool directories prepended to the inherited one, so a runner
/// launched under launchd's bare environment can still find `bun`, `node`, Homebrew binaries, and
/// `docker`.
#[must_use]
pub fn augmented_path() -> String {
    let mut parts = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        parts.push(format!("{home}/.bun/bin"));
        parts.push(format!("{home}/.local/bin"));
    }
    parts.push("/opt/homebrew/bin".to_string());
    parts.push("/usr/local/bin".to_string());
    parts.push("/usr/bin".to_string());
    parts.push("/bin".to_string());
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

/// The environment variables a runner should be launched with: the augmented `PATH`, plus `HOME`
/// when this process has one (so `~` / `$HOME` in a runner command expand even under a bare launchd
/// env). Apply them to whatever `Command` type the caller uses (`std` or `tokio`):
///
/// ```no_run
/// let mut cmd = std::process::Command::new("sh");
/// for (key, value) in adi_config::launch_env() {
///     cmd.env(key, value);
/// }
/// ```
#[must_use]
pub fn launch_env() -> Vec<(&'static str, String)> {
    let mut env = vec![("PATH", augmented_path())];
    if let Ok(home) = std::env::var("HOME") {
        env.push(("HOME", home));
    }
    env
}
