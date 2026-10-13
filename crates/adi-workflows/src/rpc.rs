//! The wire protocol between the Rust host and one attempt's `bun` process — line-delimited
//! JSON, one request per `ctx` checkpoint, read off the child's stdout; one reply per request,
//! written to its stdin. See `@adi/workflows`' own `createCtx` doc for the TS side of this same
//! contract, and [`crate::attempt`] for who drives it.

use serde::Deserialize;
use serde_json::Value;

/// One line read off an attempt's stdout — either a checkpoint asking for a reply, or one of the
/// three terminal facts that end the attempt with no reply expected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    #[serde(rename = "step")]
    Step { key: String },
    #[serde(rename = "step_save")]
    StepSave { key: String, value: Value },
    #[serde(rename = "task")]
    Task {
        key: String,
        title: String,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        person: Option<String>,
        #[serde(default)]
        instructions: Option<String>,
        #[serde(default)]
        input: Option<Value>,
        #[serde(default, rename = "outputSchema")]
        output_schema: Option<Value>,
        #[serde(default, rename = "timeoutMs")]
        timeout_ms: Option<u64>,
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    #[serde(rename = "run")]
    Run {
        key: String,
        workflow: String,
        #[serde(default)]
        input: Option<Value>,
    },
    #[serde(rename = "waitFor")]
    WaitFor {
        key: String,
        event: String,
        #[serde(default, rename = "where")]
        where_clause: Option<serde_json::Map<String, Value>>,
        #[serde(default, rename = "timeoutMs")]
        timeout_ms: Option<u64>,
    },
    #[serde(rename = "sleep")]
    Sleep { key: String, until: i64 },
    #[serde(rename = "ask")]
    Ask {
        key: String,
        person: String,
        text: String,
    },
    /// Terminal: the workflow body returned.
    #[serde(rename = "result")]
    Result { value: Value },
    /// Terminal: an uncaught error (a bad `input`/`output`, a reused key, a plain exception).
    #[serde(rename = "error")]
    Error { message: String },
    /// Terminal: an uncaught `TaskGaveUp`.
    #[serde(rename = "gave_up")]
    GaveUp { reason: String },
    /// Terminal: a checkpoint has nothing resolved yet.
    #[serde(rename = "suspend")]
    Suspend {},
}

impl Request {
    /// Whether this line ends the attempt (true) or is a checkpoint awaiting a reply (false).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Request::Result { .. } | Request::Error { .. } | Request::GaveUp { .. } | Request::Suspend {}
        )
    }
}
