//! `harness:adi` — ADI's own answering loop over a chosen model provider.
//!
//! Unlike `harness:claude-sdk`, there is no vendor CLI: each conversation turn spawns
//! `adi-mono harness-turn --agent <name> --conv <id>`, which runs [`run_turn`]. That reads the
//! conversation's committed transcript, calls the configured provider's chat API with the whole
//! history, and prints the answer to stdout — which the detached machinery captures as the turn's
//! output and [`super::conversation`] folds into the transcript, exactly like a Claude turn. So
//! continuation here is transcript replay rather than a resumable session id.
//!
//! This first version is a plain conversational loop (no tool use yet — that is the natural next
//! step). Providers implemented: a local **Ollama** endpoint and **Anthropic**'s Messages API; the
//! others are typed and stored but not wired, so they are rejected up front by [`validate`].

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::StoredAgent;
use crate::arguments::{HarnessAdiArguments, HarnessProvider};
use crate::error::{Error, Result};

use super::conversation::{self, Turn};

/// Anthropic requires an explicit output cap, so default one when the agent sets none.
const DEFAULT_MAX_TOKENS: u64 = 4096;
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

/// Whether the loop can actually run these arguments. An `adi` agent with no provider is treated as
/// not-yet-configured (`NotRunnable`, so the run button stays hidden); a provider we have not wired
/// is a clearer `Unsupported`.
pub(super) fn validate(args: &HarnessAdiArguments) -> Result<()> {
    match args.provider {
        None => Err(Error::NotRunnable("harness:adi".to_string())),
        Some(HarnessProvider::Anthropic | HarnessProvider::Ollama) => Ok(()),
        Some(other) => Err(Error::Unsupported(format!(
            "the adi loop doesn't support the {} provider yet — use anthropic or a local ollama",
            other.as_str()
        ))),
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
        // validate() already rejected everything else.
        _ => Err(Error::Unsupported("unsupported provider".to_string())),
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
    if let Some(keep) = args.keep_alive.as_deref().filter(|k| !k.trim().is_empty()) {
        body["keep_alive"] = json!(keep);
    }

    let base = args
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or("http://localhost:11434");
    let url = format!("{}/api/chat", base.trim_end_matches('/'));
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
    let key_env = args
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .unwrap_or("ANTHROPIC_API_KEY");
    let key = std::env::var(key_env).map_err(|_| {
        Error::Unsupported(format!(
            "no Anthropic API key: environment variable {key_env} is unset (attach it as a secret on the agent)"
        ))
    })?;

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

    let base = args
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or("https://api.anthropic.com");
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
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

// ---- shared HTTP + argument helpers ------------------------------------------------

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
    fn provider_support_is_gated() {
        let mut args = HarnessAdiArguments::default();
        // No provider → not-yet-configured, which reads as not runnable.
        assert!(matches!(validate(&args), Err(Error::NotRunnable(b)) if b == "harness:adi"));
        args.provider = Some(HarnessProvider::Ollama);
        assert!(validate(&args).is_ok());
        args.provider = Some(HarnessProvider::Anthropic);
        assert!(validate(&args).is_ok());
        args.provider = Some(HarnessProvider::Openai);
        assert!(matches!(validate(&args), Err(Error::Unsupported(_))));
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
