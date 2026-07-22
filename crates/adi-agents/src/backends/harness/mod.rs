//! The `harness` executor: an agentic loop ADI drives itself, rather than a vendor CLI that owns
//! its own loop.
//!
//! A harness run is a *conversation* you can answer, not a fire-and-forget `--print` run: the first
//! turn is the initial task, and each reply spawns another detached child that continues the same
//! thread and prints its answer. The conversation machinery (transcript, turn spawning, settling)
//! lives in [`conversation`]; each engine only supplies the per-turn command and how it continues.
//!
//! - `harness:claude-sdk` runs the `claude` CLI headless (a turn-capped, adi-scoped `--print` turn),
//!   establishing a session id on the first turn (`--session-id`) and resuming it on each reply
//!   (`--resume`) so the CLI reconstructs the full history. Spawned detached through the shared
//!   [`super::detached`] machinery, under its own `harness/` runtime subdir.
//! - `harness:adi` is ADI's own loop over a chosen model provider (see [`adi_loop`]): each turn
//!   re-enters `adi-mono harness-turn`, replays the transcript, and calls the provider's chat API.
//!   Runnable once a supported provider (Anthropic, or a local Ollama) is configured.

mod adi_loop;
mod claude_sdk;
mod conversation;

use std::path::{Path, PathBuf};

use crate::arguments::{HarnessAdiArguments, HarnessClaudeSdkArguments};
use crate::backend::Backend;
use crate::backends::detached;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

use conversation::Continuation;
pub use conversation::Turn;

const HARNESS_DIR: &str = "harness";

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    engine_supported(manifest).is_ok()
}

/// Run one `adi` conversation turn: read the transcript, call the provider, and return the answer
/// text. Used by the `adi-mono harness-turn` child that an `adi` turn spawns — a plain sync process,
/// since the blocking provider client must not run inside an async runtime.
///
/// # Errors
/// Returns argument, provider-configuration, or HTTP/decoding errors.
pub fn run_adi_turn(agent: &StoredAgent, sessions_dir: &Path, conv_id: &str) -> Result<String> {
    adi_loop::run_turn(agent, sessions_dir, conv_id)
}

/// Start a new conversation with `message` as its first turn. The returned launch's `run_id` is the
/// conversation id — pass it back to [`reply`] to answer into the same thread.
pub fn launch(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    message: &str,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    conversation::start(agent, sessions_dir, base_dir, bin_dir, message, secret_env)
}

/// Answer into an existing conversation, spawning the next turn. Rejected while the previous answer
/// is still being produced.
pub fn reply(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    conv_id: &str,
    message: &str,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    conversation::reply(
        agent,
        sessions_dir,
        base_dir,
        bin_dir,
        conv_id,
        message,
        secret_env,
    )
}

/// A conversation's transcript (oldest first), including the still-streaming answer — with its parsed
/// tool/thinking steps — while a turn is in flight. `backend` selects how the turn log is parsed.
#[must_use]
pub fn transcript(
    sessions_dir: &Path,
    agent_name: &str,
    conv_id: &str,
    backend: &Backend,
) -> Vec<Turn> {
    conversation::transcript(sessions_dir, agent_name, conv_id, backend)
}

/// This agent's run history, newest first.
#[must_use]
pub fn list_runs(sessions_dir: &Path, agent_name: &str) -> Vec<crate::run::RunInfo> {
    detached::list_runs(sessions_dir, HARNESS_DIR, agent_name)
}

/// Whether any run of this agent is still alive.
#[must_use]
pub fn any_running(sessions_dir: &Path, agent_name: &str) -> bool {
    detached::any_running(sessions_dir, HARNESS_DIR, agent_name)
}

/// Whether one specific run is still alive.
#[must_use]
pub fn is_running(sessions_dir: &Path, agent_name: &str, run_id: &str) -> bool {
    detached::is_running(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

/// Stop one specific run.
pub fn stop(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Result<bool> {
    detached::stop(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

/// The tail of one run's log, for the live view.
#[must_use]
pub fn tail_log(sessions_dir: &Path, agent_name: &str, run_id: &str) -> Option<String> {
    detached::tail_log(
        sessions_dir,
        HARNESS_DIR,
        agent_name,
        run_id,
        crate::run::MAX_LOG_TAIL,
    )
}

/// The log path of one run — the `tail -f` target the live view shows.
#[must_use]
pub fn log_path(sessions_dir: &Path, agent_name: &str, run_id: &str) -> PathBuf {
    detached::log_path(sessions_dir, HARNESS_DIR, agent_name, run_id)
}

/// Whether this manifest's engine can run — the check behind [`is_runnable`], and the fail-fast gate
/// a conversation runs before touching disk. Parses the typed arguments and, for `adi`, checks a
/// supported provider is configured.
fn engine_supported(manifest: &StoredAgentManifest) -> Result<()> {
    match &manifest.backend {
        Backend::HarnessClaudeSdk => manifest
            .typed_arguments::<HarnessClaudeSdkArguments>()
            .map(drop),
        Backend::HarnessAdi => adi_loop::validate(&manifest.typed_arguments::<HarnessAdiArguments>()?),
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

/// Build one conversation turn's command for the agent's engine: the argv and its optional working
/// dir. `cont` carries whether this is the establishing first turn or a resumed reply (claude-sdk);
/// the `adi` loop instead replays the transcript, so it is keyed by the agent + conversation id.
fn engine_turn(
    agent: &StoredAgent,
    conv_id: &str,
    message: &str,
    cont: Continuation<'_>,
) -> Result<(Vec<String>, Option<String>)> {
    match &agent.manifest.backend {
        Backend::HarnessClaudeSdk => {
            let arguments = agent.manifest.typed_arguments::<HarnessClaudeSdkArguments>()?;
            Ok((claude_sdk::argv(&arguments, message, &cont), None))
        }
        Backend::HarnessAdi => {
            adi_loop::validate(&agent.manifest.typed_arguments::<HarnessAdiArguments>()?)?;
            Ok((adi_loop::argv(&agent.name, conv_id), None))
        }
        other => Err(Error::NotRunnable(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(backend: &str) -> StoredAgentManifest {
        StoredAgentManifest {
            backend: backend.into(),
            ..StoredAgentManifest::default()
        }
    }

    fn first() -> Continuation<'static> {
        Continuation::First { session_id: "sid" }
    }

    fn agent(backend: &str) -> StoredAgent {
        StoredAgent {
            name: "chatty".into(),
            manifest: manifest(backend),
        }
    }

    #[test]
    fn claude_sdk_is_runnable_and_builds_a_command() {
        let agent = agent("harness:claude-sdk");
        assert!(is_runnable(&agent.manifest));
        let (argv, working_dir) =
            engine_turn(&agent, "conv-1", "go", first()).expect("engine_turn");
        assert_eq!(argv.first().map(String::as_str), Some("claude"));
        assert!(argv.iter().any(|a| a == "--print"));
        // The first turn establishes the session id it will resume on replies.
        assert!(argv.iter().any(|a| a == "--session-id"));
        assert!(working_dir.is_none());
    }

    #[test]
    fn adi_without_a_provider_is_not_runnable() {
        // With no provider configured, `adi` reads as not-yet-set-up (not runnable).
        let agent = agent("harness:adi");
        assert!(!is_runnable(&agent.manifest));
        assert!(matches!(
            engine_turn(&agent, "conv-1", "go", first()),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
    }

    #[test]
    fn adi_with_a_supported_provider_reenters_this_binary() {
        let mut agent = agent("harness:adi");
        agent.manifest.arguments = serde_json::json!({ "provider": "ollama", "model": "qwen3.6" })
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .collect();
        assert!(is_runnable(&agent.manifest));
        let (argv, _) = engine_turn(&agent, "conv-9", "hi", first()).expect("engine_turn");
        assert_eq!(argv.first().map(String::as_str), Some("adi-mono"));
        assert!(argv.iter().any(|a| a == "harness-turn"));
        assert!(argv.iter().any(|a| a == "conv-9"));
    }

    #[test]
    fn unknown_harness_engines_are_not_runnable() {
        assert!(matches!(
            engine_turn(&agent("harness:unknown"), "conv-1", "go", first()),
            Err(Error::NotRunnable(_))
        ));
    }
}
