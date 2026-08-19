use serde::de::DeserializeOwned;

use crate::types::ApiError;

/// Read a `POST` body into its request type — `None` when it isn't the JSON the endpoint asked
/// for.
///
/// Prefer [`require`], which also turns away a body that parses but names nothing and answers
/// with the message the type itself carries. This is the raw decode, for the few endpoints whose
/// request type has no single fixed shape.
pub(crate) fn parse_body<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// A `POST` body an endpoint can insist on: what to decode into, what counts as complete, and the
/// sentence the 400 says when it isn't.
///
/// The check and the message are one item on purpose. They used to be a `parse_x`/`bad_x` pair per
/// endpoint, and a pair drifts — add a field to the check and the sentence still lists the old
/// shape, so the caller is told "expected { a, b }" about a body that had both.
pub(crate) trait FromBody: DeserializeOwned + Sized {
    /// The whole 400 message: the body shape, plus anything else worth saying about it.
    const EXPECTED: &'static str;

    /// Whether a decoded body names everything the endpoint needs. Decoding alone by default —
    /// a type all of whose fields are required and non-blank has nothing left to check.
    fn is_complete(&self) -> bool {
        true
    }

    /// What an *empty* body means, for the endpoints that accept one: the file browser and the
    /// database panel both open on the root by sending nothing at all. `None` — the default —
    /// answers [`Self::EXPECTED`] instead.
    fn on_empty() -> Option<Self> {
        None
    }
}

/// Decode a `POST` body into `T`, or the 400 that says what `T` wanted.
///
/// A body that isn't JSON, isn't this shape, or is missing something [`FromBody::is_complete`]
/// insists on all come back as the same message: the caller needs to be told what to send, and
/// that is the same sentence either way.
pub(crate) fn require<T: FromBody>(body: &[u8]) -> Result<T, Response> {
    if body.iter().all(u8::is_ascii_whitespace)
        && let Some(empty) = T::on_empty()
    {
        return Ok(empty);
    }
    parse_body::<T>(body)
        .filter(T::is_complete)
        .ok_or_else(|| error(400, T::EXPECTED))
}

/// The shape of nearly every mutation endpoint: decode the body, do the one thing it names, and
/// answer with `fresh` — the endpoint's own list — so the client refreshes in one round-trip.
///
/// `op` takes the request by value, so it can hand the store the request's `String` fields
/// without cloning them. A store error becomes a response through the module's
/// `impl From<&…Error> for Response`, which is what decides the status.
pub(crate) fn mutate<T, R, E>(
    body: &[u8],
    op: impl FnOnce(T) -> Result<R, E>,
    fresh: impl FnOnce() -> Response,
) -> Response
where
    T: FromBody,
    for<'a> Response: From<&'a E>,
{
    let req = match require::<T>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    match op(req) {
        Ok(_) => fresh(),
        Err(e) => Response::from(&e),
    }
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
