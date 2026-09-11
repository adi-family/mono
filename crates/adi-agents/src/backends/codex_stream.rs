//! Parse the `codex exec --json` thread-event stream into a common [`TurnContent`] — the
//! `process:codex` engine's half of what [`claude_stream`](super::claude_stream) does for Claude.
//!
//! Without this the runner had no parser for Codex at all and fell back to [`text_of`], which is
//! "the answer is the whole log file". A run's log is stdout **and stderr merged**, and Codex puts a
//! great deal on stderr: a `tracing` wall at `INFO` (every HTTP call, the models cache, the otel
//! events — including the account's email address), a startup banner, and a replay of the prompt it
//! was handed. All of it was arriving in the chat as the agent's answer. Two things fix that
//! together: [`quiet_codex_env`](super::quiet_codex_env) silences the tracing wall at the source,
//! and this reads the answer out of the structured stream instead of off the floor. Anything that
//! is not one of Codex's own events — every remaining line of chrome — is skipped.
//!
//! The stream is one JSON object per line (`--json`, which the argv builder now always passes):
//! - `thread.started` / `turn.started` — the turn's bookends, carrying nothing to show.
//! - `item.started` / `item.updated` / `item.completed` — one *item*, keyed by `item.id`, seen
//!   two or three times as it progresses. They fold onto **one** step, in place.
//! - `turn.completed` — terminal, with `usage`.
//! - `turn.failed` / `error` — the turn died; `error.message` is what happened.
//!
//! **Order is content**, as it is for Claude: each `agent_message` keeps its place in the sequence
//! and only the last one becomes [`TurnContent::text`].
//!
//! Codex emits `reasoning` items only when `model_reasoning_summary` is on, and nothing in ADI's
//! invocation turns it on — which is why `process:codex` still does not advertise `thinking` in
//! `emits`. They are parsed anyway: a step that arrives is worth showing, and the flag is about what
//! the engine normally sends.
//!
//! The format is the CLI's, not ours. An unrecognised *event* is skipped; an unrecognised *item* is
//! shown as a step named after its type rather than dropped, so a newer Codex's tool does not go
//! missing from the timeline.

use std::collections::HashMap;

use serde_json::Value;

use crate::progress::{Step, ToolStatus, TurnContent, TurnMetrics, text_of};

pub(crate) fn parse(log: &[u8]) -> TurnContent {
    let log_text = String::from_utf8_lossy(log);
    let mut steps: Vec<Step> = Vec::new();
    let mut item_index: HashMap<String, usize> = HashMap::new();
    let mut metrics: Option<TurnMetrics> = None;
    let mut failure: Option<Failure> = None;
    let mut saw_event = false;

    for line in log_text.lines() {
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
        if !is_codex_event(kind) {
            continue;
        }
        saw_event = true;
        match kind {
            "item.started" | "item.updated" | "item.completed" => {
                absorb_item(&event, &mut steps, &mut item_index);
            }
            "turn.completed" => metrics = Some(usage_metrics(event.get("usage"))),
            // Both say the same thing, and a failing turn sends both — the first one with a message
            // wins so the account of what went wrong is the engine's first word on it.
            "turn.failed" | "error" => {
                let found = Failure::read(&event);
                if failure.is_none() && !found.message.is_empty() {
                    failure = Some(found);
                }
            }
            _ => {}
        }
    }

    // Not this stream at all: a log from before `--json` was passed unconditionally, or one written
    // by a Codex that died before it could say anything. Whatever is in it is the best answer there
    // is — minus the chrome, which is all that is left of it when the run failed this early.
    if !saw_event {
        return TurnContent {
            text: without_chrome(&text_of(log)),
            steps: Vec::new(),
            metrics: None,
        };
    }

    let answer = pop_trailing_message(&mut steps).unwrap_or_default();
    let Some(failure) = failure else {
        return TurnContent {
            text: answer,
            steps,
            metrics: metrics.filter(|m| !m.is_empty()),
        };
    };

    // A turn that failed after saying something keeps both: the words are the agent's, the failure
    // is the outcome, and `Agents::fail_over` classifies the pair — so a "usage limit" that arrived
    // mid-answer still reroutes the run.
    let text = if answer.is_empty() {
        failure.message
    } else {
        format!("{answer}\n\n{}", failure.message)
    };
    let mut metrics = metrics.unwrap_or_default();
    metrics.is_error = true;
    metrics.terminal_reason = Some(failure.reason);
    TurnContent {
        text,
        steps,
        metrics: Some(metrics),
    }
}

/// Whether a `type` is one of Codex's own thread events.
///
/// The gate matters because this parses a log with Codex's stderr in it. A JSON line that is not a
/// thread event is somebody else's — and treating one as proof the log is a stream would make every
/// noisy run report an empty answer instead of falling back to its text.
fn is_codex_event(kind: &str) -> bool {
    kind.starts_with("thread.")
        || kind.starts_with("turn.")
        || kind.starts_with("item.")
        || kind == "error"
}

/// Codex's own narration, dropped from a log we are reduced to quoting verbatim.
///
/// `exec` announces that it is reading the prompt from a pipe every single time, on stderr, and the
/// runner merges stderr into the log. It is harmless while there is a stream to read the answer
/// from; when there is not — a Codex that refused to start, say — it is half of what the chat would
/// otherwise show, above the one line that actually says why.
fn without_chrome(text: &str) -> String {
    const NARRATION: [&str; 1] = ["Reading additional input from stdin..."];
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !NARRATION.contains(&line.trim()))
        .collect();
    kept.join("\n").trim().to_string()
}

/// How a turn ended badly: what to show, and the engine's own name for it.
struct Failure {
    /// The human sentence, unwrapped from the provider envelope when there is one.
    message: String,
    /// The code and HTTP status, for [`TurnMetrics::terminal_reason`] — which
    /// [`Agents::fail_over`](crate::Agents) prepends to the text before matching a backend's
    /// `limit_rules`. Unwrapping the envelope would otherwise throw away the `429` a rate rule
    /// looks for.
    reason: String,
}

impl Failure {
    fn read(event: &Value) -> Self {
        // `turn.failed` nests it under `error`; the bare `error` event carries it at the top. The
        // code is only read from the nested form, because at the top level `type` is the *event's*
        // name — `"error"` — and calling that the reason a turn died says nothing.
        let nested = event.get("error");
        let error = nested.unwrap_or(event);
        let raw = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let code = nested
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        // Codex hands the provider's error through verbatim, so `message` is routinely a whole JSON
        // envelope: `{"type":"error","status":400,"error":{"type":"…","message":"…"}}`. Show the
        // sentence inside it, and keep the envelope's code and status in the reason.
        let Ok(envelope) = serde_json::from_str::<Value>(raw) else {
            return Self {
                message: raw.to_string(),
                reason: reason_of(code, None),
            };
        };
        let inner = envelope.get("error").unwrap_or(&envelope);
        let message = inner
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or(raw);
        let code = inner
            .get("type")
            .and_then(Value::as_str)
            .filter(|code| !code.is_empty())
            .unwrap_or(code);
        let status = envelope
            .get("status")
            .and_then(Value::as_u64)
            .or_else(|| inner.get("status").and_then(Value::as_u64));
        Self {
            message: message.to_string(),
            reason: reason_of(code, status),
        }
    }
}

fn reason_of(code: &str, status: Option<u64>) -> String {
    match (code.trim(), status) {
        ("", None) => "failed".to_string(),
        ("", Some(status)) => format!("failed {status}"),
        (code, None) => code.to_string(),
        (code, Some(status)) => format!("{code} {status}"),
    }
}

/// Fold one item event onto the timeline — updating the step it already made, if it made one.
///
/// An item is emitted two or three times (`started`, sometimes `updated`, then `completed`) and is
/// one thing that happened. Keying by `item.id` is what keeps a shell command from appearing twice,
/// once running and once done.
fn absorb_item(event: &Value, steps: &mut Vec<Step>, item_index: &mut HashMap<String, usize>) {
    let Some(item) = event.get("item") else {
        return;
    };
    let Some(kind) = item.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(step) = step_of(kind, item) else {
        return;
    };
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
    match item_index.get(id).copied() {
        // An id seen before is the same item further along, so it replaces what it said last time.
        // A blank id is not an identity — it can only ever be appended.
        Some(at) if !id.is_empty() => steps[at] = step,
        _ => {
            steps.push(step);
            if !id.is_empty() {
                item_index.insert(id.to_string(), steps.len() - 1);
            }
        }
    }
}

/// One item, as the step it is. `None` for an item with nothing to show yet (an empty message).
fn step_of(kind: &str, item: &Value) -> Option<Step> {
    let text = |field: &str| {
        item.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    match kind {
        "agent_message" => {
            let text = text("text");
            (!text.is_empty()).then_some(Step::Message { text })
        }
        "reasoning" => {
            // Whichever of the three a build sends; `text` is the current one.
            let text = [text("text"), text("summary_text"), text("raw_content")]
                .into_iter()
                .find(|value| !value.is_empty())?;
            Some(Step::Thinking { text })
        }
        // Not a tool call and not the agent speaking: a warning Codex raised about the turn (a
        // model whose metadata it could not find, say). It reads as commentary, which is what it is.
        "error" => {
            let text = text("message");
            (!text.is_empty()).then_some(Step::Message { text })
        }
        _ => Some(tool_step(kind, item)),
    }
}

/// Every other item is something the agent *did*, rendered as a tool step.
fn tool_step(kind: &str, item: &Value) -> Step {
    let field = |name: &str| item.get(name).and_then(Value::as_str).unwrap_or_default();
    let (name, input, output) = match kind {
        "command_execution" => (
            "shell".to_string(),
            field("command").to_string(),
            field("aggregated_output").to_string(),
        ),
        "file_change" => (
            "apply_patch".to_string(),
            changed_paths(item.get("changes")),
            String::new(),
        ),
        "mcp_tool_call" => (
            match (field("server"), field("tool")) {
                ("", "") => "mcp_tool_call".to_string(),
                ("", tool) => tool.to_string(),
                (server, "") => server.to_string(),
                (server, tool) => format!("{server}.{tool}"),
            },
            item.get("arguments").map(compact_json).unwrap_or_default(),
            item.get("result").map(compact_json).unwrap_or_default(),
        ),
        "web_search" => (
            "web_search".to_string(),
            field("query").to_string(),
            String::new(),
        ),
        // A tool kind this was never taught. Named after itself and shown whole, because a step
        // nobody can read still says the agent did something — and a dropped one says it did not.
        other => (
            other.to_string(),
            compact_json_without_chrome(item),
            String::new(),
        ),
    };
    Step::Tool {
        name,
        input,
        status: tool_status(item),
        output,
    }
}

/// An item's lifecycle, read from `status` and — for a command — from the exit code it finished on.
fn tool_status(item: &Value) -> ToolStatus {
    let exited_badly = item
        .get("exit_code")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0);
    match item.get("status").and_then(Value::as_str) {
        Some("completed") if exited_badly => ToolStatus::Error,
        Some("completed") => ToolStatus::Ok,
        Some("failed" | "declined") => ToolStatus::Error,
        Some("in_progress") => ToolStatus::Running,
        // No status at all is an item that only ever arrives finished (a web search, a todo list) —
        // reporting it as still running would leave a live-looking call in a turn that is over.
        _ => {
            if exited_badly {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            }
        }
    }
}

/// `add /tmp/a.txt, update /tmp/b.txt` — what a patch did, in one line.
fn changed_paths(changes: Option<&Value>) -> String {
    changes
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .map(|change| {
                    let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
                    match change.get("kind").and_then(Value::as_str) {
                        Some(kind) => format!("{kind} {path}"),
                        None => path.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Lift the timeline's trailing [`Step::Message`] out of `steps`: the turn's last word is its
/// answer, and belongs in [`TurnContent::text`]. A message with a tool call after it is commentary
/// the agent wrote mid-turn and stays where it happened.
fn pop_trailing_message(steps: &mut Vec<Step>) -> Option<String> {
    match steps.last() {
        Some(Step::Message { .. }) => match steps.pop() {
            Some(Step::Message { text }) => Some(text),
            _ => None,
        },
        _ => None,
    }
}

/// `turn.completed` carries token counts and nothing else, so the reason is ours to supply — and it
/// has to be supplied. [`RunOutcome::is_reported`](crate::store::RunOutcome::is_reported) is false
/// for a metrics block that is only numbers, and `Agents::fail_over` reads *unreported* as the one
/// ending worth classifying: a clean run would have its own answer matched against the backend's
/// `limit_rules`, and an agent that so much as discusses a rate limit would take its working
/// backend out of the chain. Naming the ending is what says there is nothing to diagnose.
fn usage_metrics(usage: Option<&Value>) -> TurnMetrics {
    TurnMetrics {
        terminal_reason: Some("completed".to_string()),
        input_tokens: usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(Value::as_u64),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(Value::as_u64),
        ..TurnMetrics::default()
    }
}

fn compact_json(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// An unknown item as one line, without the two fields already shown as the step's own name and id.
fn compact_json_without_chrome(item: &Value) -> String {
    let Some(fields) = item.as_object() else {
        return compact_json(item);
    };
    let rest: serde_json::Map<String, Value> = fields
        .iter()
        .filter(|(key, _)| key.as_str() != "id" && key.as_str() != "type")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::to_string(&rest).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured live from `codex exec --json` (0.154.0): the agent speaks, patches a file, then
    /// answers.
    const PATCH_TURN: &str = concat!(
        r#"{"type":"thread.started","thread_id":"01a08d8f"}"#,
        "\n",
        r#"{"type":"turn.started"}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I'll create `hello.txt`."}}"#,
        "\n",
        r#"{"type":"item.started","item":{"id":"item_1","type":"file_change","changes":[{"path":"/w/hello.txt","kind":"add"}],"status":"in_progress"}}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_1","type":"file_change","changes":[{"path":"/w/hello.txt","kind":"add"}],"status":"completed"}}"#,
        "\n",
        r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"DONE"}}"#,
        "\n",
        r#"{"type":"turn.completed","usage":{"input_tokens":36300,"cached_input_tokens":30848,"output_tokens":65,"reasoning_output_tokens":0}}"#,
    );

    #[test]
    fn parses_messages_a_patch_and_usage_into_steps_text_and_metrics() {
        let content = parse(PATCH_TURN.as_bytes());
        assert_eq!(content.text, "DONE");
        assert_eq!(
            content.steps,
            [
                Step::Message {
                    text: "I'll create `hello.txt`.".into()
                },
                Step::Tool {
                    name: "apply_patch".into(),
                    input: "add /w/hello.txt".into(),
                    status: ToolStatus::Ok,
                    output: String::new(),
                },
            ]
        );
        let metrics = content.metrics.expect("metrics");
        assert_eq!(metrics.input_tokens, Some(36300));
        assert_eq!(metrics.output_tokens, Some(65));
        assert!(!metrics.is_error);
        // Named, so the run reads as `done` rather than `unknown` — and so `fail_over` leaves a
        // clean answer alone instead of matching it against the backend's limit rules.
        assert_eq!(metrics.terminal_reason.as_deref(), Some("completed"));
    }

    /// The whole point of the parser: Codex's stderr shares the log file with its stdout, so the
    /// answer has to survive being surrounded by chrome. This is the real thing — the banner, the
    /// prompt replay, a `tracing` line, the token trailer.
    #[test]
    fn the_engines_own_chrome_never_reaches_the_answer() {
        let log = concat!(
            "Reading additional input from stdin...\n",
            "2026-09-10T22:57:39.099018Z  INFO codex.exec{otel.kind=\"internal\"}: codex_http_client::custom_ca: using system root certificates\n",
            "OpenAI Codex v0.154.0\n",
            "--------\n",
            "workdir: /w\n",
            "model: gpt-6-astra\n",
            "session id: 01a08d8f\n",
            "--------\n",
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"the answer"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2}}"#,
            "\ntokens used\n5 181\n",
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "the answer");
        assert!(content.steps.is_empty());
    }

    /// A started command that never completed — the run was stopped mid-call. It stays one step,
    /// still running, and its command is readable.
    #[test]
    fn a_command_folds_its_start_and_end_into_one_step() {
        let started = r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        let done = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"/bin/zsh -lc ls","aggregated_output":"a\nb\n","exit_code":0,"status":"completed"}}"#;

        let running = parse(started.as_bytes());
        assert_eq!(
            running.steps,
            [Step::Tool {
                name: "shell".into(),
                input: "/bin/zsh -lc ls".into(),
                status: ToolStatus::Running,
                output: String::new(),
            }]
        );

        let finished = parse(format!("{started}\n{done}").as_bytes());
        assert_eq!(
            finished.steps,
            [Step::Tool {
                name: "shell".into(),
                input: "/bin/zsh -lc ls".into(),
                status: ToolStatus::Ok,
                output: "a\nb\n".into(),
            }]
        );
    }

    #[test]
    fn a_command_that_exits_nonzero_is_an_error_step() {
        let log = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"false","aggregated_output":"","exit_code":1,"status":"completed"}}"#;
        let Some(Step::Tool { status, .. }) = parse(log.as_bytes()).steps.first().cloned() else {
            panic!("expected a tool step");
        };
        assert_eq!(status, ToolStatus::Error);
    }

    /// Captured live: an unusable model. The provider's envelope arrives as a *string* inside
    /// `message`, and what a reader needs is the sentence in it — with the code and status kept for
    /// `limit_rules`, which is why the reason is not thrown away with the envelope.
    #[test]
    fn a_failed_turn_unwraps_the_providers_envelope_and_keeps_its_code() {
        let log = concat!(
            r#"{"type":"thread.started","thread_id":"01a08d8f"}"#,
            "\n",
            r#"{"type":"turn.started"}"#,
            "\n",
            r#"{"type":"error","message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'no-such-model' model is not supported.\"}}"}"#,
            "\n",
            r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'no-such-model' model is not supported.\"}}"}}"#,
        );
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "The 'no-such-model' model is not supported.");
        let metrics = content.metrics.expect("metrics");
        assert!(metrics.is_error);
        assert_eq!(
            metrics.terminal_reason.as_deref(),
            Some("invalid_request_error 400")
        );
    }

    /// A limit arriving as a plain sentence — no envelope to unwrap, and the words a backend's
    /// `limit_rules` match on reach `text` intact.
    #[test]
    fn a_plain_failure_message_survives_as_the_turns_text() {
        let log = r#"{"type":"turn.failed","error":{"type":"usage_limit_exceeded","message":"You've hit your usage limit. Try again in 4 hours."}}"#;
        let content = parse(log.as_bytes());
        assert_eq!(
            content.text,
            "You've hit your usage limit. Try again in 4 hours."
        );
        assert_eq!(
            content.metrics.expect("metrics").terminal_reason.as_deref(),
            Some("usage_limit_exceeded")
        );
    }

    /// A turn that answered and *then* died keeps both halves: the reader sees what was said, and
    /// `fail_over` still sees the words it classifies on.
    #[test]
    fn an_answer_before_a_failure_is_kept_alongside_it() {
        let log = concat!(
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"partway there"}}"#,
            "\n",
            r#"{"type":"turn.failed","error":{"type":"rate_limit_exceeded","message":"429 slow down"}}"#,
        );
        assert_eq!(parse(log.as_bytes()).text, "partway there\n\n429 slow down");
    }

    /// A log that is not this stream at all — a Codex that failed before it emitted an event, or a
    /// run recorded before `--json` was unconditional. The text is all there is, and it is kept.
    #[test]
    fn a_log_with_no_events_falls_back_to_its_text() {
        let log = "codex: command not found\n";
        let content = parse(log.as_bytes());
        assert_eq!(content.text, "codex: command not found");
        assert!(content.steps.is_empty());
        assert!(content.metrics.is_none());
    }

    /// The one line of narration `exec` writes on every run. Quoting the log verbatim is the last
    /// resort, and it should not spend its first line telling the operator about a pipe.
    #[test]
    fn the_fallback_drops_the_stdin_notice_and_keeps_the_reason() {
        let log = "Reading additional input from stdin...\n\
                   Not inside a trusted directory and --skip-git-repo-check was not specified.\n";
        let content = parse(log.as_bytes());
        assert_eq!(
            content.text,
            "Not inside a trusted directory and --skip-git-repo-check was not specified."
        );
    }

    /// JSON on stderr from something else in the process tree is not a thread event, and must not
    /// convince the parser it has a stream — that would answer with silence instead of the log.
    #[test]
    fn json_that_is_not_a_thread_event_does_not_count_as_a_stream() {
        let log = "{\"type\":\"assistant\",\"message\":{\"content\":[]}}\nplain trouble\n";
        assert_eq!(parse(log.as_bytes()).text, log.trim());
    }

    /// A tool kind added by a newer Codex is shown, not dropped.
    #[test]
    fn an_unknown_item_kind_becomes_a_step_named_after_itself() {
        let log = r#"{"type":"item.completed","item":{"id":"x1","type":"todo_list","items":[{"text":"ship","completed":true}],"status":"completed"}}"#;
        assert_eq!(
            parse(log.as_bytes()).steps,
            [Step::Tool {
                name: "todo_list".into(),
                input: r#"{"items":[{"completed":true,"text":"ship"}],"status":"completed"}"#
                    .into(),
                status: ToolStatus::Ok,
                output: String::new(),
            }]
        );
    }
}
