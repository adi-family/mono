//! What went past, written to the shared store — `llm_gateway_requests` in `db/global.db`.
//!
//! Rows are handed to a thread that owns the connection, so a request is never waiting on a write:
//! the client's answer has already been streamed by the time the row is queued, and a store held
//! by another writer for a moment cannot make a model call slower than it was.
//!
//! # What is deliberately not written
//!
//! Credentials. `authorization`, `x-api-key` and their neighbours are recorded as present and
//! nothing more ([`SENSITIVE`]) — a journal every agent on the machine can read is not a place to
//! keep the key it authenticated with. Bodies are kept whole up to the configured cap and marked
//! where they were cut, because a prompt truncated in silence reads as a prompt that was sent that
//! way.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::params;
use tracing::warn;

/// Header names whose values never reach the store.
const SENSITIVE: [&str; 6] = [
    "authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
    "proxy-authorization",
];

/// What a redacted value is replaced with — enough to prove the header was there.
const REDACTED: &str = "<redacted>";

/// The table, created on open so a fresh store needs no migration step.
const SCHEMA: &str = "create table if not exists llm_gateway_requests (
    id integer primary key,
    started_at integer not null,
    duration_ms integer,
    provider text not null,
    method text not null,
    target text not null,
    upstream text not null,
    status integer,
    model text,
    streamed integer not null default 0,
    client text,
    request_headers text not null,
    request_body text,
    response_headers text,
    response_body text,
    response_bytes integer,
    error text
);
create index if not exists llm_gateway_requests_started_at
    on llm_gateway_requests (started_at desc);";

/// One request's row, filled in as the exchange proceeds.
#[derive(Debug, Default)]
pub struct Entry {
    pub started_at: u64,
    pub duration_ms: u64,
    pub provider: String,
    pub method: String,
    pub target: String,
    pub upstream: String,
    pub status: Option<u16>,
    pub model: Option<String>,
    pub streamed: bool,
    pub client: Option<String>,
    pub request_headers: String,
    pub request_body: Option<String>,
    pub response_headers: Option<String>,
    pub response_body: Option<String>,
    pub response_bytes: u64,
    /// Set when the exchange never completed — a refused connection, a timeout, a broken client.
    pub error: Option<String>,
}

/// What the writing thread accepts: rows, and a request to say when it has caught up.
#[derive(Debug)]
enum Message {
    /// Boxed because a row is far larger than the acknowledgement beside it.
    Row(Box<Entry>),
    /// Answered once every row queued before it is written — the writer is FIFO, so that is all
    /// "flushed" can usefully mean.
    Flush(Sender<()>),
}

/// A handle onto the writing thread. Cloning it is how each connection gets one.
#[derive(Debug, Clone)]
pub struct Journal {
    tx: Sender<Message>,
}

impl Journal {
    /// Open the store, ensure the table, and start the writer.
    ///
    /// # Errors
    /// Fails if the global database cannot be opened or the table cannot be created — both worth
    /// refusing to start over, since a gateway that cannot journal is only a proxy.
    pub fn open() -> anyhow::Result<Self> {
        let conn = adi_db::Db::open().connect(None)?;
        conn.execute_batch(SCHEMA)?;

        let (tx, rx) = mpsc::channel::<Message>();
        std::thread::Builder::new()
            .name("llm-gateway-journal".into())
            .spawn(move || {
                // Ends when every sender is dropped, i.e. at shutdown.
                for message in rx {
                    match message {
                        Message::Row(entry) => {
                            if let Err(e) = insert(&conn, &entry) {
                                warn!(error = %e, "could not journal a request");
                            }
                        }
                        Message::Flush(ack) => drop(ack.send(())),
                    }
                }
            })?;

        Ok(Self { tx })
    }

    /// Queue a row. A failure here means the writer is gone, which is worth a line and nothing
    /// more: the exchange it describes has already happened.
    pub fn record(&self, entry: Entry) {
        if self.tx.send(Message::Row(Box::new(entry))).is_err() {
            warn!("the journal writer has stopped; a request went unrecorded");
        }
    }

    /// Block until everything queued so far is on disk, or `timeout` passes.
    ///
    /// Called on the way out: without it, a shutdown that arrives just after a long stream ends
    /// takes that row with it, and the one request most worth having is the last one.
    pub fn flush(&self, timeout: Duration) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Message::Flush(ack)).is_err() {
            return;
        }
        if let Err(RecvTimeoutError::Timeout) = done.recv_timeout(timeout) {
            warn!("the journal was still writing after {timeout:?}; some rows may be missing");
        }
    }
}

fn insert(conn: &rusqlite::Connection, entry: &Entry) -> rusqlite::Result<()> {
    conn.execute(
        "insert into llm_gateway_requests (
            started_at, duration_ms, provider, method, target, upstream, status, model,
            streamed, client, request_headers, request_body, response_headers, response_body,
            response_bytes, error
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            entry.started_at,
            entry.duration_ms,
            entry.provider,
            entry.method,
            entry.target,
            entry.upstream,
            entry.status,
            entry.model,
            i64::from(entry.streamed),
            entry.client,
            entry.request_headers,
            entry.request_body,
            entry.response_headers,
            entry.response_body,
            entry.response_bytes,
            entry.error,
        ],
    )?;
    Ok(())
}

/// Milliseconds since the epoch — the unit both timestamps in a row are in.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Headers as a JSON object, with credential values replaced.
///
/// A field repeated across lines (`set-cookie`) keeps the last of them; the journal is a record of
/// what was asked for, not a wire capture.
#[must_use]
pub fn headers_json(headers: &[(String, String)]) -> String {
    let map: serde_json::Map<String, serde_json::Value> = headers
        .iter()
        .map(|(field, value)| {
            let redact = SENSITIVE
                .iter()
                .any(|name| field.eq_ignore_ascii_case(name));
            let value = if redact { REDACTED } else { value.as_str() };
            (field.to_ascii_lowercase(), serde_json::Value::from(value))
        })
        .collect();
    serde_json::Value::Object(map).to_string()
}

/// A body as text for the journal: UTF-8 kept as-is up to `cap`, anything else described.
#[must_use]
pub fn body_text(body: &[u8], cap: usize) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return Some(format!("<{} bytes, not UTF-8>", body.len()));
    };
    if text.len() <= cap {
        return Some(text.to_string());
    }
    // Cut on a character boundary, not a byte one, or the row is invalid UTF-8 itself.
    let end = (0..=cap).rev().find(|i| text.is_char_boundary(*i))?;
    Some(format!(
        "{}\n<truncated: {} of {} bytes kept>",
        &text[..end],
        end,
        text.len()
    ))
}

/// The `model` and `stream` fields out of a request body, when it is the JSON both the Anthropic
/// and the `OpenAI` wire formats send. Neither is interpreted — they are the two columns worth
/// having in a table somebody will read with `where`.
#[must_use]
pub fn model_and_stream(body: &[u8]) -> (Option<String>, bool) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (None, false);
    };
    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .map(ToString::to_string);
    let stream = value
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (model, stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_recorded_as_present_and_not_as_values() {
        let headers = vec![
            ("X-Api-Key".to_string(), "sk-secret".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let json = headers_json(&headers);
        assert!(json.contains("<redacted>"), "{json}");
        assert!(!json.contains("sk-secret"), "{json}");
        assert!(json.contains("application/json"), "{json}");
    }

    #[test]
    fn a_long_body_is_cut_on_a_character_boundary_and_says_so() {
        let body = "é".repeat(100);
        let text = body_text(body.as_bytes(), 33).unwrap();
        assert!(text.contains("<truncated: 32 of 200 bytes kept>"), "{text}");
        assert!(text.starts_with("éé"), "{text}");
    }

    #[test]
    fn reads_model_and_stream_off_either_wire_format() {
        let (model, stream) =
            model_and_stream(br#"{"model":"claude-opus-5","stream":true,"messages":[]}"#);
        assert_eq!(model.as_deref(), Some("claude-opus-5"));
        assert!(stream);
        assert_eq!(model_and_stream(b"not json"), (None, false));
    }
}
