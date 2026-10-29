//! Argument schemas for the built-in agent backends.
//!
//! Each struct reads only the keys its own runtime owns and *ignores* the rest. A v3 agent has one
//! `[arguments]` table but a backend chain that may span runtimes — a Codex row and a Claude row
//! reading the same table — so a key one runtime owns (`allowed_tools` for Claude,
//! `skip_git_repo_check` for Codex) must not abort another. That is why these are no longer
//! `deny_unknown_fields`; typos (a key no runtime owns) are caught at save by [`validate_builtin`]
//! instead, against the union of every runtime's schema.
//!
//! Third-party backends should define their own serializable argument struct and use it as the
//! type parameter in [`crate::AgentManifest`]. These built-in types exist so ADI's executors never
//! fetch operational settings through string keys.

use serde::{Deserialize, Deserializer, Serialize};

use crate::backend::Backend;
use crate::{Result as AgentResult, StoredAgentManifest};

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }

        impl $name {
            /// The exact value accepted by the underlying backend CLI.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }
    };
}

string_enum! {
    ClaudePermissionMode {
        AcceptEdits => "acceptEdits",
        Auto => "auto",
        BypassPermissions => "bypassPermissions",
        Manual => "manual",
        DontAsk => "dontAsk",
        Plan => "plan",
    }
}

string_enum! {
    ClaudeEffort {
        Low => "low",
        Medium => "medium",
        High => "high",
        ExtraHigh => "xhigh",
        Max => "max",
    }
}

string_enum! {
    /// Claude print-mode output encoding.
    ClaudeOutputFormat {
        Text => "text",
        Json => "json",
        StreamJson => "stream-json",
    }
}

string_enum! {
    /// Codex filesystem/command sandbox policy.
    CodexSandbox {
        ReadOnly => "read-only",
        WorkspaceWrite => "workspace-write",
        DangerFullAccess => "danger-full-access",
    }
}

string_enum! {
    /// Codex command approval policy.
    CodexApproval {
        Untrusted => "untrusted",
        OnRequest => "on-request",
        Never => "never",
    }
}

string_enum! {
    CodexReasoningEffort {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

string_enum! {
    /// The model provider the `harness:adi` loop calls; provider-specific knobs are keyed off it.
    HarnessProvider {
        Anthropic => "anthropic",
        Openai => "openai",
        Gemini => "gemini",
        Monshoot => "monshoot",
        Zai => "zai",
        Ollama => "ollama",
    }
}

string_enum! {
    /// Extended-thinking mode for the `harness:adi` loop: Anthropic's `thinking` modes, and z.ai's
    /// spelling of "turn it off".
    HarnessThinking {
        Adaptive => "adaptive",
        Disabled => "disabled",
    }
}

string_enum! {
    /// OpenAI/Monshoot structured-output mode for the `harness:adi` loop.
    HarnessResponseFormat {
        Text => "text",
        JsonObject => "json_object",
        JsonSchema => "json_schema",
    }
}

string_enum! {
    /// Ollama structured-output mode for the `harness:adi` loop.
    HarnessOllamaFormat {
        Json => "json",
    }
}

/// Validate an agent's arguments against the built-in runtimes.
///
/// One agent carries one `[arguments]` table, but its backend chain may span runtimes, and each
/// runtime reads only the keys it owns (the structs are no longer `deny_unknown_fields`). So a key
/// is legitimate when *any* built-in runtime recognises it — `allowed_tools` for a Claude row,
/// `skip_git_repo_check` for a Codex row — and a key no runtime owns is a typo. Each runtime's
/// deserialiser still type-checks the keys it does own, so a bad *value* is rejected by its owner.
///
/// A plugin/unknown *starting* runtime (a non-empty [`Backend::Other`]) owns its own argument type
/// and is validated when it converts, so it is left alone here. A v3 chained agent derives its
/// runtime and so reads as the *empty* `Other` — that one is validated against the union below.
pub(crate) fn validate_builtin(manifest: &StoredAgentManifest) -> AgentResult<()> {
    if matches!(manifest.runtime(), Backend::Other(id) if !id.is_empty()) {
        return Ok(());
    }
    let value = manifest.arguments_value();
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if object.is_empty() {
        return Ok(());
    }

    let runtimes = [
        Backend::PtyClaude,
        Backend::ProcessClaude,
        Backend::PtyCodex,
        Backend::ProcessCodex,
        Backend::HarnessClaudeSdk,
        Backend::HarnessAdi,
    ];
    let mut recognised = std::collections::BTreeSet::new();
    for runtime in &runtimes {
        recognised.extend(recognised_keys(runtime, &value)?);
    }
    for key in object.keys() {
        if !recognised.contains(key) {
            return Err(crate::Error::Arguments(format!(
                "`{key}` is not a recognised argument for any of this agent's runtimes"
            )));
        }
    }
    Ok(())
}

/// The keys of `value` that `runtime`'s typed arguments recognise.
///
/// Deserialising into the runtime's struct ignores keys it does not own (the structs are lenient)
/// and so reads only its own; re-serialising then reports which of those survived. Deserialising
/// also type-checks every key the runtime *does* own, so a bad value surfaces here as an error
/// rather than as a silent drop at run time.
fn recognised_keys(runtime: &Backend, value: &serde_json::Value) -> AgentResult<Vec<String>> {
    fn keys_of<T>(value: &serde_json::Value) -> AgentResult<Vec<String>>
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let typed: T = serde_json::from_value(value.clone())
            .map_err(|e| crate::Error::Arguments(e.to_string()))?;
        let back =
            serde_json::to_value(&typed).map_err(|e| crate::Error::Arguments(e.to_string()))?;
        Ok(back
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default())
    }
    match runtime {
        Backend::PtyClaude => keys_of::<PtyClaudeArguments>(value),
        Backend::ProcessClaude => keys_of::<ProcessClaudeArguments>(value),
        Backend::PtyCodex => keys_of::<PtyCodexArguments>(value),
        Backend::ProcessCodex => keys_of::<ProcessCodexArguments>(value),
        Backend::HarnessClaudeSdk => keys_of::<HarnessClaudeSdkArguments>(value),
        Backend::HarnessAdi => keys_of::<HarnessAdiArguments>(value),
        Backend::Other(_) => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PtyClaudeArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    /// The engine's built-in tools this agent may use — **the whole grant**. A tool it does not
    /// name is not merely un-approved, it is not there at all: the run is scoped down to this list
    /// (`--tools`) as well as pre-approved for it (`--allowed-tools`). Empty means the agent runs on
    /// ADI's own MCP tools alone. Scoped rules are welcome — `Edit(src/**)` grants `Edit` and
    /// permits exactly that scope. See `crate::backends::mcp::scope_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessClaudeArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<ClaudeOutputFormat>,
    /// The engine's built-in tools this agent may use — **the whole grant**. A tool it does not
    /// name is not merely un-approved, it is not there at all: the run is scoped down to this list
    /// (`--tools`) as well as pre-approved for it (`--allowed-tools`). Empty means the agent runs on
    /// ADI's own MCP tools alone. Scoped rules are welcome — `Edit(src/**)` grants `Edit` and
    /// permits exactly that scope. See `crate::backends::mcp::scope_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub max_budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PtyCodexArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<CodexSandbox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<CodexApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<CodexReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dir: Option<String>,
    #[serde(default, deserialize_with = "boolish")]
    pub web_search: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessCodexArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<CodexSandbox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<CodexApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<CodexReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dir: Option<String>,
    #[serde(default, deserialize_with = "boolish")]
    pub skip_git_repo_check: bool,
    #[serde(default, deserialize_with = "boolish")]
    pub web_search: bool,
    /// **Accepted and ignored**, and no longer offered on the agent form.
    ///
    /// `codex exec --json` is now unconditional: it is the only shape `crate::backends::codex_stream`
    /// can read, and turning it off meant the reader was handed the CLI's raw log — banner, prompt
    /// replay, tracing wall and all — as the agent's answer. Kept as a field, and only as a field,
    /// so a manifest written while it was a choice still loads under `deny_unknown_fields`.
    #[serde(default, deserialize_with = "boolish", skip_serializing)]
    pub json_events: bool,
}

/// Arguments accepted by the `harness:claude-sdk` backend: the `claude` CLI run headless by ADI's
/// harness. It shares the Claude engine knobs with the pty/process Claude backends, but adds a
/// harness turn cap (`max_turns`) and the adi-mono command scope (`tools`), and drops the
/// process-only options (`output_format`, `max_budget_usd`, `add_dir`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessClaudeSdkArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    /// The engine's built-in tools this agent may use — **the whole grant**. A tool it does not
    /// name is not merely un-approved, it is not there at all: the run is scoped down to this list
    /// (`--tools`) as well as pre-approved for it (`--allowed-tools`). Empty means the agent runs on
    /// ADI's own MCP tools alone. Scoped rules are welcome — `Edit(src/**)` grants `Edit` and
    /// permits exactly that scope. See `crate::backends::mcp::scope_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub max_turns: Option<u64>,
    /// The adi-mono command groups this agent may use (e.g. `tasks,projects`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    /// Where the run's process starts — the agent's declared home. Leaving it unset falls through
    /// to the agent's project directory, and then to the ADI store root, which is right for an agent
    /// that operates the environment and wrong for one that lives in a repo. Pointing a run at its
    /// workspace saves every command an absolute-path preamble, and saves the run the mistakes it
    /// makes when it forgets one. A launch may override this for one run; see
    /// `Agents::run_in` and `crate::workspace` for the whole precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// The engine's own settings, as a path to a JSON file or a JSON string — the CLI's
    /// `--settings`. It is what lets two agents on this one machine run the *same* engine against
    /// different accounts: a settings file carries an `env` block, so pointing one agent at
    /// `~/.claude/settings.glm.json` sends its turns to that file's `ANTHROPIC_BASE_URL` while
    /// every other agent keeps the default login, with no environment shared between them.
    ///
    /// The agent's `[env]` can say the same thing one variable at a time; this says it once, in the
    /// file the human already maintains for their own CLI. A leading `~`/`$HOME` is expanded here
    /// because the run is spawned without a shell to do it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
}

/// Arguments accepted by the `harness:adi` backend — ADI's own answering loop. `provider` selects
/// which model API the loop calls; the remaining fields are the union of every provider's knobs
/// (only the ones matching the chosen provider are ever set). Every provider is implemented, so an
/// agent is runnable as soon as it names one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessAdiArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<HarnessProvider>,
    /// The adi-mono command groups this agent may use (e.g. `tasks,projects`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub max_turns: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub max_tokens: Option<u64>,
    /// Comma-separated stop strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
    /// The environment variable the loop reads the provider API key from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// A provider endpoint override (e.g. a self-hosted or proxied base URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    // ---- Anthropic / z.ai ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<HarnessThinking>,

    // ---- OpenAI / Monshoot ----
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub frequency_penalty: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<HarnessResponseFormat>,

    // ---- Gemini ----
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub thinking_budget: Option<u64>,

    // ---- Ollama ----
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub num_ctx: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub repeat_penalty: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub min_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<String>,
    #[serde(default, deserialize_with = "boolish")]
    pub think: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<HarnessOllamaFormat>,

    // ---- Sampling (provider-scoped) ----
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub temperature: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_f64"
    )]
    pub top_p: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub top_k: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub seed: Option<u64>,
}

/// The small typed projection used by human-readable list output.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct AgentSummaryArguments {
    pub model: Option<String>,
    pub tools: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Boolish {
    Bool(bool),
    String(String),
}

fn boolish<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Boolish::deserialize(deserializer)?;
    match value {
        Boolish::Bool(value) => Ok(value),
        Boolish::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(serde::de::Error::custom("expected a boolean")),
        },
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum U64ish {
    Number(u64),
    // The web form sends every numeric field through `parse::<f64>()`, so an integer count like
    // `max_turns` arrives as a JSON float (`4096.0`). Accept an integral, non-negative float.
    Float(f64),
    String(String),
}

fn option_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<U64ish>::deserialize(deserializer)? {
        None => Ok(None),
        Some(U64ish::Number(value)) => Ok(Some(value)),
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the guard admits only whole, non-negative floats; a value past u64::MAX \
                      saturates, which is a cap nothing here is anywhere near"
        )]
        Some(U64ish::Float(value)) if value.fract() == 0.0 && value >= 0.0 => {
            Ok(Some(value as u64))
        }
        Some(U64ish::Float(_)) => Err(serde::de::Error::custom("expected a non-negative integer")),
        Some(U64ish::String(value)) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum F64ish {
    Number(f64),
    String(String),
}

fn option_f64<'de, D>(deserializer: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<F64ish>::deserialize(deserializer)? {
        None => Ok(None),
        Some(F64ish::Number(value)) => Ok(Some(value)),
        Some(F64ish::String(value)) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web form encodes every numeric field with `parse::<f64>()`, so integer counts reach the
    /// store as JSON floats. Decoding them into the harness `u64` fields must not reject them.
    #[test]
    fn harness_adi_accepts_integral_floats_for_u64_knobs() {
        let arguments: HarnessAdiArguments = serde_json::from_value(serde_json::json!({
            "provider": "openai",
            "max_tokens": 4096.0,
            "seed": 42.0,
            "top_k": 40.0,
            "top_p": 0.9,
            "max_turns": 12.0,
        }))
        .expect("float-encoded numeric knobs decode");
        assert_eq!(arguments.provider, Some(HarnessProvider::Openai));
        assert_eq!(arguments.max_tokens, Some(4096));
        assert_eq!(arguments.seed, Some(42));
        assert_eq!(arguments.top_k, Some(40));
        assert_eq!(arguments.max_turns, Some(12));
        assert!((arguments.top_p.expect("top_p") - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn harness_u64_rejects_fractional_floats() {
        let err = serde_json::from_value::<HarnessAdiArguments>(serde_json::json!({
            "max_tokens": 4096.5,
        }))
        .expect_err("a fractional token count is not an integer");
        assert!(err.to_string().contains("non-negative integer"), "{err}");
    }

    /// Run-time decode is lenient: a key one runtime owns is ignored by another, so one shared
    /// `[arguments]` table can feed a chain that spans runtimes. Typos are caught at save by
    /// `validate_builtin`, not here.
    #[test]
    fn runtime_decode_ignores_other_runtimes_arguments() {
        // The Claude SDK struct ignores Codex-only keys...
        let claude: HarnessClaudeSdkArguments = serde_json::from_value(serde_json::json!({
            "system_prompt": "hi",
            "allowed_tools": "Read Edit",
            "skip_git_repo_check": true,
            "sandbox": "read-only",
        }))
        .expect("the Claude SDK struct ignores Codex-only keys");
        assert_eq!(claude.allowed_tools.as_deref(), Some("Read Edit"));

        // ...and the Codex struct ignores Claude-only keys.
        let codex: ProcessCodexArguments = serde_json::from_value(serde_json::json!({
            "system_prompt": "hi",
            "allowed_tools": "Read Edit",
            "skip_git_repo_check": true,
        }))
        .expect("the Codex struct ignores Claude-only keys");
        assert!(codex.skip_git_repo_check);
        assert_eq!(codex.system_prompt.as_deref(), Some("hi"));
    }

    fn manifest_v3(arguments: serde_json::Value) -> StoredAgentManifest {
        StoredAgentManifest {
            // No `backend` line: a v3 chained agent derives its runtime, reading as the empty
            // `Other`, which `validate_builtin` checks against the union of the built-in schemas.
            backend: None,
            arguments: arguments
                .as_object()
                .expect("object")
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..Default::default()
        }
    }

    /// The whole point of the fix: a chained agent carrying one row's Claude key and another row's
    /// Codex key validates, because each is owned by *some* built-in runtime.
    #[test]
    fn validate_builtin_accepts_each_rows_arguments() {
        let agent = manifest_v3(serde_json::json!({
            "system_prompt": "hi",
            "allowed_tools": "Read Edit",
            "skip_git_repo_check": true,
        }));
        validate_builtin(&agent).expect("cross-runtime arguments are accepted");
    }

    /// A key no runtime owns is still a typo, named in the error.
    #[test]
    fn validate_builtin_rejects_a_typo() {
        let agent = manifest_v3(serde_json::json!({
            "system_prompt": "hi",
            "max_truns": 4,
        }));
        let err = validate_builtin(&agent).expect_err("an unrecognised key is rejected");
        assert!(err.to_string().contains("max_truns"), "{err}");
    }

    /// A bad *value* for an owned key is rejected by its owning runtime, even across the union.
    #[test]
    fn validate_builtin_rejects_a_bad_value_for_an_owned_key() {
        let agent = manifest_v3(serde_json::json!({ "max_turns": "lots" }));
        assert!(
            validate_builtin(&agent).is_err(),
            "a non-numeric max_turns is rejected"
        );
    }

    #[test]
    fn harness_claude_sdk_round_trips_its_engine_knobs() {
        let arguments = HarnessClaudeSdkArguments {
            model: Some("claude-opus-4-8".into()),
            permission_mode: Some(ClaudePermissionMode::Plan),
            effort: Some(ClaudeEffort::High),
            max_turns: Some(20),
            tools: Some("tasks,projects".into()),
            ..HarnessClaudeSdkArguments::default()
        };
        let value = serde_json::to_value(&arguments).expect("serialize");
        let decoded: HarnessClaudeSdkArguments = serde_json::from_value(value).expect("round-trip");
        assert_eq!(decoded, arguments);
    }
}
