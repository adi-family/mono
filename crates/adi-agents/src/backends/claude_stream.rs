//! Parse the `claude --print --output-format stream-json` NDJSON event stream into a common
//! [`TurnContent`] — shared by the `process:claude` and `harness:claude-sdk` engines.
//!
//! The stream is one JSON object per line. The ones that carry progress:
//! - `assistant` — a message whose `content` blocks are `thinking`, `tool_use`, or `text`.
//! - `user` — carries `tool_result` blocks (a tool's output), keyed back to its `tool_use` by id.
//! - `result` — the terminal event: authoritative final `result` text plus metrics (`usage`,
//!   `total_cost_usd`, `duration_ms`, `num_turns`, `permission_denials`, `is_error`).
//!
//! Partial streams (a turn still in flight) parse fine: a `tool_use` with no matching `tool_result`
//! yet stays [`ToolStatus::Running`], and with no `result` event the answer is the text blocks so
//! far and metrics are absent. A log that is *not* this stream (plain text, an old log) falls back
//! to text-only content, so this is safe to run on any claude-backend log.

use std::collections::HashMap;

use serde_json::Value;

use crate::progress::{Step, ToolStatus, TurnContent, TurnMetrics, text_of};

pub(crate) fn parse(log: &[u8]) -> TurnContent {
    let text = String::from_utf8_lossy(log);
    let mut steps: Vec<Step> = Vec::new();
    // tool_use id → index into `steps`, so its later `tool_result` attaches to the right tool.
    let mut tool_index: HashMap<String, usize> = HashMap::new();
    let mut answer = String::new();
    let mut metrics: Option<TurnMetrics> = None;
    let mut saw_event = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        saw_event = true;
        match kind {
            "assistant" => absorb_assistant(&event, &mut steps, &mut tool_index, &mut answer),
            "user" => absorb_tool_results(&event, &mut steps, &tool_index),
            "result" => {
                if let Some(final_text) = event.get("result").and_then(Value::as_str) {
                    if !final_text.trim().is_empty() {
                        // The result event's text is authoritative for the final answer.
                        answer = final_text.to_string();
                    }
                }
                metrics = Some(parse_metrics(&event));
            }
            _ => {}
        }
    }

    if !saw_event {
        // Not a stream-json log (plain text / old log): the whole thing is the answer.
        return TurnContent {
            text: text_of(log),
            steps: Vec::new(),
            metrics: None,
        };
    }

    TurnContent {
        text: answer.trim().to_string(),
        steps,
        metrics: metrics.filter(|m| !m.is_empty()),
    }
}

/// Fold an `assistant` message's content blocks into thinking/tool steps and answer text.
fn absorb_assistant(
    event: &Value,
    steps: &mut Vec<Step>,
    tool_index: &mut HashMap<String, usize>,
    answer: &mut String,
) {
    for block in content_blocks(event) {
        match block.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                let text = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    steps.push(Step::Thinking { text });
                }
            }
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    answer.push_str(t);
                }
            }
            Some("tool_use") => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let input = block
                    .get("input")
                    .map(compact_json)
                    .unwrap_or_default();
                steps.push(Step::Tool {
                    name,
                    input,
                    status: ToolStatus::Running,
                    output: String::new(),
                });
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    tool_index.insert(id.to_string(), steps.len() - 1);
                }
            }
            _ => {}
        }
    }
}

/// Attach each `tool_result` block in a `user` message to the tool step it answers, by id.
fn absorb_tool_results(event: &Value, steps: &mut [Step], tool_index: &HashMap<String, usize>) {
    for block in content_blocks(event) {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(&idx) = tool_index.get(id) else {
            continue;
        };
        let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let result = tool_result_text(block.get("content"));
        if let Some(Step::Tool { status, output, .. }) = steps.get_mut(idx) {
            *status = if is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            };
            *output = result;
        }
    }
}

/// The content blocks of a message event (`event.message.content`), or empty.
fn content_blocks(event: &Value) -> Vec<Value> {
    event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A tool_result block's content flattened to text (it is a string, or an array of `{type:text}`).
fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Parse the terminal `result` event's telemetry.
fn parse_metrics(event: &Value) -> TurnMetrics {
    let usage = event.get("usage");
    let denials = event
        .get("permission_denials")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    d.as_str()
                        .or_else(|| d.get("tool_name").and_then(Value::as_str))
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    TurnMetrics {
        input_tokens: usage.and_then(|u| u.get("input_tokens")).and_then(Value::as_u64),
        output_tokens: usage.and_then(|u| u.get("output_tokens")).and_then(Value::as_u64),
        cost_micro_usd: event
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .map(|c| (c * 1_000_000.0).round() as u64),
        duration_ms: event.get("duration_ms").and_then(Value::as_u64),
        num_turns: event.get("num_turns").and_then(Value::as_u64),
        permission_denials: denials,
        is_error: event.get("is_error").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Compact one-line JSON for a tool's input (dropping whitespace), or a short string as-is.
fn compact_json(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A stream-json log for a turn that thinks, runs Bash, sees the result, then answers — the
    // shape captured live from `claude --print --output-format stream-json`.
    const TOOL_TURN: &str = concat!(
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-haiku"}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"I should run echo."}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hi"}}]}}"#,
        "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"hi"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"It printed hi."}]}}"#,
        "\n",
        r#"{"type":"result","is_error":false,"result":"It printed hi.","duration_ms":8596,"num_turns":3,"total_cost_usd":0.0194538,"usage":{"input_tokens":26,"output_tokens":523}}"#,
    );

    #[test]
    fn parses_thinking_tool_and_result_into_steps_text_and_metrics() {
        let c = parse(TOOL_TURN.as_bytes());
        assert_eq!(c.text, "It printed hi.");
        assert_eq!(c.steps.len(), 2);
        assert_eq!(c.steps[0], Step::Thinking { text: "I should run echo.".into() });
        assert_eq!(
            c.steps[1],
            Step::Tool {
                name: "Bash".into(),
                input: r#"{"command":"echo hi"}"#.into(),
                status: ToolStatus::Ok,
                output: "hi".into(),
            }
        );
        let m = c.metrics.expect("metrics");
        assert_eq!(m.input_tokens, Some(26));
        assert_eq!(m.output_tokens, Some(523));
        assert_eq!(m.cost_micro_usd, Some(19454)); // 0.0194538 * 1e6, rounded
        assert_eq!(m.duration_ms, Some(8596));
        assert!(!m.is_error);
    }

    #[test]
    fn an_in_flight_tool_has_no_result_yet_and_stays_running() {
        // Up to the tool_use, before its result or the terminal event.
        let partial: Vec<&str> = TOOL_TURN.lines().take(3).collect();
        let c = parse(partial.join("\n").as_bytes());
        assert!(c.metrics.is_none());
        assert_eq!(c.steps.len(), 2);
        assert!(matches!(c.steps[1], Step::Tool { status: ToolStatus::Running, .. }));
    }

    #[test]
    fn a_plain_text_log_falls_back_to_text_only() {
        let c = parse(b"just a plain answer\n");
        assert_eq!(c.text, "just a plain answer");
        assert!(c.steps.is_empty());
        assert!(c.metrics.is_none());
    }
}
