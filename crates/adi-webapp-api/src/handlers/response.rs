use serde::de::DeserializeOwned;

use crate::types::ApiError;

/// Read a `POST` body into its request type — `None` when it isn't the JSON the endpoint asked
/// for.
///
/// Chain [`Option::filter`] to also turn away a body that parses but names nothing: a blank id,
/// an empty title, a port of zero. Either way the handler answers with the `bad_*` message that
/// spells out what it wanted, so the two failures need no telling apart here.
pub(crate) fn parse_body<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

pub(crate) use adi_config::clean;

/// An HTTP response: a status paired with its (JSON) body. Handlers build one exclusively
/// through [`error`], [`ok_json`], and the `From<&…Error>` impls.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// A JSON error body paired with its status.
#[must_use]
pub fn error(status: u16, message: &str) -> Response {
    let body = serde_json::to_string(&ApiError::new(message))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"internal error"}"#.to_string());
    Response { status, body }
}

/// Serialize a success payload; a serialization failure degrades to a 500 error body.
///
/// Public because a handler is not always in this crate: `POST /api/dashboards/transfer` lives in
/// adi-app (it is the one endpoint that calls *out* to another machine) and must answer with the
/// same shape everything else does.
#[must_use]
pub fn ok_json<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(json) => Response {
            status: 200,
            body: json,
        },
        Err(e) => error(500, &format!("serializing response: {e}")),
    }
}
