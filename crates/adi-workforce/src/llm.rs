use crate::plugin::PluginError;
use crate::tool_def::ToolDef;

// --- Conversation model ---
//
// A conversation is a flat `Vec<Turn>` where each Turn is either a
// `User` turn or an `Assistant` turn. Each turn holds an ordered list
// of typed blocks. This mirrors the Anthropic Messages API wire format
// 1:1 and preserves interleaved thinking order / multiplicity.
//
// Why Vec<Block> and not named fields (thinking: ..., text: ..., tool_calls: ...):
//   - Anthropic's `interleaved-thinking-2025-05-14` beta lets the model
//     produce blocks like `thinking → tool_use → thinking → tool_use`
//     within a single turn. Ordering matters because signatures are
//     scoped to a specific position in the content array.
//   - Multiple text blocks and multiple thinking blocks per turn are
//     both legal. A Vec handles them naturally; named fields don't.

/// A single conversation turn. Either a user turn or an assistant turn.
/// Rust enum makes illegal states ("both set" / "neither set") impossible.
#[derive(Debug, Clone)]
pub enum Turn {
    User(UserTurn),
    Assistant(AssistantTurn),
}

impl Turn {
    /// Convenience: build a user turn containing a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Turn::User(UserTurn {
            blocks: vec![UserBlock::Text(text.into())],
        })
    }

    /// Convenience: build an empty assistant turn you can push blocks into.
    pub fn assistant_empty() -> Self {
        Turn::Assistant(AssistantTurn { blocks: Vec::new() })
    }
}

#[derive(Debug, Clone)]
pub struct UserTurn {
    /// Ordered content blocks for this user turn.
    pub blocks: Vec<UserBlock>,
}

#[derive(Debug, Clone)]
pub struct AssistantTurn {
    /// Ordered content blocks produced by the assistant in this turn.
    /// Order is load-bearing for interleaved thinking.
    pub blocks: Vec<AssistantBlock>,
}

/// Content that can appear in a user-role turn.
///
/// Note: tool results are user-role content in Anthropic's wire format.
/// The system relays tool results back as user messages; they do not
/// originate from the human but they ride on the same turn structure.
#[derive(Debug, Clone)]
pub enum UserBlock {
    /// Plain text from the human.
    Text(String),
    /// Result of a prior `AssistantBlock::ToolUse`. The `tool_use_id`
    /// must match the id the assistant emitted so the provider can
    /// pair them up.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Content that can appear in an assistant-role turn.
#[derive(Debug, Clone)]
pub enum AssistantBlock {
    /// Private reasoning produced by the model.
    ///
    /// `signature` is an opaque provider-specific verification hash.
    /// For Anthropic, it's required when echoing the block back on a
    /// subsequent tool-use turn — the server validates it to detect
    /// tampering. We preserve it verbatim and never inspect it.
    ///
    /// `redacted == true` indicates the provider stripped the reasoning
    /// for safety reasons (Anthropic's "redacted_thinking" block type);
    /// the text may be empty or opaque data.
    Thinking {
        text: String,
        signature: Option<String>,
        redacted: bool,
    },
    /// User-facing text output.
    Text(String),
    /// A tool invocation. The result arrives in the next user turn as
    /// a `UserBlock::ToolResult` referencing this `id`.
    ToolUse {
        id: String,
        name: String,
        /// Tool arguments as a JSON string. Kept as a string (not a
        /// parsed Value) so loggers can round-trip it verbatim.
        arguments: String,
    },
}

// --- LLM request/response ---

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system_prompt: String,
    pub turns: Vec<Turn>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: usize,
}

/// Response from an LLM backend for a single `call()`.
///
/// `turn` holds the model's new assistant turn with its ordered blocks.
/// The loop runner appends it directly to its `Vec<Turn>` conversation
/// state. No separate text/tool_calls/thinking fields — they all live
/// inside `turn.blocks` in the correct order.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub turn: AssistantTurn,
    /// Model id the response was produced by (e.g. "claude-opus-4-7").
    /// Backends that don't identify the model leave this empty.
    pub model: String,
    /// Non-cached input tokens billed at full price.
    pub input_tokens: usize,
    pub output_tokens: usize,
    /// Tokens written to the prompt cache on this request (billed at ~1.25x input rate).
    pub cache_creation_input_tokens: usize,
    /// Tokens read from the prompt cache on this request (billed at ~0.1x input rate).
    pub cache_read_input_tokens: usize,
}

impl LlmResponse {
    /// Convenience: all text blocks in the assistant turn, concatenated.
    /// Empty string if the turn has no text (e.g. pure tool_use turn).
    pub fn text(&self) -> String {
        self.turn
            .blocks
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Convenience: iterator over tool_use blocks in this turn.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.turn.blocks.iter().filter_map(|b| match b {
            AssistantBlock::ToolUse {
                id,
                name,
                arguments,
            } => Some((id.as_str(), name.as_str(), arguments.as_str())),
            _ => None,
        })
    }
}

pub trait LlmBackend: Send + Sync {
    fn call(&self, request: &LlmRequest) -> Result<LlmResponse, PluginError>;
}
