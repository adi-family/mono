//! The `/api/*` server surface: the real backend over the [`adi_ports_manager`] port
//! registry. Each handler returns `(status, json_body)`; the host ([`adi-app`](../adi-app))
//! owns the socket and writes the response. Compiled only with the `server` feature,
//! which pulls in the filesystem-backed registry and so is native-only.

use std::time::Instant;

use adi_ports_manager::Ports;

use crate::types::{
    ApiError, Health, Lease, LeaseRef, PortsState, Range, ReleaseResponse, ReserveResponse,
    UsedPort, UsedPorts,
};

/// `GET /api/health` — liveness plus identity and uptime. The host supplies its own
/// `service`/`version` so the reported identity is the app's, not this library's.
#[must_use]
pub fn health(service: &str, version: &str, start: Instant) -> (u16, String) {
    ok_json(&Health {
        ok: true,
        service: service.to_string(),
        version: version.to_string(),
        uptime_secs: start.elapsed().as_secs(),
    })
}

/// `GET /api/ports` — the allocator's configuration and current static leases.
#[must_use]
pub fn ports(manager: &Ports) -> (u16, String) {
    let config = manager.config();
    let range = Range {
        start: *config.range.start(),
        end: *config.range.end(),
    };
    let reserved = config
        .reserved
        .iter()
        .map(|band| Range {
            start: *band.start(),
            end: *band.end(),
        })
        .collect();

    match manager.leases() {
        Ok(leases) => {
            let leases = leases
                .into_iter()
                .map(|l| Lease {
                    service: l.service,
                    key: l.key,
                    port: l.port,
                })
                .collect();
            ok_json(&PortsState {
                range,
                reserved,
                leases,
            })
        }
        Err(e) => error(500, &format!("reading port registry: {e}")),
    }
}

/// `GET /api/ports/used` — the machine's listening TCP ports. The system scan is done by
/// the host (it's platform I/O); this just shapes the response.
#[must_use]
pub fn used_ports(ports: Vec<UsedPort>) -> (u16, String) {
    ok_json(&UsedPorts { ports })
}

/// `POST /api/ports/reserve` — reserve (or return the existing) static port for a pair.
#[must_use]
pub fn reserve(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return bad_lease_ref();
    };
    match manager.reserve(&req.service, &req.key) {
        Ok(port) => ok_json(&ReserveResponse {
            service: req.service,
            key: req.key,
            port,
        }),
        Err(e) => error(500, &format!("reserving port: {e}")),
    }
}

/// `POST /api/ports/release` — release a static lease, reporting the freed port.
#[must_use]
pub fn release(manager: &Ports, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_lease_ref(body) else {
        return bad_lease_ref();
    };
    match manager.release(&req.service, &req.key) {
        Ok(freed) => ok_json(&ReleaseResponse {
            service: req.service,
            key: req.key,
            freed,
        }),
        Err(e) => error(500, &format!("releasing port: {e}")),
    }
}

/// A JSON error body paired with its status.
#[must_use]
pub fn error(status: u16, message: &str) -> (u16, String) {
    let body = serde_json::to_string(&ApiError::new(message))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"internal error"}"#.to_string());
    (status, body)
}

/// Serialize a success payload; a serialization failure degrades to a 500 error body.
fn ok_json<T: serde::Serialize>(value: &T) -> (u16, String) {
    match serde_json::to_string(value) {
        Ok(json) => (200, json),
        Err(e) => error(500, &format!("serializing response: {e}")),
    }
}

fn bad_lease_ref() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"service\": \"…\", \"key\": \"…\" }",
    )
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
            "adi-webapp-api-{}-{:?}/registry.json",
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
    fn health_reports_ok_and_identity() {
        let (status, body) = health("adi-app", "1.2.3", Instant::now());
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "adi-app");
        assert_eq!(v["version"], "1.2.3");
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
