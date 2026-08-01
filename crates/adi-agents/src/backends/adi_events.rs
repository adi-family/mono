//! The `harness:adi` loop's own event stream: what a turn writes to stdout while it works, and how
//! that log is read back into a [`TurnContent`].
//!
//! A turn used to print one thing — its answer — at the end. With tools it has a *timeline*: it
//! says something, calls a tool, reads the result, calls another, and eventually answers. Writing
//! that timeline out as it happens is what lets the panel show a tool running before the turn
//! settles, and it costs the loop nothing: the same log is the answer once the turn is over.
//!
//! One JSON object per line, each with a `kind`. A line that isn't JSON is not an error — the
//! parser keeps it as answer text, which is how a crash message or an old plain-text log still
//! reads correctly instead of vanishing into a parse failure.

use std::collections::HashMap;
use std::io::Write;

use serde_json::{Value, json};

use crate::progress::{Step, ToolStatus, TurnContent, TurnMetrics};

/// Where a turn writes its events. The child's stdout in production; a buffer in tests.
pub(crate) type Sink<'a> = &'a mut dyn Write;

/// Write one event line, flushing so the panel sees it while the turn is still running — the whole
/// point of streaming. A failed write is ignored: losing an event must never abort the turn that
/// was producing it.
fn emit(sink: Sink<'_>, event: &Value) {
    let _ = writeln!(sink, "{event}");
    let _ = sink.flush();
}

/// Something the agent said before its final answer — commentary between tool calls.
pub(crate) fn message(sink: Sink<'_>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    emit(sink, &json!({ "kind": "message", "text": text }));
}

/// A tool call that has started. Emitted before the tool runs, so a slow command shows as running
/// rather than as nothing at all.
pub(crate) fn tool_started(sink: Sink<'_>, id: &str, name: &str, input: &Value) {
    emit(
        sink,
        &json!({ "kind": "tool", "id": id, "name": name, "input": render_input(input), "status": "running" }),
    );
}

/// The same tool call, finished. Keyed by `id` so it replaces the running line rather than adding
/// a second entry.
pub(crate) fn tool_finished(sink: Sink<'_>, id: &str, name: &str, output: &str, ok: bool) {
    emit(
        sink,
        &json!({
            "kind": "tool",
            "id": id,
            "name": name,
            "status": if ok { "ok" } else { "error" },
            "output": output,
        }),
    );
}

/// The turn's answer. Last one wins, so a loop that stops early still reports what it had.
pub(crate) fn answer(sink: Sink<'_>, text: &str) {
    emit(sink, &json!({ "kind": "answer", "text": text }));
}

/// Per-turn telemetry, accumulated across every round the turn took.
pub(crate) fn metrics(sink: Sink<'_>, metrics: &TurnMetrics) {
    let mut event = serde_json::to_value(metrics).unwrap_or_else(|_| json!({}));
    event["kind"] = json!("metrics");
    emit(sink, &event);
}

/// Arguments as one short line: the panel shows this beside the tool name, where a pretty-printed
/// object would push everything else off the row.
fn render_input(input: &Value) -> String {
    let compact = input.to_string();
    if compact.chars().count() <= 200 {
        return compact;
    }
    let cut: String = compact.chars().take(200).collect();
    format!("{cut}…")
}

/// Read a turn's log back into its content: the answer, the timeline that produced it, and the
/// telemetry. Never fails — an unparseable log yields its own text, which is the honest reading of
/// a log written by something that crashed before it could say anything structured.
pub(crate) fn parse(log: &[u8]) -> TurnContent {
    let text = String::from_utf8_lossy(log);
    let mut steps: Vec<Step> = Vec::new();
    // Tool call id → index into `steps`, so a finished call updates the row its start created.
    let mut tool_index: HashMap<String, usize> = HashMap::new();
    let mut metrics: Option<TurnMetrics> = None;
    let mut answer: Option<String> = None;
    let mut plain = String::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            // Not one of ours: a panic, a stray print, or a log from before this format existed.
            plain.push_str(line);
            plain.push('\n');
            continue;
        };
        match event.get("kind").and_then(Value::as_str) {
            Some("message") => {
                if let Some(t) = event.get("text").and_then(Value::as_str) {
                    steps.push(Step::Message { text: t.to_string() });
                }
            }
            Some("tool") => fold_tool(&event, &mut steps, &mut tool_index),
            Some("answer") => {
                answer = event
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("metrics") => {
                metrics = serde_json::from_value::<TurnMetrics>(event).ok();
            }
            // An unknown kind is a newer writer talking to an older reader: skip it rather than
            // treating it as text, which would put machine noise in front of a person.
            _ => {}
        }
    }

    let text = answer.unwrap_or_else(|| plain.trim().to_string());
    TurnContent {
        text,
        steps,
        metrics: metrics.filter(|m| !m.is_empty()),
    }
}

/// Fold one `tool` event into the timeline: start a row, or complete the row it belongs to.
fn fold_tool(event: &Value, steps: &mut Vec<Step>, index: &mut HashMap<String, usize>) {
    let id = event
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = event
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    let status = match event.get("status").and_then(Value::as_str) {
        Some("ok") => ToolStatus::Ok,
        Some("error") => ToolStatus::Error,
        _ => ToolStatus::Running,
    };
    let output = event
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let input = event
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if let Some(&at) = index.get(&id)
        && let Some(Step::Tool {
            status: s,
            output: o,
            ..
        }) = steps.get_mut(at)
    {
        *s = status;
        if !output.is_empty() {
            *o = output;
        }
        return;
    }
    index.insert(id, steps.len());
    steps.push(Step::Tool {
        name,
        input,
        status,
        output,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_of(events: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in events {
            emit(&mut out, e);
        }
        out
    }

    #[test]
    fn a_finished_call_updates_the_row_its_start_created() {
        let log = log_of(&[
            json!({"kind": "tool", "id": "c1", "name": "Read", "input": "{}", "status": "running"}),
            json!({"kind": "tool", "id": "c1", "name": "Read", "status": "ok", "output": "hello"}),
            json!({"kind": "answer", "text": "done"}),
        ]);
        let content = parse(&log);
        assert_eq!(content.text, "done");
        assert_eq!(content.steps.len(), 1, "one call is one row: {:?}", content.steps);
        let Step::Tool { status, output, .. } = &content.steps[0] else {
            panic!("expected a tool step, got {:?}", content.steps[0]);
        };
        assert_eq!(*status, ToolStatus::Ok);
        assert_eq!(output, "hello");
    }

    #[test]
    fn a_turn_still_running_shows_its_tool_as_running_and_no_answer_yet() {
        let log = log_of(&[
            json!({"kind": "message", "text": "let me look"}),
            json!({"kind": "tool", "id": "c1", "name": "Grep", "input": "{}", "status": "running"}),
        ]);
        let content = parse(&log);
        assert!(content.text.is_empty(), "no answer until one is written");
        assert!(matches!(content.steps[0], Step::Message { .. }));
        assert!(
            matches!(&content.steps[1], Step::Tool { status, .. } if *status == ToolStatus::Running)
        );
    }

    #[test]
    fn a_log_that_is_not_ours_is_read_as_the_answer() {
        // The error path prints a plain line, and logs written before this format existed are all
        // plain — both must still read as the turn's text rather than as nothing.
        let content = parse(b"\xe2\x9a\xa0 adi loop error: no API key\n");
        assert_eq!(content.text, "⚠ adi loop error: no API key");
        assert!(content.steps.is_empty());
    }

    #[test]
    fn metrics_round_trip_and_an_empty_set_is_dropped() {
        let mut out = Vec::new();
        metrics(
            &mut out,
            &TurnMetrics {
                input_tokens: Some(12),
                output_tokens: Some(34),
                ..TurnMetrics::default()
            },
        );
        let content = parse(&out);
        let m = content.metrics.expect("metrics survive the round trip");
        assert_eq!(m.input_tokens, Some(12));
        assert_eq!(m.output_tokens, Some(34));

        let mut empty = Vec::new();
        metrics(&mut empty, &TurnMetrics::default());
        assert!(parse(&empty).metrics.is_none(), "empty metrics are not shown");
    }
}
