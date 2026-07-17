use crate::types::ApiError;

/// Trim a string, dropping it entirely when blank (so an empty optional field clears).
pub(crate) fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

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
pub(crate) fn ok_json<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(json) => Response {
            status: 200,
            body: json,
        },
        Err(e) => error(500, &format!("serializing response: {e}")),
    }
}
