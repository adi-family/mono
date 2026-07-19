use adi_ports_manager::Ports;

use crate::types::{
    Lease, LeaseRef, PortsState, Range, ReleaseResponse, ReserveResponse, UsedPort, UsedPorts,
};

use super::response::{Response, error, ok_json};

/// `GET /api/ports` — the allocator's configuration and current static leases.
#[must_use]
pub fn ports(manager: &Ports) -> Response {
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
pub fn used_ports(ports: Vec<UsedPort>) -> Response {
    ok_json(&UsedPorts { ports })
}

/// `POST /api/ports/reserve` — reserve (or return the existing) static port for a pair.
#[must_use]
pub fn reserve(manager: &Ports, body: &[u8]) -> Response {
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
pub fn release(manager: &Ports, body: &[u8]) -> Response {
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

// MARK: projects — metadata manifests under ~/.adi/mono/projects

fn bad_lease_ref() -> Response {
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
