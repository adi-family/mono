//! Following this machine across networks: re-publishing its ticket, and getting the pool off the
//! relay, when the network under a long-lived daemon changes.
//!
//! The daemon binds one endpoint for the whole life of the process (the control panel's, which is
//! days), and the gateway keeps one pooled connection per peer for as long as it stays open. Both
//! were fine until the laptop changed Wi-Fi: observed 2026-10-20, a move from `192.168.0.x` to
//! `192.168.1.x` (and IPv6 appearing) left `/api/mesh` publishing the old LAN address with no IPv6,
//! and the pooled connection to a node riding the relay at ~180–360 KB/s — while a freshly bound
//! endpoint on the same network hole-punched direct, over both families, in half a second. The
//! only cure was restarting `adi-app`.
//!
//! ## What iroh already does, and why it is not enough
//!
//! iroh 1.0.3 watches the network itself: its socket actor runs `netwatch`'s interface monitor
//! (a route socket on macOS, netlink on Linux, `NotifyUnicastIpAddressChange` and
//! `NotifyRouteChange` on Windows), and on a major change it rebinds its UDP sockets, re-checks
//! the relay, re-runs its address discovery and tells every peer's state actor to re-hole-punch
//! *inside* the existing connection. It re-tries
//! that every minute while a connection has no fast direct path. So this module rebinds nothing and
//! watches no OS API of its own — that would be a second monitor, per platform, racing the first.
//!
//! What iroh does not do is anything outside the endpoint: the published ticket is ours, written
//! once, and in the field the in-place re-hole-punch on a days-old connection did not bring the
//! direct path back where a new connection did. So this module adds the two things that were
//! missing, and nothing else:
//!
//! 1. **The ticket follows the address.** [`Endpoint::watch_addr`] is iroh's own answer to "what
//!    are my addresses now", fed by that monitor *and* by its periodic re-discovery — so it moves
//!    even on a change the interface monitor missed. Every settled change is re-published.
//! 2. **Relay-only connections from before a move are retired.** [`RECYCLE_GRACE`] after the direct
//!    addresses change, every pooled connection that is still relay-only and was dialled before the
//!    change is dropped from the pool ([`Gateway::retire_relayed`]), and the next request dials
//!    fresh. The grace is iroh's chance to fix it in place first; a connection that went direct in
//!    the meantime is left alone.
//!
//! ## Why a signal plus a slow sweep, rather than either alone
//!
//! The address watch is a signal, so recovery starts within seconds of a move instead of on a
//! timer. But it only sees *this* side move: a node that changed networks, or an in-place
//! hole-punch that wedged for a reason nobody detected, changes nothing here. So every
//! [`SWEEP_INTERVAL`] the same retirement runs for connections that have been relay-only for at
//! least that long. A peer that genuinely cannot be reached directly — both ends behind symmetric
//! NATs, a browser — costs one handshake over the relay per interval, and only while it is in use.
//!
//! [`Endpoint::network_change`] is also called on a move. iroh documents it as harmless when it
//! already noticed; when the change reached us through re-discovery instead of the monitor, it is
//! what gets the monitor to look again.
//!
//! The browser client (`adi-mesh-client`) needs none of this: it is relay-only by construction and
//! its websocket reconnects itself. The iOS viewer (`adi-mesh-ffi`) already throws its whole pool
//! away on every foregrounding.

use std::sync::Arc;
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, Watcher as _};
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::gateway::Gateway;
use crate::ticket;

/// How long the address has to stay put before it counts as the new one. Interfaces come up in
/// steps — a v4 lease, then v6 router advertisements, then iroh's own discovery reporting the
/// public mapping — and each step moves the address once.
const SETTLE: Duration = Duration::from_secs(2);

/// How long after a move iroh gets to re-hole-punch the existing connections before the
/// relay-only ones are retired. Its own retry interval is five seconds; this is a few of them.
const RECYCLE_GRACE: Duration = Duration::from_secs(15);

/// How often relay-only connections are retired with no move to prompt it, and how long one has
/// to have been pooled to qualify. See the module docs for why this exists at all.
const SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Publish this machine's ticket, and keep it — and the pool — current as the network changes,
/// until `shutdown` flips or the endpoint closes.
///
/// `first` is the address the ticket starts from: the daemon waits for a home relay before it
/// calls this, so the first ticket is reachable off the LAN.
pub async fn follow(
    endpoint: Endpoint,
    first: EndpointAddr,
    gateway: Arc<Gateway>,
    shutdown: watch::Receiver<bool>,
) {
    // [`crate::Daemon::stop`] clears the ticket before it waits for this task, so a ticket written
    // after the signal would advertise a daemon that is gone. Hence the check here, and `biased`.
    if *shutdown.borrow() {
        return;
    }
    let mut published = first;
    publish(&published);
    let mut addrs = endpoint.watch_addr();
    let mut next = Recycle::periodic(Instant::now());
    // A second receiver for the `select!` to wait on, so the arms below can still read the first.
    let mut stopping = shutdown.clone();
    loop {
        tokio::select! {
            biased;
            // `.map`: the guard `wait_for` returns is not `Send`, and the arms below await.
            _ = async { stopping.wait_for(|stop| *stop).await.map(|_| ()) } => return,
            changed = addrs.updated() => {
                if changed.is_err() {
                    debug!("roam: the endpoint is gone; nothing left to follow");
                    return;
                }
                // Drain the burst: keep taking updates until one `SETTLE` passes without any.
                while let Ok(Ok(_)) = tokio::time::timeout(SETTLE, addrs.updated()).await {}
                let current = addrs.get();
                if current == published || *shutdown.borrow() {
                    continue;
                }
                let now = Instant::now();
                if moved(&published, &current) {
                    info!(addrs = ?current.addrs, "roam: this machine's direct addresses changed");
                    endpoint.network_change().await;
                    next = Recycle::after_move(now);
                }
                publish(&current);
                published = current;
            }
            () = tokio::time::sleep_until(next.at) => {
                let retired = gateway.retire_relayed(next.dialled_before);
                if retired > 0 {
                    info!(retired, "roam: retired relay-only connections; the next request re-dials");
                }
                next = Recycle::periodic(Instant::now());
            }
        }
    }
}

/// Did the direct (IP) half of the address change? A relay-only change — a new home relay — is
/// re-published but is not a move: it says nothing about the paths to peers.
fn moved(before: &EndpointAddr, after: &EndpointAddr) -> bool {
    !before.ip_addrs().eq(after.ip_addrs())
}

/// The next retirement: when it runs, and the dial time a connection must predate to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Recycle {
    at: Instant,
    dialled_before: Instant,
}

impl Recycle {
    /// The slow sweep: [`SWEEP_INTERVAL`] from `now`, for connections at least that old by then.
    fn periodic(now: Instant) -> Self {
        Self {
            at: now + SWEEP_INTERVAL,
            dialled_before: now,
        }
    }

    /// After a move at `now`: [`RECYCLE_GRACE`] from it, for every connection dialled before it.
    /// Replaces whatever was scheduled — a second move inside the grace pushes it out, and its
    /// later cut-off covers everything the first one would have.
    fn after_move(now: Instant) -> Self {
        Self {
            at: now + RECYCLE_GRACE,
            dialled_before: now,
        }
    }
}

/// Encode and publish `addr` as this machine's ticket.
fn publish(addr: &EndpointAddr) {
    match ticket::encode(addr) {
        Ok(token) => {
            if let Err(e) = ticket::publish(&token) {
                warn!(error = %e, "could not persist this machine's ticket");
            }
            info!(ticket = %token, "share this ticket with a peer to reach this machine");
        }
        Err(e) => warn!(error = %e, "could not encode this machine's ticket"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(id: iroh::EndpointId, ips: &[&str]) -> EndpointAddr {
        ips.iter().fold(EndpointAddr::new(id), |addr, ip| {
            addr.with_ip_addr(ip.parse().expect("socket addr"))
        })
    }

    /// The observed move: a new LAN address, and IPv6 appearing.
    #[test]
    fn a_new_lan_address_is_a_move() {
        let id = iroh::SecretKey::generate().public();
        let before = addr(id, &["192.168.0.121:51000"]);
        let after = addr(id, &["192.168.1.221:51000", "[2a00:d180::1]:51001"]);
        assert!(moved(&before, &after));
        assert!(!moved(&after, &after.clone()), "the same set is not a move");
    }

    #[test]
    fn a_relay_change_alone_is_not_a_move() {
        let id = iroh::SecretKey::generate().public();
        let before = addr(id, &["192.168.1.221:51000"]);
        let relay: iroh::RelayUrl = "https://relay.example.com".parse().expect("url");
        let after = before.clone().with_relay_url(relay);
        assert_ne!(before, after, "it is still re-published");
        assert!(!moved(&before, &after));
    }

    #[test]
    fn a_move_retires_sooner_and_covers_everything_dialled_before_it() {
        let now = Instant::now();
        let after = Recycle::after_move(now);
        assert!(after.at < Recycle::periodic(now).at);
        assert_eq!(after.dialled_before, now);
        assert_eq!(after.at - now, RECYCLE_GRACE);
    }

    /// The periodic cut-off is the sweep's own start, so by the time it runs a retired
    /// connection has been pooled for at least one whole interval.
    #[test]
    fn the_periodic_sweep_only_takes_connections_older_than_an_interval() {
        let now = Instant::now();
        let sweep = Recycle::periodic(now);
        assert_eq!(sweep.at - sweep.dialled_before, SWEEP_INTERVAL);
    }
}
