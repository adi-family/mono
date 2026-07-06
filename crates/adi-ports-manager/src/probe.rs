//! Liveness probing: is a port actually free on the machine *right now*? Tests by
//! attempting to bind loopback TCP — a point-in-time answer (a TOCTOU race), which is
//! why static leases are recorded rather than re-probed.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

/// True if `port` can be bound on loopback TCP at this instant.
#[must_use]
pub fn is_bindable(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_port_is_not_bindable() {
        let held =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind ephemeral");
        let port = held.local_addr().expect("local addr").port();
        assert!(
            !is_bindable(port),
            "port {port} is held, must not be bindable"
        );
    }

    #[test]
    fn port_zero_is_never_bindable() {
        assert!(!is_bindable(0));
    }
}
