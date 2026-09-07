//! Parse the `codex exec --json` JSONL event stream into a common [`TurnContent`] — beside
//! [`crate::backends::claude_stream::parse`], but a different wire. Codex's schema is
//! `{"type": "item.completed", "item": {"id": …, "type": "agent_message", "text": …}}`, not
//! Claude's `{"type": "assistant", "message": {"content": [...]}}`, so nothing here reuses that
//! parser's field names — only the shared [`pop_trailing_message_if`] rule, which is the same rule
//! regardless of whose wire format it was read off.
//!
//! Confirmed against a live `codex exec --json` run (CLI 0.153.2) and the upstream event types
//! (`codex-rs/exec/src/exec_events.rs`, `ThreadEvent`) rather than guessed: `thread.started`,
//! `turn.started`, `item.started` / `item.completed` (an id plus a `type`-tagged item),
//! `turn.completed` (usage), `turn.failed` (an error), and a bare `error` for a stream that dies
//! before any turn does. Unrecognised event and item kinds are skipped, the same tolerance
//! `claude_stream` extends its own CLI — a newer Codex's extra events cost a turn nothing.
//!
//! Codex reports no reasoning capability — `crate::progress::capabilities` claims `thinking: false`
//! for `process:codex`, because [`crate::runner::detached::DetachedRunner::emits`] does — so a
//! `reasoning` item's summary is read for nothing here. Surfacing it as a [`Step::Thinking`] a
//! reader was told this engine never sends would make the claim a lie the parser itself commits.

use std::collections::HashMap;

use serde_json::Value;

use crate::progress::{
    Step, ToolStatus, TurnContent, TurnMetrics, pop_trailing_message_if, text_of,
};

pub(crate) fn parse(log: &[u8]) -> TurnContent {
    let text = String::from_utf8_lossy(log);
    let mut steps: Vec<Step> = Vec::new();
    let mut item_index: HashMap<String, usize> = HashMap::new();
    let mut metrics: Option<TurnMetrics> = None;
    let mut saw_event = false;
    let mut result_text: Option<String> = None;
    let mut failed = false;

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
            "item.started" => absorb_item(&event, &mut steps, &mut item_index, false),
            "item.completed" => absorb_item(&event, &mut steps, &mut item_index, true),
            "turn.completed" => metrics = Some(parse_usage(event.get("usage"))),
            // `TurnFailedEvent { error: ThreadErrorEvent }` — the message nests under `error`.
            "turn.failed" => {
                failed = true;
                if let Some(message) = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                {
                    result_text = Some(message.to_string());
                }
            }
            // The bare `error` variant is internally tagged same as every other event, so its
            // `message` sits beside `type` rather than nested — unlike `turn.failed` above, which
            // carries a whole `ThreadErrorEvent` as a named field.
            "error" => {
                failed = true;
                if let Some(message) = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                {
                    result_text = Some(message.to_string());
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
            raw: true,
        };
    }

    let text = match result_text {
        Some(final_text) => {
            pop_trailing_message_if(&mut steps, |last| last == final_text);
            final_text
        }
        None => pop_trailing_message_if(&mut steps, |_| true).unwrap_or_default(),
    };

    let mut metrics = metrics.unwrap_or_default();
    metrics.is_error = metrics.is_error || failed;

    TurnContent {
        text,
        steps,
        metrics: (!metrics.is_empty()).then_some(metrics),
        raw: false,
    }
}

/// Fold one `item.started` / `item.completed` event onto the turn's timeline.
///
/// A tool-shaped item (`command_execution`, `file_change`, `mcp_tool_call`, `web_search`) is
/// indexed by its id so a later `item.completed` updates the row `item.started` opened rather than
/// adding a second one — the same pairing `claude_stream::absorb_tool_results` does by
/// `tool_use_id`, keyed here by the item's own id since Codex has one id per item, not two.
/// `agent_message` has no in-progress state at all — it is only ever emitted completed — so
/// `item.started` for one is a no-op rather than a running placeholder nobody would resolve.
fn absorb_item(
    event: &Value,
    steps: &mut Vec<Step>,
    index: &mut HashMap<String, usize>,
    completed: bool,
) {
    let Some(item) = event.get("item") else {
        return;
    };
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(kind) = item.get("type").and_then(Value::as_str) else {
        return;
    };

    match kind {
        "agent_message" if completed => {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !text.is_empty() {
                steps.push(Step::Message { text });
            }
        }
        // `agent_message` not yet completed (nothing to update — see the doc above), `reasoning`
        // (dropped rather than a `Step::Thinking`, see the module doc), and `todo_list` (the
        // agent's own running to-do list, not a call it made — nothing here renders it as one).
        // Named explicitly rather than left to the wildcard below so a reader can tell "skipped
        // on purpose" from "an item kind nobody has written yet".
        #[allow(clippy::match_same_arms)]
        "agent_message" | "reasoning" | "todo_list" => {}
        "command_execution" => upsert(steps, index, id, command_execution_step(item)),
        "file_change" => upsert(steps, index, id, file_change_step(item)),
        "mcp_tool_call" => upsert(steps, index, id, mcp_tool_call_step(item)),
        "web_search" => upsert(steps, index, id, web_search_step(item)),
        // A non-fatal error surfaced as an item (distinct from the stream-level `error` event
        // above) reads as commentary: it is the closest thing Codex has to the agent explaining
        // what went wrong mid-turn.
        "error" => {
            let text = item
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !text.is_empty() {
                steps.push(Step::Message { text });
            }
        }
        _ => {}
    }
}

/// Update the tool step `id` already opened, or open one — the id-keyed half of [`absorb_item`].
fn upsert(steps: &mut Vec<Step>, index: &mut HashMap<String, usize>, id: &str, step: Step) {
    if let Some(&at) = index.get(id) {
        steps[at] = step;
    } else {
        index.insert(id.to_string(), steps.len());
        steps.push(step);
    }
}

fn command_execution_step(item: &Value) -> Step {
    let command = item
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let output = item
        .get("aggregated_output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let status = match item.get("status").and_then(Value::as_str) {
        // `Completed` still needs its own exit code checked — a shell command that ran to
        // completion and returned non-zero is a failure the model needs flagged, not a call that
        // merely finished.
        Some("completed") => {
            if item.get("exit_code").and_then(Value::as_i64) == Some(0) {
                ToolStatus::Ok
            } else {
                ToolStatus::Error
            }
        }
        Some("failed" | "declined") => ToolStatus::Error,
        _ => ToolStatus::Running,
    };
    Step::Tool {
        name: "shell".to_string(),
        input: command,
        status,
        output,
    }
}

fn file_change_step(item: &Value) -> Step {
    let input = item
        .get("changes")
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .filter_map(|change| {
                    let path = change.get("path").and_then(Value::as_str)?;
                    let kind = change
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("update");
                    Some(format!("{kind} {path}"))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let status = match item.get("status").and_then(Value::as_str) {
        Some("completed") => ToolStatus::Ok,
        Some("failed") => ToolStatus::Error,
        _ => ToolStatus::Running,
    };
    Step::Tool {
        name: "file_change".to_string(),
        input,
        status,
        output: String::new(),
    }
}

fn mcp_tool_call_step(item: &Value) -> Step {
    let server = item
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or_default();
    let input = item.get("arguments").map(compact_json).unwrap_or_default();
    let status = match item.get("status").and_then(Value::as_str) {
        Some("completed") => ToolStatus::Ok,
        Some("failed") => ToolStatus::Error,
        _ => ToolStatus::Running,
    };
    // An error's own message when there is one; otherwise the whole result, compact — the MCP
    // content block shape is `rmcp`'s, not this crate's, so it is shown as the JSON it is rather
    // than guessed apart the way a tool's own input is.
    let output = item
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| item.get("result").map(compact_json))
        .unwrap_or_default();
    Step::Tool {
        name: format!("{server}.{tool}"),
        input,
        status,
        output,
    }
}

fn web_search_step(item: &Value) -> Step {
    let query = item
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Step::Tool {
        name: "web_search".to_string(),
        input: query,
        status: ToolStatus::Ok,
        output: String::new(),
    }
}

/// Compact one-line JSON for a tool's input (dropping whitespace), or a short string as-is.
fn compact_json(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn parse_usage(usage: Option<&Value>) -> TurnMetrics {
    TurnMetrics {
        input_tokens: usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_u64),
        // Codex's `turn.completed` reports token counts and nothing else — no cost, no duration,
        // no turn count, no denials, no `terminal_reason` of its own. `TurnMetrics::is_empty`
        // treats an all-default value as "nothing to show", which is exactly what an engine that
        // truly reports nothing more should leave behind.
        ..TurnMetrics::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured live from `codex exec --json --sandbox danger-full-access` (CLI 0.153.2): a message,
    // a shell command, and the answer that follows it.
    const TOOL_TURN: &str = concat!(
        r#"{"type":"thread.started","thread_id":"01a07d7f-23ec-7a03-926d-872561373b78"}"#,
        "\n",
        r#"{"type":"turn.started"}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I'll read `sample.txt` with `cat`."}}"#,
        "\n",
        r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'cat sample.txt'","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'cat sample.txt'","aggregated_output":"test file\n","exit_code":0,"status":"completed"}}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"The exact contents are `test file` followed by a newline."}}"#,
        "\n",
        r#"{"type":"turn.completed","usage":{"input_tokens":30553,"cached_input_tokens":27264,"cache_write_input_tokens":0,"output_tokens":64,"reasoning_output_tokens":0}}"#,
    );

    #[test]
    fn a_tool_turn_becomes_a_message_a_shell_step_and_the_final_answer() {
        let c = parse(TOOL_TURN.as_bytes());
        assert_eq!(
            c.text,
            "The exact contents are `test file` followed by a newline."
        );
        assert!(!c.raw);
        assert_eq!(
            c.steps,
            vec![
                Step::Message {
                    text: "I'll read `sample.txt` with `cat`.".into()
                },
                Step::Tool {
                    name: "shell".into(),
                    input: "/bin/bash -lc 'cat sample.txt'".into(),
                    status: ToolStatus::Ok,
                    output: "test file".into(),
                },
            ]
        );
        let m = c.metrics.expect("usage is reported");
        assert_eq!(m.input_tokens, Some(30553));
        assert_eq!(m.output_tokens, Some(64));
        assert!(!m.is_error);
    }

    /// While the command is still running (only `item.started` has landed) the step reads as
    /// running, and there is no answer yet.
    #[test]
    fn an_in_flight_command_has_no_output_yet_and_stays_running() {
        let partial: Vec<&str> = TOOL_TURN.lines().take(4).collect();
        let c = parse(partial.join("\n").as_bytes());
        assert!(c.metrics.is_none());
        assert_eq!(c.steps.len(), 2);
        assert!(matches!(
            c.steps[1],
            Step::Tool {
                status: ToolStatus::Running,
                ..
            }
        ));
    }

    /// A command that exits non-zero is a failed call even though it "completed" — Codex's own
    /// status field only says the process exited, not that it succeeded.
    #[test]
    fn a_nonzero_exit_is_an_error_status_even_though_the_item_completed() {
        let log = concat!(
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"false","aggregated_output":"","exit_code":1,"status":"completed"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#,
        );
        let c = parse(log.as_bytes());
        assert!(matches!(
            c.steps[0],
            Step::Tool {
                status: ToolStatus::Error,
                ..
            }
        ));
    }

    /// `turn.failed` carries the reason the model never got to answer, and it becomes the turn's
    /// text — the honest answer to "what happened" when nothing else was said.
    #[test]
    fn a_failed_turn_reports_its_error_as_the_answer_and_flags_the_metrics() {
        let log = concat!(
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Let me check."}}"#,
            "\n",
            r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#,
        );
        let c = parse(log.as_bytes());
        assert_eq!(c.text, "stream disconnected");
        assert_eq!(
            c.steps,
            vec![Step::Message {
                text: "Let me check.".into()
            }],
            "the commentary before the failure stays on the timeline"
        );
        let m = c.metrics.expect("a failure still reports metrics");
        assert!(m.is_error);
    }

    /// A stream that dies before any turn does — the bare `error` event, whose message sits
    /// beside `type` rather than nested under an `error` key the way `turn.failed`'s does.
    #[test]
    fn a_bare_stream_error_is_read_from_its_own_message_field() {
        let c = parse(br#"{"type":"error","message":"could not reach the model"}"#);
        assert_eq!(c.text, "could not reach the model");
        assert!(c.metrics.expect("flagged").is_error);
    }

    /// A reasoning item is never turned into a `Step::Thinking` — this engine's capabilities say
    /// it reports no thinking, and this is where that claim is kept true.
    #[test]
    fn a_reasoning_item_is_dropped_rather_than_shown_as_thinking() {
        let log = concat!(
            r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"weighing options"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Done."}}"#,
        );
        let c = parse(log.as_bytes());
        assert_eq!(c.text, "Done.");
        assert!(c.steps.is_empty(), "{:?}", c.steps);
    }

    /// A run with `--json` never passed (or one whose log is not this wire at all) yields
    /// plain text rather than an empty message — the fallback every backend without a matching
    /// event on a line must have, so a genuinely different log still produces *something*.
    #[test]
    fn a_log_with_no_recognisable_events_falls_back_to_plain_text_and_is_marked_raw() {
        let c = parse(b"Hello, nice to meet you!\n");
        assert_eq!(c.text, "Hello, nice to meet you!");
        assert!(c.steps.is_empty());
        assert!(
            c.raw,
            "unparsed content must not be read as the engine's own structured answer"
        );
    }

    #[test]
    fn a_plain_text_fallback_still_drops_the_engines_own_startup_tracing() {
        let log = "Reading additional input from stdin...\n\
                    2026-09-07T19:37:40.559219Z INFO codex_exec{otel.kind=\"internal\"}: \
                    codex_http_client::custom_ca: using system root certificates\n\
                    Hello, nice to meet you!\n";
        let c = parse(log.as_bytes());
        assert_eq!(c.text, "Hello, nice to meet you!");
        assert!(c.raw);
    }
}
