//! Everything a run needs, already resolved and already on disk.
//!
//! The agent layer does the *work* — resolving the working directory, syncing the agent's `.bin` of
//! enabled tools, assembling `PATH` and the environment — and hands the result down. A runner
//! resolves nothing, creates no directories, and syncs no tools: by the time it is called, the cwd
//! exists, the shims are written, and `PATH` already names them.

use std::path::PathBuf;

use adi_tools::ToolHelp;

/// One run's materialized context.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSpec {
    /// Where the run starts. Resolved **once, at session creation**, and re-used verbatim for every
    /// later turn — an engine's session store is keyed by the directory it ran in, so re-resolving
    /// mid-conversation makes the session unresumable and puts the files earlier turns wrote out of
    /// reach.
    pub cwd: PathBuf,
    /// The assembled `PATH`: the agent's own `.bin` first, then the dirs its manifest declared, then
    /// the standard ones a launchd-minimal environment misses (see [`crate::launch::run_path`]).
    pub path: String,
    /// The run's environment: attached secrets, its scope's database pointer, the workspace vars,
    /// and the agent's own declared entries — already merged, in precedence order.
    pub env: Vec<(String, String)>,
    /// The engine's own configuration, exactly as stored. Uninterpreted here: each runner
    /// deserializes its own typed argument struct out of this in [`check`](super::Runner::check),
    /// which is why no layer above needs a table of every backend's argument type.
    pub arguments: serde_json::Value,
    /// The enabled tools, as **data**.
    ///
    /// Not folded into a prompt here. Where tool help belongs — an appended system prompt, a system
    /// message, or nowhere at all for an engine whose "system prompt" is really an opening user
    /// turn — is a fact about the engine, so the runner composes it.
    pub tools: Vec<ToolHelp>,
    /// The tool section this conversation was opened with, already rendered — use this instead of
    /// re-rendering [`tools`](Self::tools) when it is set.
    ///
    /// Deriving the section fresh is right at *launch* and wrong on every turn after it. Each tool
    /// is asked to describe itself under a shared time budget, and one that isn't cached when the
    /// budget runs out is listed by name alone and fully described on the next turn — so the same
    /// conversation's system prompt changed between turns for no reason the model could see, and
    /// every cached token after it was thrown away and paid for again. Touching a tool's script
    /// did the same thing. Freezing what the conversation opened with keeps the stated bargain
    /// exactly: edit a tool and the *next run* sees it, not the turn you are in the middle of.
    pub tool_help: Option<String>,
    /// The agent's own system prompt, unmodified. Nothing above the runner writes into it.
    pub system_prompt: Option<String>,
    /// Where the run starts, stated in prose for the prompt — `None` when there is nothing to say.
    ///
    /// Rendered above (the wording is generic, and it needs the store's config to name the project),
    /// but folded in by the runner, on the same rule as [`tools`](Self::tools): an engine whose
    /// "system prompt" is really an opening *user* turn must not be handed it.
    ///
    /// It has to reach the prompt somehow. A run that is only *told* its directory through the
    /// environment gets it right until the first command that forgets to look, and then quietly
    /// writes somewhere else — which is the failure `workspace` exists to end.
    pub workspace_note: Option<String>,
}
