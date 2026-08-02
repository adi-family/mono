//! `harness:adi` — ADI's own agentic loop over a chosen model provider.
//!
//! Unlike `harness:claude-sdk`, there is no vendor CLI: each conversation turn spawns
//! `adi-mono harness-turn --agent <name> --conv <id>`, which runs [`run_turn`]. That reads the
//! conversation's committed transcript, calls the configured provider's chat API with the whole
//! history, and writes the turn's events to stdout — which the detached machinery captures and
//! [`super::conversation`] folds into the transcript, exactly like a Claude turn. So continuation
//! here is transcript replay rather than a resumable session id.
//!
//! It speaks every provider the manifest can name: **Anthropic**'s Messages API, **`OpenAI`** and
//! **Monshoot** (Moonshot's Kimi) over the shared chat-completions dialect, **Gemini**'s
//! `generateContent`, and a local **Ollama**. The only thing [`validate`] rejects is an agent that
//! has not picked one.
//!
//! **A turn is a loop, not a call.** The model is offered the [`tools`](super::tools) every coding
//! agent has — `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep` — and while it asks for them, this
//! runs them and asks again, up to [`MAX_ROUNDS`]. The four providers disagree about how tools are
//! declared, how a call comes back, and how a result is handed over; [`Wire`] is where those four
//! disagreements live, so the loop itself is written once and reads the same for all of them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::StoredAgent;
use crate::arguments::{
    HarnessAdiArguments, HarnessProvider, HarnessResponseFormat, HarnessThinking,
};
use crate::backends::adi_events::{self, Sink};
use crate::error::{Error, Result};
use crate::progress::TurnMetrics;

use crate::store::Turn;
use super::tools;

/// Anthropic requires an explicit output cap, so default one when the agent sets none.
const DEFAULT_MAX_TOKENS: u64 = 4096;
/// Kimi's models think before they answer, and every reasoning token comes out of the same output
/// budget as the reply — a 4k cap routinely ends the turn mid-thought with an empty answer. So the
/// Monshoot default is roomier; an agent that sets `max_tokens` still wins.
const MONSHOOT_DEFAULT_MAX_TOKENS: u64 = 16_384;
/// A generous per-round ceiling — a local model can be slow, and a round is one blocking call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(600);
/// How many model round-trips one turn may take before the loop gives up. High enough for real
/// work (read a few files, edit, check), low enough that a model stuck in a call/retry rut costs
/// a bounded amount of money and time. An agent's `max_turns` overrides it.
const MAX_ROUNDS: u64 = 16;

/// The command a conversation turn spawns for an `adi` agent: re-enter this binary's hidden
/// `harness-turn` subcommand, which reads the transcript and calls the provider.
pub(crate) fn argv(agent_name: &str, conv_id: &str) -> Vec<String> {
    vec![
        "adi-mono".to_string(),
        "harness-turn".to_string(),
        "--agent".to_string(),
        agent_name.to_string(),
        "--conv".to_string(),
        conv_id.to_string(),
    ]
}

/// Whether the loop can actually run these arguments. Every provider the manifest can name is
/// implemented, so the only thing left to reject is an agent that hasn't picked one — which is
/// not-yet-configured rather than broken, hence `NotRunnable` and a hidden run button.
pub(crate) fn validate(args: &HarnessAdiArguments) -> Result<()> {
    match args.provider {
        None => Err(Error::NotRunnable("harness:adi".to_string())),
        Some(_) => Ok(()),
    }
}

/// Run one turn to completion, writing its events to `sink` as they happen and returning the
/// answer. Called from the spawned `adi-mono harness-turn` child (a plain sync process — the
/// blocking HTTP client must not run inside an async runtime).
pub(crate) fn run_turn(
    agent: &StoredAgent,
    sessions_dir: &Path,
    conv_id: &str,
    sink: Sink<'_>,
) -> Result<String> {
    let mut args = agent.manifest.typed_arguments::<HarnessAdiArguments>()?;
    validate(&args)?;
    // The runner composes this run's system prompt — the agent's own instructions with its tools'
    // help behind them — and exports it, because this loop has no command-line flag to carry one.
    // It wins over the stored arguments; a turn spawned by anything that exported nothing falls
    // back to them, which is exactly what happened before the runner existed.
    if let Some(prompt) = composed_prompt() {
        args.system_prompt = Some(prompt);
    }
    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            Error::Unsupported("the adi loop needs a model — set one on the agent".to_string())
        })?;

    // The committed transcript ends with the user turn this reply answers (the agent layer appended
    // it before spawning us). Read straight from the session store: the turn child and the process
    // that launched it are different processes sharing one directory, so the store *is* the channel.
    let turns = crate::store::SessionStore::new(sessions_dir).turns(&agent.name, conv_id);
    if turns.iter().all(|t| t.text.trim().is_empty()) {
        return Err(Error::Process(
            "the conversation has no messages to answer".to_string(),
        ));
    }

    // The child was spawned into the run's own directory, so this is the directory the agent's
    // work is about — the one its relative paths resolve against.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // `Awaits::open()` here rather than deeper down: this child shares the app's store through the
    // same `$ADI_DIR` every other process here does, and holds no `Agents` of its own.
    let ctx = tools::Ctx {
        cwd: &cwd,
        agent: &agent.name,
        conv: conv_id,
        awaits: crate::awaits::Awaits::open(),
    };
    let wire = Wire::of(&args, model)?;
    tool_loop(&wire, &args, &turns, &ctx, sink)
}

// ---- the loop ----------------------------------------------------------------------

/// One tool call the model asked for.
struct ToolCall {
    /// The provider's own id where it has one; a synthesized `call-N` where it doesn't (Gemini and
    /// Ollama identify a call only by position, so the loop supplies what the transcript needs).
    id: String,
    name: String,
    input: Value,
}

/// What running one call produced. `ok` false is a tool that failed, which is an answer the model
/// is expected to read and act on — not a failure of the turn.
struct ToolResult {
    call_id: String,
    name: String,
    output: String,
    ok: bool,
}

/// One round-trip's outcome, in provider-neutral terms.
struct Reply {
    /// What the model wrote this round: commentary when calls follow, the answer when none do.
    text: String,
    calls: Vec<ToolCall>,
    /// The assistant message exactly as the provider sent it. Echoed back verbatim on the next
    /// round, because every one of these APIs wants its own message shape returned unaltered.
    raw: Value,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// Ask, run what was asked for, and ask again — until the model answers without calling anything.
fn tool_loop(
    wire: &Wire<'_>,
    args: &HarnessAdiArguments,
    turns: &[Turn],
    ctx: &tools::Ctx<'_>,
    sink: Sink<'_>,
) -> Result<String> {
    let max_rounds = args.max_turns.filter(|n| *n > 0).unwrap_or(MAX_ROUNDS);
    let mut messages = wire.seed(turns);
    let mut metrics = TurnMetrics::default();

    for round in 1..=max_rounds {
        let reply = wire.round(&messages)?;
        metrics.num_turns = Some(round);
        add(&mut metrics.input_tokens, reply.input_tokens);
        add(&mut metrics.output_tokens, reply.output_tokens);

        if reply.calls.is_empty() {
            if reply.text.trim().is_empty() {
                metrics.is_error = true;
                adi_events::metrics(sink, &metrics);
                return Err(Error::Process(format!(
                    "{} answered with neither text nor a tool call",
                    wire.provider()
                )));
            }
            adi_events::answer(sink, &reply.text);
            adi_events::metrics(sink, &metrics);
            return Ok(reply.text);
        }

        // Anything said before the calls is commentary — it belongs on the timeline, not in the
        // answer, which is whatever the model writes once it stops reaching for tools.
        adi_events::message(sink, &reply.text);

        let mut results = Vec::with_capacity(reply.calls.len());
        for call in &reply.calls {
            adi_events::tool_started(sink, &call.id, &call.name, &call.input);
            let (output, ok) = match tools::run(&call.name, &call.input, ctx) {
                Ok(out) => (out, true),
                Err(err) => (err, false),
            };
            adi_events::tool_finished(sink, &call.id, &call.name, &output, ok);
            results.push(ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                output,
                ok,
            });
        }
        wire.append(&mut messages, &reply, &results);
    }

    metrics.is_error = true;
    adi_events::metrics(sink, &metrics);
    Err(Error::Process(format!(
        "the turn was still calling tools after {max_rounds} rounds and was stopped — the steps \
         above are what it did; raise the agent's max turns if the work genuinely needs more"
    )))
}

fn add(total: &mut Option<u64>, more: Option<u64>) {
    if let Some(n) = more {
        *total = Some(total.unwrap_or(0) + n);
    }
}

// ---- the four wire formats ---------------------------------------------------------

/// A provider's tool-calling dialect: how the conversation starts, how one round is sent and read,
/// and how a round plus its tool results is appended for the next one.
enum Wire<'a> {
    Anthropic {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
    /// `OpenAI` and Monshoot: the same chat-completions shape, differing only in [`OpenAiDialect`].
    OpenAi {
        args: &'a HarnessAdiArguments,
        model: &'a str,
        dialect: &'static OpenAiDialect,
    },
    Gemini {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
    Ollama {
        args: &'a HarnessAdiArguments,
        model: &'a str,
    },
}

impl<'a> Wire<'a> {
    fn of(args: &'a HarnessAdiArguments, model: &'a str) -> Result<Self> {
        Ok(match args.provider {
            Some(HarnessProvider::Anthropic) => Self::Anthropic { args, model },
            Some(HarnessProvider::Openai) => Self::OpenAi {
                args,
                model,
                dialect: &OPENAI,
            },
            Some(HarnessProvider::Monshoot) => Self::OpenAi {
                args,
                model,
                dialect: &MONSHOOT,
            },
            Some(HarnessProvider::Gemini) => Self::Gemini { args, model },
            Some(HarnessProvider::Ollama) => Self::Ollama { args, model },
            None => return Err(Error::NotRunnable("harness:adi".to_string())),
        })
    }

    fn provider(&self) -> &'static str {
        match self {
            Self::Anthropic { .. } => "anthropic",
            Self::OpenAi { dialect, .. } => dialect.provider,
            Self::Gemini { .. } => "gemini",
            Self::Ollama { .. } => "ollama",
        }
    }

    /// The transcript as this provider's opening message list.
    fn seed(&self, turns: &[Turn]) -> Vec<Value> {
        let plain: Vec<&Turn> = turns
            .iter()
            .filter(|t| !t.text.trim().is_empty())
            .collect();
        match self {
            // Gemini names the assistant `model` and carries the system prompt outside the list.
            Self::Gemini { .. } => plain
                .iter()
                .map(|t| {
                    let role = if t.role == "assistant" { "model" } else { "user" };
                    json!({ "role": role, "parts": [{ "text": t.text }] })
                })
                .collect(),
            // Everyone else takes `{role, content}` with the system prompt as the first message —
            // except Anthropic, which has a `system` field; passing it as a message is accepted on
            // current models, but the dedicated field is what the API documents, so it goes there.
            Self::Anthropic { .. } => plain
                .iter()
                .map(|t| json!({ "role": t.role, "content": t.text }))
                .collect(),
            Self::OpenAi { args, .. } | Self::Ollama { args, .. } => {
                let mut messages: Vec<Value> = plain
                    .iter()
                    .map(|t| json!({ "role": t.role, "content": t.text }))
                    .collect();
                if let Some(system) = system_prompt(args) {
                    messages.insert(0, json!({ "role": "system", "content": system }));
                }
                messages
            }
        }
    }

    /// Send one round and read it back.
    fn round(&self, messages: &[Value]) -> Result<Reply> {
        match self {
            Self::Anthropic { args, model } => anthropic_round(args, model, messages),
            Self::OpenAi {
                args,
                model,
                dialect,
            } => openai_round(args, model, messages, dialect),
            Self::Gemini { args, model } => gemini_round(args, model, messages),
            Self::Ollama { args, model } => ollama_round(args, model, messages),
        }
    }

    /// Append the assistant's round and one result per call, in this provider's shape.
    fn append(&self, messages: &mut Vec<Value>, reply: &Reply, results: &[ToolResult]) {
        messages.push(reply.raw.clone());
        match self {
            Self::Anthropic { .. } => {
                // One user message carrying every result, which is the shape the API requires:
                // a tool_result block per tool_use, all in the turn that answers them.
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": r.call_id,
                            "content": r.output,
                            "is_error": !r.ok,
                        })
                    })
                    .collect();
                messages.push(json!({ "role": "user", "content": blocks }));
            }
            Self::OpenAi { .. } => messages.extend(results.iter().map(|r| {
                json!({ "role": "tool", "tool_call_id": r.call_id, "content": r.output })
            })),
            // Ollama identifies a result by the tool's name rather than a call id.
            Self::Ollama { .. } => messages.extend(
                results
                    .iter()
                    .map(|r| json!({ "role": "tool", "tool_name": r.name, "content": r.output })),
            ),
            // Gemini answers a call with a functionResponse part, all of them in one user turn.
            Self::Gemini { .. } => {
                let parts: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "functionResponse": {
                                "name": r.name,
                                "response": { "output": r.output, "ok": r.ok },
                            }
                        })
                    })
                    .collect();
                messages.push(json!({ "role": "user", "parts": parts }));
            }
        }
    }
}

/// The tool set as JSON Schema function declarations — the shape `OpenAI`, Ollama and (nested one
/// level deeper) Gemini all take.
fn function_declarations() -> Vec<Value> {
    tools::TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": (t.schema)(),
            })
        })
        .collect()
}

// ---- Anthropic ---------------------------------------------------------------------

fn anthropic_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
) -> Result<Reply> {
    let key = api_key(args, "ANTHROPIC_API_KEY", "Anthropic")?;

    let mut body = json!({
        "model": model,
        "max_tokens": args.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": messages,
        // Anthropic keeps the schema under `input_schema` rather than `parameters`.
        "tools": tools::TOOLS.iter().map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": (t.schema)(),
        })).collect::<Vec<_>>(),
    });
    if let Some(system) = system_prompt(args) {
        body["system"] = json!(system);
    }
    if let Some(t) = args.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = args.top_p {
        body["top_p"] = json!(p);
    }
    if let Some(k) = args.top_k {
        body["top_k"] = json!(k);
    }
    if let Some(stops) = stop_sequences(args) {
        body["stop_sequences"] = json!(stops);
    }
    // Extended thinking is a mode, not a token budget: current models take `adaptive` (Claude
    // decides how much to think) or `disabled`, and reject the `budget_tokens` the older API
    // wanted. Left unset, the model's own default stands.
    if let Some(thinking) = args.thinking {
        let mode = match thinking {
            HarnessThinking::Adaptive => "adaptive",
            HarnessThinking::Disabled => "disabled",
        };
        body["thinking"] = json!({ "type": mode });
    }

    let base = base_url(args, "https://api.anthropic.com");
    let url = versioned_url(&base, "v1", "messages");
    let headers = [
        ("x-api-key", key.as_str()),
        ("anthropic-version", "2023-06-01"),
    ];
    let resp = post_json(&url, &headers, &body)?;

    // The reply is a list of content blocks: text ones make up what it said, tool_use ones are what
    // it wants run. Both can appear in the same round, which is exactly the "here's what I'm about
    // to do" narration the timeline wants to keep.
    let blocks = resp
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| provider_shape_error("anthropic", &resp))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => calls.push(ToolCall {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: block.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {}
        }
    }
    if text.trim().is_empty()
        && calls.is_empty()
        && resp.get("stop_reason").and_then(Value::as_str) == Some("max_tokens")
    {
        return Err(out_of_budget_error(
            model,
            args.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        ));
    }
    Ok(Reply {
        text,
        calls,
        raw: json!({ "role": "assistant", "content": blocks }),
        input_tokens: usage(&resp, &["usage", "input_tokens"]),
        output_tokens: usage(&resp, &["usage", "output_tokens"]),
    })
}

// ---- OpenAI dialect (OpenAI, and Monshoot's Kimi) ----------------------------------

/// The two providers that speak `OpenAI`'s `/v1/chat/completions`. They agree on the whole request
/// body but disagree on where they live, which variable holds the key, and — the one that bites —
/// what the output cap is called.
struct OpenAiDialect {
    /// Name used in error messages; also the manifest's `provider` value.
    provider: &'static str,
    default_base: &'static str,
    default_key_env: &'static str,
    /// `OpenAI`'s reasoning models **reject** `max_tokens` and want `max_completion_tokens`;
    /// Moonshot only knows `max_tokens`. An `OpenAI`-compatible third party that predates the
    /// rename is reachable through the `monshoot` provider with a `base_url` override.
    max_tokens_field: &'static str,
    /// The cap to send when the agent sets none — see [`MONSHOOT_DEFAULT_MAX_TOKENS`].
    default_max_tokens: u64,
}

const OPENAI: OpenAiDialect = OpenAiDialect {
    provider: "openai",
    default_base: "https://api.openai.com",
    default_key_env: "OPENAI_API_KEY",
    max_tokens_field: "max_completion_tokens",
    default_max_tokens: MONSHOOT_DEFAULT_MAX_TOKENS,
};

const MONSHOOT: OpenAiDialect = OpenAiDialect {
    provider: "monshoot",
    default_base: "https://api.moonshot.ai",
    default_key_env: "MOONSHOT_API_KEY",
    max_tokens_field: "max_tokens",
    default_max_tokens: MONSHOOT_DEFAULT_MAX_TOKENS,
};

/// One round against an `OpenAI`-dialect chat-completions endpoint.
///
/// Two things about the reasoning models on both providers are worth knowing before reading the
/// parse below. They think first, and the scratchpad comes back *beside* the answer (`reasoning`
/// on `OpenAI`, `reasoning_content` on Kimi) while `content` holds the reply — so a round that runs
/// out of budget mid-thought returns an **empty** `content` with `finish_reason: "length"`. That
/// case gets its own error, because "raise max output tokens" is the fix and nothing else says so.
/// And several of them (`kimi-k2.6`, `OpenAI`'s o-series and gpt-5) accept only the default
/// temperature, which is why nothing is sent unless the agent asked for it explicitly.
fn openai_round(
    args: &HarnessAdiArguments,
    model: &str,
    messages: &[Value],
    dialect: &OpenAiDialect,
) -> Result<Reply> {
    let key = api_key(args, dialect.default_key_env, dialect.provider)?;

    let max_tokens = args.max_tokens.unwrap_or(dialect.default_max_tokens);
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "tools": function_declarations()
            .into_iter()
            .map(|f| json!({ "type": "function", "function": f }))
            .collect::<Vec<_>>(),
    });
    body[dialect.max_tokens_field] = json!(max_tokens);
    if let Some(t) = args.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = args.top_p {
        body["top_p"] = json!(p);
    }
    if let Some(f) = args.frequency_penalty {
        body["frequency_penalty"] = json!(f);
    }
    if let Some(p) = args.presence_penalty {
        body["presence_penalty"] = json!(p);
    }
    if let Some(seed) = args.seed {
        body["seed"] = json!(seed);
    }
    if let Some(format) = args.response_format {
        body["response_format"] = json!({ "type": response_format_kind(format)? });
    }
    if let Some(stops) = stop_sequences(args) {
        body["stop"] = json!(stops);
    }

    let base = base_url(args, dialect.default_base);
    let url = versioned_url(&base, "v1", "chat/completions");
    let bearer = format!("Bearer {key}");
    let resp = post_json(&url, &[("authorization", bearer.as_str())], &body)?;

    let choice = resp
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| provider_shape_error(dialect.provider, &resp))?;
    let message = choice
        .get("message")
        .ok_or_else(|| provider_shape_error(dialect.provider, &resp))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `arguments` is a JSON *string* here, not an object — the one place this dialect makes the
    // caller parse a second time.
    let calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .map_or_else(|| format!("call-{i}"), str::to_string),
                        name: f.get("name").and_then(Value::as_str)?.to_string(),
                        input: parse_arguments(f.get("arguments")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if text.trim().is_empty()
        && calls.is_empty()
        && choice.get("finish_reason").and_then(Value::as_str) == Some("length")
    {
        return Err(out_of_budget_error(model, max_tokens));
    }
    Ok(Reply {
        text,
        calls,
        raw: message.clone(),
        input_tokens: usage(&resp, &["usage", "prompt_tokens"]),
        output_tokens: usage(&resp, &["usage", "completion_tokens"]),
    })
}

// ---- Gemini ------------------------------------------------------------------------

/// Google's `generateContent` — the one provider here that isn't a chat-completions clone. Its
/// differences, all visible below: the assistant role is called `model`, the system prompt is a
/// `systemInstruction` of its own, every sampling knob lives under `generationConfig`, the model
/// name is part of the URL rather than the body, tool declarations nest one level deeper, and a
/// 2.5-series reply interleaves *thought* parts with answer parts in one list — so the read keeps
/// only the parts that aren't thoughts.
fn gemini_round(args: &HarnessAdiArguments, model: &str, messages: &[Value]) -> Result<Reply> {
    let key = api_key(args, "GEMINI_API_KEY", "Gemini")?;

    let mut config = serde_json::Map::new();
    put_f64(&mut config, "temperature", args.temperature);
    put_f64(&mut config, "topP", args.top_p);
    put_u64(&mut config, "topK", args.top_k);
    put_u64(&mut config, "maxOutputTokens", args.max_tokens);
    if let Some(stops) = stop_sequences(args) {
        config.insert("stopSequences".to_string(), json!(stops));
    }
    if let Some(budget) = args.thinking_budget {
        config.insert(
            "thinkingConfig".to_string(),
            json!({ "thinkingBudget": budget }),
        );
    }

    let mut body = json!({
        "contents": messages,
        "tools": [{ "functionDeclarations": function_declarations() }],
    });
    if let Some(system) = system_prompt(args) {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if !config.is_empty() {
        body["generationConfig"] = Value::Object(config);
    }

    let base = base_url(args, "https://generativelanguage.googleapis.com");
    let url = versioned_url(&base, "v1beta", &format!("models/{model}:generateContent"));
    // Two credential shapes reach this endpoint and they go in different headers: a plain API key
    // (`AIza…`) as `x-goog-api-key`, an OAuth access token (`ya29.…`) as a bearer. Pick by the
    // token's own prefix, so either kind of secret can be attached to the agent and just work.
    let bearer;
    let header = if key.starts_with("ya29.") {
        bearer = format!("Bearer {key}");
        ("authorization", bearer.as_str())
    } else {
        ("x-goog-api-key", key.as_str())
    };
    let resp = post_json(&url, &[header], &body)?;

    let candidate = resp
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| provider_shape_error("gemini", &resp))?;
    let parts = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
        // A function call here carries no id of its own — position is its only identity, so the
        // loop supplies one for the transcript and answers by name.
        if let Some(fc) = part.get("functionCall") {
            calls.push(ToolCall {
                id: format!("call-{i}"),
                name: fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: fc.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        }
    }

    if text.trim().is_empty() && calls.is_empty() {
        // The two ways an answer goes missing: the budget went entirely on thinking, or the reply
        // was stopped (safety, recitation). Both name the reason Google gave, which is the whole
        // diagnosis.
        return match candidate.get("finishReason").and_then(Value::as_str) {
            Some("MAX_TOKENS") => Err(out_of_budget_error(
                model,
                args.max_tokens.unwrap_or_default(),
            )),
            Some(reason) if reason != "STOP" => Err(Error::Process(format!(
                "gemini stopped before writing an answer: {reason}"
            ))),
            _ => Err(provider_shape_error("gemini", &resp)),
        };
    }
    Ok(Reply {
        text,
        calls,
        raw: json!({ "role": "model", "parts": parts }),
        input_tokens: usage(&resp, &["usageMetadata", "promptTokenCount"]),
        output_tokens: usage(&resp, &["usageMetadata", "candidatesTokenCount"]),
    })
}

// ---- Ollama (local) ----------------------------------------------------------------

fn ollama_round(args: &HarnessAdiArguments, model: &str, messages: &[Value]) -> Result<Reply> {
    let mut options = serde_json::Map::new();
    put_f64(&mut options, "temperature", args.temperature);
    put_f64(&mut options, "top_p", args.top_p);
    put_u64(&mut options, "top_k", args.top_k);
    put_u64(&mut options, "num_ctx", args.num_ctx);
    put_f64(&mut options, "repeat_penalty", args.repeat_penalty);
    put_f64(&mut options, "min_p", args.min_p);
    put_u64(&mut options, "num_predict", args.max_tokens);
    if let Some(seed) = args.seed {
        options.insert("seed".to_string(), json!(seed));
    }
    if let Some(stops) = stop_sequences(args) {
        options.insert("stop".to_string(), json!(stops));
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "tools": function_declarations()
            .into_iter()
            .map(|f| json!({ "type": "function", "function": f }))
            .collect::<Vec<_>>(),
    });
    if !options.is_empty() {
        body["options"] = Value::Object(options);
    }
    if args.format.is_some() {
        body["format"] = json!("json");
    }
    if args.think {
        // Only sent when asked for: a model that can't think rejects the field outright.
        body["think"] = json!(true);
    }
    if let Some(keep) = args.keep_alive.as_deref().filter(|k| !k.trim().is_empty()) {
        body["keep_alive"] = json!(keep);
    }

    let base = base_url(args, "http://localhost:11434");
    let url = format!("{base}/api/chat");
    let resp = post_json(&url, &[], &body)?;

    let message = resp
        .get("message")
        .ok_or_else(|| provider_shape_error("ollama", &resp))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Ollama returns already-parsed arguments (an object, not a string) and no call id.
    let calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let f = c.get("function")?;
                    Some(ToolCall {
                        id: format!("call-{i}"),
                        name: f.get("name").and_then(Value::as_str)?.to_string(),
                        input: parse_arguments(f.get("arguments")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if text.trim().is_empty() && calls.is_empty() {
        return Err(provider_shape_error("ollama", &resp));
    }
    Ok(Reply {
        text,
        calls,
        raw: message.clone(),
        input_tokens: usage(&resp, &["prompt_eval_count"]),
        output_tokens: usage(&resp, &["eval_count"]),
    })
}

// ---- shared HTTP + argument helpers ------------------------------------------------

/// Tool arguments as an object, whichever way the provider sent them: an object already (Ollama,
/// Gemini) or a JSON string to decode (`OpenAI`, Monshoot). A model that emits malformed JSON gets
/// an empty object and, a moment later, the tool's own complaint about the missing argument —
/// which is a better teacher than a parse error it never sees.
fn parse_arguments(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        Some(other) => other.clone(),
        None => json!({}),
    }
}

/// A usage counter from a response, by path.
fn usage(resp: &Value, path: &[&str]) -> Option<u64> {
    let mut node = resp;
    for key in path {
        node = node.get(key)?;
    }
    node.as_u64()
}

/// POST `body` as JSON with the given extra headers, returning the decoded JSON response. A non-2xx
/// status surfaces the provider's own error body, which is what the caller needs to see.
fn post_json(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| Error::Process(format!("couldn't build HTTP client: {e}")))?;
    let mut req = client.post(url).json(body);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = req
        .send()
        .map_err(|e| Error::Process(format!("request to {url} failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| Error::Process(format!("reading response from {url} failed: {e}")))?;
    if !status.is_success() {
        return Err(Error::Process(format!(
            "{url} returned {status}: {}",
            text.trim()
        )));
    }
    serde_json::from_str(&text)
        .map_err(|e| Error::Process(format!("invalid JSON from {url}: {e}")))
}

fn provider_shape_error(provider: &str, resp: &Value) -> Error {
    Error::Process(format!(
        "{provider} response had no answer text: {}",
        resp.to_string().chars().take(300).collect::<String>()
    ))
}

/// A reply that is all reasoning and no answer. Every thinking model can produce one, and the fix
/// is always the same, so they all say it the same way.
fn out_of_budget_error(model: &str, max_tokens: u64) -> Error {
    Error::Process(format!(
        "{model} spent its whole {max_tokens} token budget thinking and never wrote an answer — \
         raise the agent's max output tokens"
    ))
}

/// The provider's API key, read from the environment variable the agent named (or the provider's
/// conventional one). A missing key is a setup problem rather than a run failure — hence
/// `Unsupported`, and hence the pointer to where the key belongs.
fn api_key(args: &HarnessAdiArguments, default_env: &str, provider: &str) -> Result<String> {
    let key_env = args
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .unwrap_or(default_env);
    std::env::var(key_env).map_err(|_| {
        Error::Unsupported(format!(
            "no {provider} API key: environment variable {key_env} is unset (attach it as a secret on the agent)"
        ))
    })
}

/// The provider's endpoint: the agent's `base_url` override, or the provider's own host.
fn base_url(args: &HarnessAdiArguments, default: &str) -> String {
    args.base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/')
        .to_string()
}

/// `<base>/<version>/<path>`, tolerating a base that already ends in the version segment — the
/// panel's own hint for this field reads `https://api.moonshot.ai/v1`, and pasting exactly that
/// must not produce `/v1/v1/chat/completions`.
fn versioned_url(base: &str, version: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(&format!("/{version}")) {
        format!("{base}/{path}")
    } else {
        format!("{base}/{version}/{path}")
    }
}

/// The wire value for a structured-output mode. `json_schema` needs the schema itself, which this
/// argument set has nowhere to carry — so say that, rather than sending a request the provider
/// would reject for us.
fn response_format_kind(format: HarnessResponseFormat) -> Result<&'static str> {
    match format {
        HarnessResponseFormat::Text => Ok("text"),
        HarnessResponseFormat::JsonObject => Ok("json_object"),
        HarnessResponseFormat::JsonSchema => Err(Error::Unsupported(
            "the adi loop can't send a json_schema response format — it has no schema to send; \
             use json_object"
                .to_string(),
        )),
    }
}

/// The system prompt, trimmed to a non-empty value, or `None`.
fn system_prompt(args: &HarnessAdiArguments) -> Option<String> {
    args.system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The prompt the runner composed for this turn and handed down in the environment, if it did.
fn composed_prompt() -> Option<String> {
    std::env::var(crate::runner::detached::SYSTEM_PROMPT_ENV)
        .ok()
        .map(|prompt| prompt.trim().to_string())
        .filter(|prompt| !prompt.is_empty())
}

/// The comma-separated `stop` argument split into a non-empty list of stop strings.
fn stop_sequences(args: &HarnessAdiArguments) -> Option<Vec<String>> {
    let stops: Vec<String> = args
        .stop
        .as_deref()?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!stops.is_empty()).then_some(stops)
}

fn put_f64(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(v) = value {
        map.insert(key.to_string(), json!(v));
    }
}

fn put_u64(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(v) = value {
        map.insert(key.to_string(), json!(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for(provider: HarnessProvider) -> HarnessAdiArguments {
        HarnessAdiArguments {
            provider: Some(provider),
            ..HarnessAdiArguments::default()
        }
    }

    fn turn(role: &str, text: &str) -> Turn {
        Turn {
            role: role.into(),
            text: text.into(),
            at: 1,
            pending: false,
            queued: false,
            steps: Vec::new(),
            metrics: None,
        }
    }

    #[test]
    fn every_provider_runs_and_only_an_unconfigured_agent_does_not() {
        let mut args = HarnessAdiArguments::default();
        // No provider → not-yet-configured, which reads as not runnable.
        assert!(matches!(validate(&args), Err(Error::NotRunnable(b)) if b == "harness:adi"));
        for provider in [
            HarnessProvider::Anthropic,
            HarnessProvider::Openai,
            HarnessProvider::Gemini,
            HarnessProvider::Monshoot,
            HarnessProvider::Ollama,
        ] {
            args.provider = Some(provider);
            assert!(
                validate(&args).is_ok(),
                "{} must be runnable",
                provider.as_str()
            );
        }
    }

    #[test]
    fn a_base_url_that_already_carries_its_version_is_not_doubled() {
        // The panel hints the endpoint with its version segment, so both spellings must land on
        // the same URL.
        assert_eq!(
            versioned_url("https://api.moonshot.ai", "v1", "chat/completions"),
            "https://api.moonshot.ai/v1/chat/completions"
        );
        assert_eq!(
            versioned_url("https://api.moonshot.ai/v1/", "v1", "chat/completions"),
            "https://api.moonshot.ai/v1/chat/completions"
        );
        assert_eq!(
            versioned_url(
                "https://generativelanguage.googleapis.com",
                "v1beta",
                "models/g:x"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/g:x"
        );
    }

    #[test]
    fn the_two_openai_dialects_differ_only_where_the_providers_do() {
        // The rename is the whole reason the dialect struct exists: sending Moonshot's field name
        // to an OpenAI reasoning model is a 400, and vice versa.
        assert_eq!(OPENAI.max_tokens_field, "max_completion_tokens");
        assert_eq!(MONSHOOT.max_tokens_field, "max_tokens");
    }

    #[test]
    fn gemini_seeds_the_assistant_turn_under_its_own_role_name() {
        let args = args_for(HarnessProvider::Gemini);
        let wire = Wire::of(&args, "gemini-2.5-pro").expect("wire");
        let seeded = wire.seed(&[turn("user", "hi"), turn("assistant", "hello")]);
        assert_eq!(seeded[0]["role"], "user");
        assert_eq!(seeded[1]["role"], "model");
        assert_eq!(seeded[1]["parts"][0]["text"], "hello");
    }

    #[test]
    fn blank_turns_are_dropped_and_a_system_prompt_leads_the_chat_dialects() {
        let mut args = args_for(HarnessProvider::Monshoot);
        args.system_prompt = Some("be terse".into());
        let wire = Wire::of(&args, "kimi-k3").expect("wire");
        let seeded = wire.seed(&[turn("user", "hi"), turn("assistant", "  "), turn("user", "again")]);
        assert_eq!(seeded.len(), 3, "the blank turn is dropped: {seeded:?}");
        assert_eq!(seeded[0]["role"], "system");
        assert_eq!(seeded[2]["content"], "again");
    }

    #[test]
    fn each_provider_answers_a_call_the_way_its_api_expects() {
        let reply = |calls: Vec<ToolCall>| Reply {
            text: String::new(),
            calls,
            raw: json!({ "role": "assistant" }),
            input_tokens: None,
            output_tokens: None,
        };
        let call = || ToolCall {
            id: "c1".into(),
            name: "Read".into(),
            input: json!({}),
        };
        let results = [ToolResult {
            call_id: "c1".into(),
            name: "Read".into(),
            output: "contents".into(),
            ok: true,
        }];

        // Anthropic: every result in one user message, each block naming the tool_use it answers.
        let args = args_for(HarnessProvider::Anthropic);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "tool_result");
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "c1");

        // OpenAI dialect: one `tool` message per call, keyed by call id.
        let args = args_for(HarnessProvider::Openai);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "c1");

        // Gemini: a functionResponse part, answered by name because a call has no id.
        let args = args_for(HarnessProvider::Gemini);
        let mut messages = Vec::new();
        Wire::of(&args, "m")
            .expect("wire")
            .append(&mut messages, &reply(vec![call()]), &results);
        assert_eq!(messages[1]["parts"][0]["functionResponse"]["name"], "Read");
    }

    #[test]
    fn tool_arguments_are_read_whichever_way_the_provider_sends_them() {
        // OpenAI and Monshoot send a JSON string; Ollama and Gemini send the object itself.
        assert_eq!(
            parse_arguments(Some(&json!("{\"path\":\"a.rs\"}")))["path"],
            "a.rs"
        );
        assert_eq!(parse_arguments(Some(&json!({"path": "a.rs"})))["path"], "a.rs");
        // Malformed JSON degrades to an empty object, so the tool's own error teaches the model.
        assert_eq!(parse_arguments(Some(&json!("{not json"))), json!({}));
        assert_eq!(parse_arguments(None), json!({}));
    }

    #[test]
    fn argv_reenters_this_binary_for_the_turn() {
        assert_eq!(
            argv("planner", "0000000000001-0000"),
            [
                "adi-mono",
                "harness-turn",
                "--agent",
                "planner",
                "--conv",
                "0000000000001-0000",
            ]
        );
    }
}
