use std::time::Instant;

use crate::types::Health;

use super::response::{Response, ok_json};

/// `GET /api/health` — liveness plus identity and uptime. The host supplies its own
/// `service`/`version` so the reported identity is the app's, not this library's.
#[must_use]
pub fn health(service: &str, version: &str, start: Instant) -> Response {
    ok_json(&Health {
        ok: true,
        service: service.to_string(),
        version: version.to_string(),
        uptime_secs: start.elapsed().as_secs(),
    })
}
