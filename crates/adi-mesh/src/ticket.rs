//! The shareable token that tells a peer how to reach this machine.
//!
//! Dialing by a bare [`EndpointId`] works but leans on public DNS discovery, whose
//! propagation is racy (n0/iroh issue #3713). A **ticket** instead carries the endpoint's
//! [`EndpointAddr`] — id **plus** its relay URL and direct socket addresses — so the peer
//! dials it straight away: directly on a shared LAN, relay-assisted across NATs, no DNS
//! wait. The wire form is `adimesh:<hex(json(EndpointAddr))>` — one copy-pasteable,
//! shell-safe token.
//!
//! A forward target may be either a ticket or a bare id; [`parse_target`] accepts both.

use adi_config::Config;
use anyhow::Context as _;
use iroh::{EndpointAddr, EndpointId, TransportAddr};

/// Marks a string as an adimesh ticket (vs. a bare id).
const PREFIX: &str = "adimesh:";

/// The raw file the running daemon publishes its current ticket to, so other tools (the
/// control panel) can display it without binding an endpoint of their own.
const TICKET_FILE: &str = "ticket";

/// Encode an endpoint address as a shareable ticket string.
///
/// # Errors
/// Fails only if the address cannot be serialized (not expected in practice).
pub fn encode(addr: &EndpointAddr) -> anyhow::Result<String> {
    let json = serde_json::to_vec(addr).context("serializing the endpoint address")?;
    Ok(format!("{PREFIX}{}", to_hex(&json)))
}

/// Decode a ticket string back into an endpoint address.
///
/// # Errors
/// Fails if the string is not a `adimesh:` ticket, is not valid hex, or does not decode
/// to an [`EndpointAddr`].
pub fn decode(token: &str) -> anyhow::Result<EndpointAddr> {
    let hex = token
        .trim()
        .strip_prefix(PREFIX)
        .context("not an adimesh ticket")?;
    let bytes = from_hex(hex).context("ticket payload is not valid hex")?;
    serde_json::from_slice(&bytes).context("ticket does not decode to an endpoint address")
}

/// Resolve a forward target — a ticket **or** a bare [`EndpointId`] — to an address to
/// dial. A bare id yields an address with no hints, so the connection relies on discovery.
///
/// # Errors
/// Fails if the token is neither a valid ticket nor a parseable endpoint id.
pub fn parse_target(token: &str) -> anyhow::Result<EndpointAddr> {
    let token = token.trim();
    if token.starts_with(PREFIX) {
        return decode(token);
    }
    let id: EndpointId = token.parse().map_err(|e| {
        anyhow::anyhow!("target is neither an adimesh ticket nor an endpoint id: {e}")
    })?;
    Ok(EndpointAddr::new(id))
}

/// The [`EndpointId`] a target names — for authorization checks and default labels.
///
/// # Errors
/// Fails if the token does not parse (see [`parse_target`]).
pub fn target_id(token: &str) -> anyhow::Result<EndpointId> {
    Ok(parse_target(token)?.id)
}

/// Does this address carry a relay URL (i.e. is it reachable off the local network)?
#[must_use]
pub fn has_relay(addr: &EndpointAddr) -> bool {
    addr.addrs
        .iter()
        .any(|a| matches!(a, TransportAddr::Relay(_)))
}

/// Publish `token` as this machine's current ticket, so tools that don't run an endpoint
/// (e.g. the control panel) can show it. The daemon calls this once its relay is known.
///
/// # Errors
/// Fails on any store write error.
pub fn publish(token: &str) -> anyhow::Result<()> {
    Config::open()
        .module(crate::config::MODULE)
        .write_raw(TICKET_FILE, token.as_bytes())?;
    Ok(())
}

/// The ticket the running daemon last published, if any. Its presence also stands in for
/// "the daemon is up" (it is cleared on a clean shutdown).
#[must_use]
pub fn published() -> Option<String> {
    let bytes = Config::open()
        .module(crate::config::MODULE)
        .read_raw(TICKET_FILE)
        .ok()??;
    let token = String::from_utf8(bytes).ok()?;
    let token = token.trim().to_string();
    if token.is_empty() { None } else { Some(token) }
}

/// Remove the published ticket — called on daemon shutdown so a stopped daemon shows none.
pub fn clear_published() {
    let _ = Config::open()
        .module(crate::config::MODULE)
        .remove_raw(TICKET_FILE);
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.trim();
    anyhow::ensure!(s.len().is_multiple_of(2), "odd-length hex string");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("invalid hex byte"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_addr() -> EndpointAddr {
        let id = iroh::SecretKey::generate().public();
        EndpointAddr::new(id).with_ip_addr("127.0.0.1:45080".parse().unwrap())
    }

    #[test]
    fn hex_round_trips_arbitrary_bytes() {
        let bytes = [0u8, 1, 15, 16, 127, 128, 254, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert!(from_hex("abc").is_err(), "odd length rejected");
        assert!(from_hex("zz").is_err(), "non-hex rejected");
    }

    #[test]
    fn ticket_round_trips_an_address() {
        let addr = sample_addr();
        let token = encode(&addr).expect("encode");
        assert!(token.starts_with(PREFIX));
        assert_eq!(decode(&token).expect("decode"), addr);
    }

    #[test]
    fn parse_target_accepts_a_ticket_and_a_bare_id() {
        let addr = sample_addr();
        let token = encode(&addr).expect("encode");
        assert_eq!(parse_target(&token).expect("ticket").id, addr.id);

        let id = iroh::SecretKey::generate().public();
        let from_id = parse_target(&id.to_string()).expect("bare id");
        assert_eq!(from_id.id, id);
        assert!(from_id.addrs.is_empty(), "a bare id carries no addresses");

        assert!(parse_target("not-a-real-target").is_err());
    }

    #[test]
    fn target_id_extracts_the_id_from_either_form() {
        let addr = sample_addr();
        assert_eq!(target_id(&encode(&addr).unwrap()).unwrap(), addr.id);
        assert_eq!(target_id(&addr.id.to_string()).unwrap(), addr.id);
    }
}
