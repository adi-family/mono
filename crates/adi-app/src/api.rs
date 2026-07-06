//! The `/api/*` surface: a real backend over the adi platform's live state. It reads
//! and mutates the [`adi_ports_manager`] port registry, so the SPA is driving actual
//! platform state, not a mock.
//!
//! Each handler returns `(status, json_body)`. The server ([`crate::main`]) owns the
//! socket; these stay pure and testable.

use std::time::Instant;

use adi_ports_manager::Ports;
use serde::Deserialize;
use serde_json::{Value, json};

/// This crate's version, surfaced at `/api/health`.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A `(service, key)` pair naming a static port lease — the body of the reserve/release
/// endpoints.
#[derive(Debug, Deserialize)]
struct LeaseRef {
    service: String,
    key: String,
}

/// `GET /api/health` — liveness plus identity and uptime.
#[must_use]
pub fn health(start: Instant) -> (u16, String) {
    let body = json!({
        "ok": true,
        "service": "adi-app",
        "version": VERSION,
        "uptime_secs": start.elapsed().as_secs(),
    });
    (200, body.to_string())
}

/// `GET /api/ports` — the allocator's configuration and current static leases.
#[must_use]
pub fn ports(manager: &Ports) -> (u16, String) {
    let config = manager.config();
    let range = json!({ "start": *config.range.start(), "end": *config.range.end() });
    let reserved: Vec<Value> = config
        .reserved
        .iter()
        .map(|band| json!({ "start": *band.start(), "end": *band.end() }))
        .collect();

    match manager.leases() {
        Ok(leases) => {
            let body = json!({ "range": range, "reserved": reserved, "leases": leases });
            (200, body.to_string())
        }
        Err(e) => error(500, &format!("reading port registry: {e}")),
    }
}

/// `POST /api/ports/reserve` — reserve (or return the existing) static port for a
/// `(service, key)`.
#[must_use]
pub fn reserve(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return error(
            400,
            "expected JSON body { \"service\": \"…\", \"key\": \"…\" }",
        );
    };
    match manager.reserve(&req.service, &req.key) {
        Ok(port) => (
            200,
            json!({ "service": req.service, "key": req.key, "port": port }).to_string(),
        ),
        Err(e) => error(500, &format!("reserving port: {e}")),
    }
}

/// `POST /api/ports/release` — release a static lease, reporting the freed port (or
/// `null` if there was none).
#[must_use]
pub fn release(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return error(
            400,
            "expected JSON body { \"service\": \"…\", \"key\": \"…\" }",
        );
    };
    match manager.release(&req.service, &req.key) {
        Ok(freed) => (
            200,
            json!({ "service": req.service, "key": req.key, "freed": freed }).to_string(),
        ),
        Err(e) => error(500, &format!("releasing port: {e}")),
    }
}

/// A JSON error body paired with its status.
#[must_use]
pub fn error(status: u16, message: &str) -> (u16, String) {
    (status, json!({ "ok": false, "error": message }).to_string())
}

fn parse_lease_ref(body: &[u8]) -> Option<LeaseRef> {
    let req: LeaseRef = serde_json::from_slice(body).ok()?;
    if req.service.trim().is_empty() || req.key.trim().is_empty() {
        return None;
    }
    Some(req)
}

#[cfg(test)]
mod tests {
    use adi_ports_manager::Config;
    use serde_json::Value;

    use super::*;

    fn temp_manager() -> Ports {
        // Isolated registry per test so we never touch the real one.
        let path = std::env::temp_dir().join(format!(
            "adi-app-api-{}-{:?}/registry.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        Ports::with_config(Config {
            registry_path: path,
            ..Config::default()
        })
    }

    #[test]
    fn health_reports_ok_and_version() {
        let (status, body) = health(Instant::now());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "adi-app");
        assert!(v["version"].is_string());
    }

    #[test]
    fn reserve_then_ports_lists_the_lease() {
        let m = temp_manager();
        let (status, body) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let reserved: Value = serde_json::from_str(&body).unwrap();
        let port = reserved["port"].as_u64().unwrap();

        let (status, body) = ports(&m);
        assert_eq!(status, 200);
        let listed: Value = serde_json::from_str(&body).unwrap();
        let leases = listed["leases"].as_array().unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0]["service"], "web");
        assert_eq!(leases[0]["port"].as_u64().unwrap(), port);
    }

    #[test]
    fn reserve_is_idempotent_over_the_api() {
        let m = temp_manager();
        let (_, first) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let (_, again) = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let a: Value = serde_json::from_str(&first).unwrap();
        let b: Value = serde_json::from_str(&again).unwrap();
        assert_eq!(a["port"], b["port"]);
    }

    #[test]
    fn release_frees_the_lease() {
        let m = temp_manager();
        let _ = reserve(&m, br#"{"service":"web","key":"http"}"#);
        let (status, body) = release(&m, br#"{"service":"web","key":"http"}"#);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["freed"].is_number());

        let (_, body) = ports(&m);
        let listed: Value = serde_json::from_str(&body).unwrap();
        assert!(listed["leases"].as_array().unwrap().is_empty());
    }

    #[test]
    fn bad_body_is_a_400() {
        let m = temp_manager();
        assert_eq!(reserve(&m, b"not json").0, 400);
        assert_eq!(reserve(&m, br#"{"service":"","key":"x"}"#).0, 400);
    }
}
