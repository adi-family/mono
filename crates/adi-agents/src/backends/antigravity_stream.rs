//! Parse the Google Antigravity CLI (`agy --output-format stream-json`) NDJSON event stream into
//! a common [`TurnContent`] — the `process:antigravity` engine's counterpart to
//! [`claude_stream`](super::claude_stream) and [`codex_stream`](super::codex_stream).
//!
//! Each JSON line on stdout carries an `"event"` discriminator paired with a payload object of the
//! same name:
//! - `init` — emitted once at startup, with top-level `"conversation_id"`.
//! - `step_update` — emitted as each zero-based `step_index` transitions through `ACTIVE`, `DONE`,
//!   or `ERROR` (`agent_response`, `tool`, `subagent`, `user_input`, `system_message`, `finish`, …).
//! - `result` — terminal summary with `conversation_id`, `status`, `response`, `error`,
//!   `finished_reason`, `duration_seconds`, `num_turns`, `usage`, and `denied_actions`.
//! - `command_result` — occasional command status event (ignored for timeline content).
//!
//! Non-JSON lines (stderr narration, ADI's own `=== turn started ===` / `=== turn ended ===`
//! separators) are skipped whenever at least one Antigravity event is present, and stripped on
//! plain-text fallback when the CLI failed before emitting its first event.

use std::collections::HashMap;

use serde_json::Value;

use crate::progress::{Step, ToolStatus, TurnContent, TurnMetrics, text_of};

/// The durable conversation id Antigravity announces on `init` (and repeats on `step_update` and
/// `result`).
///
/// Kept separate from [`parse`] because it is runner state, not content to render. The detached
/// runner reads it after the first turn exits and passes it back as `--conversation <id>` on every
/// follow-up turn.
pub(crate) fn conversation_id(log: &[u8]) -> Option<String> {
    String::from_utf8_lossy(log).lines().find_map(|line| {
        let event = serde_json::from_str::<Value>(line.trim()).ok()?;
        let kind = event.get("event").and_then(Value::as_str)?;
        let raw = match kind {
            "init" => event
                .get("conversation_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    event
                        .get("init")
                        .and_then(|v| v.get("conversation_id"))
                        .and_then(Value::as_str)
                }),
            "step_update" => event
                .get("step_update")
                .and_then(|v| v.get("conversation_id"))
                .and_then(Value::as_str)
                .or_else(|| event.get("conversation_id").and_then(Value::as_str)),
            "result" => event
                .get("result")
                .and_then(|v| v.get("conversation_id"))
                .and_then(Value::as_str)
                .or_else(|| event.get("conversation_id").and_then(Value::as_str)),
            _ => None,
        }?;
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

pub(crate) fn parse(log: &[u8]) -> TurnContent {
    let log_text = String::from_utf8_lossy(log);
    let mut steps: Vec<Step> = Vec::new();
    let mut step_positions: HashMap<u64, usize> = HashMap::new();
    let mut result_text: Option<String> = None;
    let mut metrics: Option<TurnMetrics> = None;
    let mut error_message: Option<String> = None;
    let mut saw_event = false;

    for line in log_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(kind) = event.get("event").and_then(Value::as_str) else {
            continue;
        };
        if !is_antigravity_event(kind) {
            continue;
        }
        saw_event = true;
        match kind {
            "step_update" => {
                if let Some(update) = event.get("step_update") {
                    absorb_step_update(update, &mut steps, &mut step_positions);
                }
            }
            "result" => {
                if let Some(result) = event.get("result") {
                    absorb_result(result, &mut result_text, &mut metrics, &mut error_message);
                }
            }
            _ => {}
        }
    }

    if !saw_event {
        return TurnContent {
            text: text_of(log),
            steps: Vec::new(),
            metrics: None,
        };
    }

    // Remove trailing assistant message from `steps` so the final answer is not shown twice (once
    // as commentary and once as the turn reply). Prefer the terminal `result.response` when
    // present, falling back to the streamed message while the turn is still in flight.
    let streamed_answer = pop_trailing_message(&mut steps);
    let answer = result_text
        .filter(|text| !text.is_empty())
        .or(streamed_answer)
        .unwrap_or_default();

    let is_error = metrics.as_ref().is_some_and(|m| m.is_error);
    if !is_error {
        return TurnContent {
            text: answer,
            steps,
            metrics: metrics.filter(|m| !m.is_empty()),
        };
    }

    let text = match (answer.is_empty(), error_message.filter(|m| !m.is_empty())) {
        (true, Some(err)) => err,
        (false, Some(err)) if !answer.contains(&err) => format!("{answer}\n\n{err}"),
        _ => answer,
    };
    TurnContent {
        text,
        steps,
        metrics,
    }
}

fn is_antigravity_event(kind: &str) -> bool {
    matches!(kind, "init" | "step_update" | "result" | "command_result")
}

fn absorb_step_update(
    update: &Value,
    steps: &mut Vec<Step>,
    step_positions: &mut HashMap<u64, usize>,
) {
    let Some(step_type) = update.get("step_type").and_then(Value::as_str) else {
        return;
    };
    let step_index = update.get("step_index").and_then(Value::as_u64);
    let state = update
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match step_type {
        "agent_response" => {
            let delta = update
                .get("text_delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                return;
            }
            if let Some(idx) = step_index.and_then(|key| step_positions.get(&key).copied())
                && let Some(Step::Message { text }) = steps.get_mut(idx)
            {
                text.push_str(delta);
                return;
            }
            let pos = steps.len();
            steps.push(Step::Message {
                text: delta.to_string(),
            });
            if let Some(key) = step_index {
                step_positions.insert(key, pos);
            }
        }
        "thinking" => {
            let delta = update
                .get("text_delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                return;
            }
            if let Some(idx) = step_index.and_then(|key| step_positions.get(&key).copied())
                && let Some(Step::Thinking { text }) = steps.get_mut(idx)
            {
                text.push_str(delta);
                return;
            }
            let pos = steps.len();
            steps.push(Step::Thinking {
                text: delta.to_string(),
            });
            if let Some(key) = step_index {
                step_positions.insert(key, pos);
            }
        }
        "tool" => {
            upsert_tool(steps, step_positions, step_index, tool_step(update, state));
        }
        "subagent" => {
            let status = match state {
                "ERROR" => ToolStatus::Error,
                "DONE" => ToolStatus::Ok,
                _ => ToolStatus::Running,
            };
            let input = update
                .get("subagent_info")
                .map(compact_value)
                .unwrap_or_default();
            upsert_tool(
                steps,
                step_positions,
                step_index,
                Step::Tool {
                    name: "subagent".to_string(),
                    input,
                    status,
                    output: String::new(),
                },
            );
        }
        _ => {}
    }
}

fn tool_step(update: &Value, state: &str) -> Step {
    let tool_info = update.get("tool_info");
    let name = update
        .get("tool_name")
        .and_then(Value::as_str)
        .or_else(|| {
            tool_info
                .and_then(|info| info.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string();
    let input = tool_info
        .and_then(|info| info.get("parameters"))
        .map(compact_value)
        .unwrap_or_default();
    let has_error = tool_info
        .and_then(|info| info.get("error"))
        .is_some_and(|err| !err.is_null());
    let status = match state {
        "ERROR" => ToolStatus::Error,
        "DONE" if has_error => ToolStatus::Error,
        "DONE" => ToolStatus::Ok,
        _ => ToolStatus::Running,
    };
    let output = tool_info
        .and_then(|info| {
            info.get("error")
                .filter(|err| !err.is_null())
                .map(format_tool_error)
                .or_else(|| {
                    info.get("output")
                        .filter(|out| !out.is_null())
                        .map(compact_value)
                })
        })
        .unwrap_or_default();
    Step::Tool {
        name,
        input,
        status,
        output,
    }
}

fn upsert_tool(
    steps: &mut Vec<Step>,
    step_positions: &mut HashMap<u64, usize>,
    step_index: Option<u64>,
    step: Step,
) {
    if let Some(idx) = step_index.and_then(|key| step_positions.get(&key).copied())
        && let Some(slot) = steps.get_mut(idx)
    {
        if let (
            Step::Tool {
                input: existing_input,
                ..
            },
            Step::Tool {
                name,
                input,
                status,
                output,
            },
        ) = (slot, step)
        {
            let kept_input = if input.is_empty() {
                std::mem::take(existing_input)
            } else {
                input
            };
            steps[idx] = Step::Tool {
                name,
                input: kept_input,
                status,
                output,
            };
        }
        return;
    }
    let pos = steps.len();
    steps.push(step);
    if let Some(key) = step_index {
        step_positions.insert(key, pos);
    }
}

fn absorb_result(
    result: &Value,
    result_text: &mut Option<String>,
    metrics: &mut Option<TurnMetrics>,
    error_message: &mut Option<String>,
) {
    if let Some(response) = result.get("response").and_then(Value::as_str) {
        let trimmed = response.trim();
        if !trimmed.is_empty() {
            *result_text = Some(trimmed.to_string());
        }
    }

    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let error = result
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|err| !err.is_empty())
        .map(ToString::to_string);
    let finished_reason = result
        .get("finished_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(ToString::to_string);

    let is_error = error.is_some()
        || (!status.is_empty() && !status.eq_ignore_ascii_case("SUCCESS"))
        || finished_reason
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("error"));
    if let Some(err) = error {
        *error_message = Some(err);
    }

    let usage = result.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(Value::as_u64);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "duration_seconds is guarded to be finite and non-negative before converting to ms"
    )]
    let duration_ms = result
        .get("duration_seconds")
        .and_then(Value::as_f64)
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .map(|secs| (secs * 1000.0).round() as u64);
    let num_turns = result.get("num_turns").and_then(Value::as_u64);
    let permission_denials = result
        .get("denied_actions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("display_name")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("action").and_then(Value::as_str))
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let terminal_reason = finished_reason.or_else(|| {
        if is_error && !status.is_empty() {
            Some(status.to_ascii_lowercase())
        } else {
            None
        }
    });

    let m = TurnMetrics {
        input_tokens,
        output_tokens,
        cost_micro_usd: None,
        duration_ms,
        num_turns,
        permission_denials,
        is_error,
        terminal_reason,
    };
    if !m.is_empty() {
        *metrics = Some(m);
    }
}

fn pop_trailing_message(steps: &mut Vec<Step>) -> Option<String> {
    if !matches!(steps.last(), Some(Step::Message { .. })) {
        return None;
    }
    match steps.pop() {
        Some(Step::Message { text }) => {
            let trimmed = text.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        }
        _ => None,
    }
}

fn format_tool_error(err: &Value) -> String {
    if let Some(msg) = err.get("message").and_then(Value::as_str) {
        let trimmed = msg.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    compact_value(err)
}

fn compact_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.trim().to_string(),
        Value::Object(map) if map.is_empty() => String::new(),
        Value::Array(arr) if arr.is_empty() => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_conversation_id_from_init_and_fallbacks() {
        let log = concat!(
            "=== turn started: 2026-11-23T10:46:10Z ===\n",
            "{\"event\":\"init\",\"conversation_id\":\"2c57957b-ef6f-4679-be89-9e3430c48473\",\"init\":{\"model\":\"gemini-3.8-flash-low\"}}\n",
        );
        assert_eq!(
            conversation_id(log.as_bytes()).as_deref(),
            Some("2c57957b-ef6f-4679-be89-9e3430c48473")
        );

        let from_result = "{\"event\":\"result\",\"result\":{\"conversation_id\":\"conv-from-result\",\"status\":\"SUCCESS\"}}\n";
        assert_eq!(
            conversation_id(from_result.as_bytes()).as_deref(),
            Some("conv-from-result")
        );
    }

    #[test]
    fn parses_real_stream_json_turn_with_deltas_and_metrics() {
        let log = concat!(
            "{\"event\":\"init\",\"conversation_id\":\"c-1\",\"init\":{\"model\":\"gemini-3.8-flash-low\",\"cwd\":\"/repo\"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-1\",\"step_index\":0,\"state\":\"DONE\",\"step_type\":\"user_input\"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-1\",\"step_index\":1,\"state\":\"ACTIVE\",\"step_type\":\"agent_response\",\"text_delta\":\"O\"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-1\",\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"K\\n\",\"duration_seconds\":1.82}}\n",
            "{\"event\":\"result\",\"result\":{\"conversation_id\":\"c-1\",\"status\":\"SUCCESS\",\"response\":\"OK\\n\",\"duration_seconds\":1.857,\"num_turns\":1,\"usage\":{\"input_tokens\":10206,\"output_tokens\":1,\"thinking_tokens\":0,\"cache_read_tokens\":0,\"total_tokens\":10207}}}\n",
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "OK");
        assert!(
            content.steps.is_empty(),
            "final reply is not duplicated in steps"
        );
        let metrics = content.metrics.expect("metrics present");
        assert_eq!(metrics.input_tokens, Some(10206));
        assert_eq!(metrics.output_tokens, Some(1));
        assert_eq!(metrics.duration_ms, Some(1857));
        assert_eq!(metrics.num_turns, Some(1));
        assert!(!metrics.is_error);
    }

    #[test]
    fn folds_tool_steps_in_place_and_records_denied_actions() {
        let log = concat!(
            "{\"event\":\"init\",\"conversation_id\":\"c-2\",\"init\":{}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-2\",\"step_index\":4,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"Checking files...\"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-2\",\"step_index\":5,\"state\":\"ACTIVE\",\"step_type\":\"tool\",\"tool_name\":\"run_command\",\"tool_info\":{\"name\":\"run_command\",\"parameters\":{\"CommandLine\":\"ls -la\"}}}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-2\",\"step_index\":5,\"state\":\"ERROR\",\"step_type\":\"tool\",\"tool_name\":\"run_command\",\"tool_info\":{\"name\":\"run_command\",\"parameters\":{\"CommandLine\":\"ls -la\"},\"error\":{\"type\":\"TOOL_ERROR\",\"message\":\"permission denied\"}}}}\n",
            "jetski: no output produced — a tool required permission.\n",
            "{\"event\":\"result\",\"result\":{\"conversation_id\":\"c-2\",\"status\":\"SUCCESS\",\"response\":\"\",\"duration_seconds\":2.5,\"num_turns\":2,\"denied_actions\":[{\"action\":\"command\",\"display_name\":\"Bash\"}]}}\n",
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "");
        assert_eq!(
            content.steps,
            vec![
                Step::Message {
                    text: "Checking files...".into(),
                },
                Step::Tool {
                    name: "run_command".into(),
                    input: "{\"CommandLine\":\"ls -la\"}".into(),
                    status: ToolStatus::Error,
                    output: "permission denied".into(),
                },
            ]
        );
        let metrics = content.metrics.expect("metrics");
        assert_eq!(metrics.permission_denials, vec!["Bash".to_string()]);
        assert!(!metrics.is_error);
    }

    #[test]
    fn surfaces_error_result_and_reason() {
        let log = concat!(
            "{\"event\":\"init\",\"conversation_id\":\"c-3\",\"init\":{}}\n",
            "{\"event\":\"result\",\"result\":{\"conversation_id\":\"c-3\",\"status\":\"ERROR\",\"error\":\"429 quota exceeded\",\"finished_reason\":\"error\"}}\n",
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "429 quota exceeded");
        let metrics = content.metrics.expect("metrics");
        assert!(metrics.is_error);
        assert_eq!(metrics.terminal_reason.as_deref(), Some("error"));
    }

    #[test]
    fn folds_thinking_deltas_into_thinking_steps() {
        let log = concat!(
            "{\"event\":\"init\",\"conversation_id\":\"c-t\",\"init\":{}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-t\",\"step_index\":0,\"state\":\"ACTIVE\",\"step_type\":\"thinking\",\"text_delta\":\"Plan: \"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-t\",\"step_index\":0,\"state\":\"ACTIVE\",\"step_type\":\"thinking\",\"text_delta\":\"run TsCode\"}}\n",
            "{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-t\",\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"Done.\"}}\n",
            "{\"event\":\"result\",\"result\":{\"conversation_id\":\"c-t\",\"status\":\"SUCCESS\",\"response\":\"Done.\"}}\n",
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "Done.");
        assert_eq!(
            content.steps,
            vec![Step::Thinking {
                text: "Plan: run TsCode".into(),
            }]
        );
    }

    #[test]
    fn falls_back_to_plain_text_without_adi_chrome_when_no_events_arrived() {
        let log = "=== turn started: 2026-11-23T10:00:00Z ===\nagy: not logged in\n=== turn ended: 2026-11-23T10:00:01Z ===\n";
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "agy: not logged in");
        assert!(content.steps.is_empty());
        assert!(content.metrics.is_none());
    }
}
