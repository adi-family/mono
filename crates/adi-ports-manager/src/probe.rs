//! Liveness probing: is a port actually free on the machine *right now*?
//!
//! The registry tracks what we have promised; this asks the OS what is real. We test
//! by attempting to bind loopback TCP — if the bind succeeds, nothing is listening and
//! the kernel will let us take it; if it fails (typically `AddrInUse`), the port is
//! occupied. This is a point-in-time answer: a port free now can be taken a moment
//! later (a TOCTOU race), which is why static leases are recorded rather than re-probed.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

/// True if `port` can be bound on loopback TCP at this instant.
///
/// A pure OS probe: it consults neither the registry nor any reserved bands. Port `0`
/// is treated as unavailable — it is the "any free port" sentinel, not a real port to
/// hand out.
#[must_use]
pub fn is_bindable(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    // Binding and immediately dropping the listener frees the port again. We ask for an
    // explicit port (not 0) so we are probing exactly this one.
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_port_is_not_bindable() {
        // Take an OS-chosen ephemeral port and hold it for the duration of the test.
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
