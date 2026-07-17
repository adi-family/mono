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
    String(String),
}

fn option_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<U64ish>::deserialize(deserializer)? {
        None => Ok(None),
        Some(U64ish::Number(value)) => Ok(Some(value)),
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
