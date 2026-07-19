//! Strict argument schemas for the built-in agent backends.
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
    /// Claude's permission handling mode.
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
    /// Claude's reasoning effort.
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
    /// Codex model reasoning effort.
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
        Ollama => "ollama",
    }
}

string_enum! {
    /// Anthropic extended-thinking mode for the `harness:adi` loop.
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

/// Validate arguments for every backend whose executor ships with this crate. Unknown/plugin
/// backends own their argument type and are validated when they convert or call
/// [`crate::Agents::get_typed`].
pub(crate) fn validate_builtin(manifest: &StoredAgentManifest) -> AgentResult<()> {
    match &manifest.backend {
        Backend::TmuxClaude => manifest.typed_arguments::<TmuxClaudeArguments>().map(drop),
        Backend::ProcessClaude => manifest
            .typed_arguments::<ProcessClaudeArguments>()
            .map(drop),
        Backend::TmuxCodex => manifest.typed_arguments::<TmuxCodexArguments>().map(drop),
        Backend::ProcessCodex => manifest
            .typed_arguments::<ProcessCodexArguments>()
            .map(drop),
        Backend::HarnessClaudeSdk => manifest
            .typed_arguments::<HarnessClaudeSdkArguments>()
            .map(drop),
        Backend::HarnessAdi => manifest.typed_arguments::<HarnessAdiArguments>().map(drop),
        Backend::Wasm => manifest.typed_arguments::<WasmArguments>().map(drop),
        Backend::Other(_) => Ok(()),
    }
}

/// Arguments accepted by the interactive `tmux:claude` backend.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxClaudeArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_dir: Option<String>,
}

/// Arguments accepted by the headless `process:claude` backend.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<String>,
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

/// Arguments accepted by the interactive `tmux:codex` backend.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxCodexArguments {
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

/// Arguments accepted by the headless `process:codex` backend.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    #[serde(default, deserialize_with = "boolish")]
    pub json_events: bool,
}

/// Arguments accepted by the `harness:claude-sdk` backend: the `claude` CLI run headless by ADI's
/// harness. It shares the Claude engine knobs with the tmux/process Claude backends, but adds a
/// harness turn cap (`max_turns`) and the adi-mono command scope (`tools`), and drops the
/// process-only options (`output_format`, `max_budget_usd`, `add_dir`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HarnessClaudeSdkArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<String>,
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
}

/// Arguments accepted by the `harness:adi` backend — ADI's own agentic loop. `provider` selects
/// which model API the loop calls; the remaining fields are the union of every provider's knobs
/// (only the ones matching the chosen provider are ever set). Typed and stored today, but the loop
/// engine that would run it does not exist yet, so the backend is not runnable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

    // ---- Anthropic ----
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

/// Arguments accepted by `wasm:loop-script` agents.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WasmArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_u64"
    )]
    pub max_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
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
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

    #[test]
    fn harness_backends_reject_unknown_fields() {
        // `deny_unknown_fields` is what turns a mis-scoped or misspelled knob into a save error.
        assert!(
            serde_json::from_value::<HarnessClaudeSdkArguments>(serde_json::json!({
                "temperature": 0.2,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<HarnessAdiArguments>(serde_json::json!({
                "output_format": "json",
            }))
            .is_err()
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
