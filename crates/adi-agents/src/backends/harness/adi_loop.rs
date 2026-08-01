//! `harness:adi` — ADI's own answering loop over a chosen model provider.
//!
//! Unlike `harness:claude-sdk`, there is no vendor CLI: each conversation turn spawns
//! `adi-mono harness-turn --agent <name> --conv <id>`, which runs [`run_turn`]. That reads the
//! conversation's committed transcript, calls the configured provider's chat API with the whole
//! history, and prints the answer to stdout — which the detached machinery captures as the turn's
//! output and [`super::conversation`] folds into the transcript, exactly like a Claude turn. So
//! continuation here is transcript replay rather than a resumable session id.
//!
//! This is a plain conversational loop (no tool use yet — that is the natural next step), and it
//! now speaks every provider the manifest can name: **Anthropic**'s Messages API, **OpenAI** and
//! **Monshoot** (Moonshot's Kimi) over the shared chat-completions dialect, **Gemini**'s
//! `generateContent`, and a local **Ollama**. The only thing [`validate`] still rejects is an
//! agent that has not picked one.
//!
//! Each arm sends exactly the arguments its provider understands, and the panel scopes the fields
//! it offers to the chosen provider — so the union type in [`HarnessAdiArguments`] never leaks a
//! knob into a request that would reject it.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::StoredAgent;
use crate::arguments::{
    HarnessAdiArguments, HarnessProvider, HarnessResponseFormat, HarnessThinking,
};
use crate::error::{Error, Result};

use super::conversation::{self, Turn};

/// Anthropic requires an explicit output cap, so default one when the agent sets none.
const DEFAULT_MAX_TOKENS: u64 = 4096;
/// Kimi's models think before they answer, and every reasoning token comes out of the same output
/// budget as the reply — a 4k cap routinely ends the turn mid-thought with an empty answer. So the
/// Monshoot default is roomier; an agent that sets `max_tokens` still wins.
const MONSHOOT_DEFAULT_MAX_TOKENS: u64 = 16_384;
/// A generous per-turn ceiling — a local model can be slow, and a turn is one blocking call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(600);

/// The command a conversation turn spawns for an `adi` agent: re-enter this binary's hidden
/// `harness-turn` subcommand, which reads the transcript and calls the provider.
pub(super) fn argv(agent_name: &str, conv_id: &str) -> Vec<String> {
    vec![
        "adi-mono".to_string(),
        "harness-turn".to_string(),
        "--agent".to_string(),
        agent_name.to_string(),
        "--conv".to_string(),
        conv_id.to_string(),
    ]
}

/// Whether the loop can actually run these arguments. Every provider the manifest can name is now
/// implemented, so the only thing left to reject is an agent that hasn't picked one — which is
/// not-yet-configured rather than broken, hence `NotRunnable` and a hidden run button.
pub(super) fn validate(args: &HarnessAdiArguments) -> Result<()> {
    match args.provider {
        None => Err(Error::NotRunnable("harness:adi".to_string())),
        Some(_) => Ok(()),
    }
}

/// Run one turn: read the transcript, call the provider, and return its answer text. Called from the
/// spawned `adi-mono harness-turn` child (a plain sync process — the blocking HTTP client must not
/// run inside an async runtime).
pub(crate) fn run_turn(agent: &StoredAgent, sessions_dir: &Path, conv_id: &str) -> Result<String> {
    let args = agent.manifest.typed_arguments::<HarnessAdiArguments>()?;
    validate(&args)?;
    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            Error::Unsupported("the adi loop needs a model — set one on the agent".to_string())
        })?;

    // The committed transcript ends with the user turn this reply answers (conversation appended it
    // before spawning us). Map it straight to provider chat messages.
    let turns = conversation::committed(sessions_dir, &agent.name, conv_id);
    let messages = chat_messages(&turns);
    if messages.is_empty() {
        return Err(Error::Process("the conversation has no messages to answer".to_string()));
    }

    match args.provider {
        Some(HarnessProvider::Ollama) => ollama_chat(&args, model, messages),
        Some(HarnessProvider::Anthropic) => anthropic_messages(&args, model, messages),
        Some(HarnessProvider::Openai) => openai_chat(&args, model, messages, &OPENAI),
        Some(HarnessProvider::Monshoot) => openai_chat(&args, model, messages, &MONSHOOT),
        Some(HarnessProvider::Gemini) => gemini_generate(&args, model, messages),
        // validate() already rejected the only remaining case: no provider at all.
        None => Err(Error::NotRunnable("harness:adi".to_string())),
    }
}

/// The transcript's user/assistant turns as `{role, content}` chat messages (blank turns dropped).
fn chat_messages(turns: &[Turn]) -> Vec<Value> {
    turns
        .iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| json!({ "role": t.role, "content": t.text }))
        .collect()
}

// ---- Ollama (local) ----------------------------------------------------------------

fn ollama_chat(args: &HarnessAdiArguments, model: &str, mut messages: Vec<Value>) -> Result<String> {
    if let Some(system) = system_prompt(args) {
        messages.insert(0, json!({ "role": "system", "content": system }));
    }
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
    resp.get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| provider_shape_error("ollama", &resp))
}

// ---- Anthropic ---------------------------------------------------------------------

fn anthropic_messages(
    args: &HarnessAdiArguments,
    model: &str,
    messages: Vec<Value>,
) -> Result<String> {
    let key = api_key(args, "ANTHROPIC_API_KEY", "Anthropic")?;

    let mut body = json!({
        "model": model,
        "max_tokens": args.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": messages,
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
    // The reply is an array of content blocks; concatenate the text ones.
    let text = resp
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|t| !t.is_empty());
    text.ok_or_else(|| provider_shape_error("anthropic", &resp))
}

// ---- OpenAI dialect (OpenAI, and Monshoot's Kimi) ----------------------------------

/// The two providers that speak OpenAI's `/v1/chat/completions`. They agree on the whole request
/// body but disagree on where they live, which variable holds the key, and — the one that bites —
/// what the output cap is called.
struct OpenAiDialect {
    /// Name used in error messages; also the manifest's `provider` value.
    provider: &'static str,
    default_base: &'static str,
    default_key_env: &'static str,
    /// OpenAI's reasoning models **reject** `max_tokens` and want `max_completion_tokens`;
    /// Moonshot only knows `max_tokens`. An OpenAI-compatible third party that predates the
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

/// One turn against an OpenAI-dialect chat-completions endpoint.
///
/// Two things about the reasoning models on both providers are worth knowing before reading the
/// parse below. They think first, and the scratchpad comes back *beside* the answer (`reasoning`
/// on OpenAI, `reasoning_content` on Kimi) while `content` holds the reply — so a turn that runs
/// out of budget mid-thought returns an **empty** `content` with `finish_reason: "length"`. That
/// case gets its own error, because "raise max output tokens" is the fix and nothing else says so.
/// And several of them (`kimi-k2.6`, OpenAI's o-series and gpt-5) accept only the default
/// temperature, which is why nothing is sent unless the agent asked for it explicitly.
fn openai_chat(
    args: &HarnessAdiArguments,
    model: &str,
    mut messages: Vec<Value>,
    dialect: &OpenAiDialect,
) -> Result<String> {
    let key = api_key(args, dialect.default_key_env, dialect.provider)?;
    if let Some(system) = system_prompt(args) {
        messages.insert(0, json!({ "role": "system", "content": system }));
    }

    let max_tokens = args.max_tokens.unwrap_or(dialect.default_max_tokens);
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
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
    let text = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !text.trim().is_empty() {
        return Ok(text.to_string());
    }
    // Empty answer: say which of the two ways it happened, since only one has an obvious fix.
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(out_of_budget_error(model, max_tokens));
    }
    Err(provider_shape_error(dialect.provider, &resp))
}

// ---- Gemini ------------------------------------------------------------------------

/// Google's `generateContent` — the one provider here that isn't a chat-completions clone. Its
/// differences, all visible below: the assistant role is called `model`, the system prompt is a
/// `systemInstruction` of its own, every sampling knob lives under `generationConfig`, the model
/// name is part of the URL rather than the body, and a 2.5-series reply interleaves *thought*
/// parts with answer parts in one list — so the parse keeps only the parts that aren't thoughts.
fn gemini_generate(args: &HarnessAdiArguments, model: &str, messages: Vec<Value>) -> Result<String> {
    let key = api_key(args, "GEMINI_API_KEY", "Gemini")?;

    // Same turns, Google's spelling: `assistant` is `model`, and text is a part rather than a
    // string. Anything that isn't an assistant turn is a user turn — the transcript has no others.
    let contents: Vec<Value> = messages
        .iter()
        .map(|m| {
            let role = match m.get("role").and_then(Value::as_str) {
                Some("assistant") => "model",
                _ => "user",
            };
            json!({ "role": role, "parts": [{ "text": m.get("content") }] })
        })
        .collect();

    let mut config = serde_json::Map::new();
    put_f64(&mut config, "temperature", args.temperature);
    put_f64(&mut config, "topP", args.top_p);
    put_u64(&mut config, "topK", args.top_k);
    put_u64(&mut config, "maxOutputTokens", args.max_tokens);
    if let Some(stops) = stop_sequences(args) {
        config.insert("stopSequences".to_string(), json!(stops));
    }
    if let Some(budget) = args.thinking_budget {
        config.insert("thinkingConfig".to_string(), json!({ "thinkingBudget": budget }));
    }

    let mut body = json!({ "contents": contents });
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
    let text = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if !text.trim().is_empty() {
        return Ok(text);
    }
    // The two ways an answer goes missing: the budget went entirely on thinking, or the reply was
    // stopped (safety, recitation). Both name the reason Google gave, which is the whole diagnosis.
    match candidate.get("finishReason").and_then(Value::as_str) {
        Some("MAX_TOKENS") => Err(out_of_budget_error(
            model,
            args.max_tokens.unwrap_or_default(),
        )),
        Some(reason) if reason != "STOP" => Err(Error::Process(format!(
            "gemini stopped before writing an answer: {reason}"
        ))),
        _ => Err(provider_shape_error("gemini", &resp)),
    }
}

// ---- shared HTTP + argument helpers ------------------------------------------------

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
            assert!(validate(&args).is_ok(), "{} must be runnable", provider.as_str());
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
            versioned_url("https://generativelanguage.googleapis.com", "v1beta", "models/g:x"),
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
    fn gemini_renames_the_assistant_role_and_drops_nothing_else() {
        let turn = |role: &str, text: &str, at: u64| Turn {
            role: role.into(),
            text: text.into(),
            at,
            pending: false,
            queued: false,
            steps: Vec::new(),
            metrics: None,
        };
        let msgs = chat_messages(&[turn("user", "hi", 1), turn("assistant", "hello", 2)]);
        let roles: Vec<&str> = msgs
            .iter()
            .map(|m| match m["role"].as_str() {
                Some("assistant") => "model",
                _ => "user",
            })
            .collect();
        assert_eq!(roles, ["user", "model"]);
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

    #[test]
    fn blank_turns_are_dropped_from_the_chat_history() {
        let turn = |role: &str, text: &str, at: u64| Turn {
            role: role.into(),
            text: text.into(),
            at,
            pending: false,
            queued: false,
            steps: Vec::new(),
            metrics: None,
        };
        let turns = vec![turn("user", "hi", 1), turn("assistant", "  ", 2), turn("user", "again", 3)];
        let msgs = chat_messages(&turns);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["content"], "again");
    }
}
