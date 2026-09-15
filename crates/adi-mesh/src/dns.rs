//! Turning `mesh.toml`'s [`dns`](crate::config::MeshConfig::dns) into the resolver iroh looks
//! names up with.
//!
//! One module for one decision, beside [`relay`](crate::relay) and for the same reason: the
//! endpoint is built in two crates that cannot see each other — [`Daemon`](crate::Daemon) on a Mac
//! or a node, and the iOS viewer (`adi-mesh-ffi`).
//!
//! ## Why a mesh crate has an opinion about DNS at all
//!
//! iroh resolves two kinds of name, and without both it cannot reach a single peer: the relay host
//! it calls home, and `dns.iroh.link`, where pkarr publishes this machine's address and reads
//! everybody else's. Its default resolver reads **only the system's global DNS configuration** —
//! on macOS that is `SystemConfiguration`'s `State:/Network/Global/DNS`, which is the list
//! `/etc/resolv.conf` mirrors and `dig` asks.
//!
//! macOS itself does not resolve that way. mDNSResponder — so Safari, `curl`, the App Store —
//! queries *every* resolver in `scutil --dns`, including the scoped per-interface ones. So when a
//! VPN becomes the primary service and its nameserver stops answering, the machine stays perfectly
//! usable while the mesh goes dark: every lookup times out, there is no home relay and no
//! discovery, every peer is undiallable, and each node's tile reports a 502 whose advice is about
//! pairing and grants — none of which is wrong.
//!
//! Measured on a 1.16.0 install, 2026-09-15: primary resolver `172.19.0.2` on a VPN's `utun8`,
//! reported `Reachable` by `SystemConfiguration` and answering nothing. `dig +short <relay>` timed
//! out, `dig +short @<the router> <relay>` answered instantly, and
//! `curl https://mad.mono-relay.withadi.dev/` returned 200 throughout. The log said
//! `Resolve failed, IPv4: Request timed out, IPv6: Request timed out` for the relay and for
//! `dns.iroh.link`, ~50 times each, while 736 dials failed against one node. Nothing about that
//! is diagnosable from inside adi, and nothing in adi could fix it.
//!
//! Appending a resolver that does answer is what makes it survivable: hickory pools the servers
//! and settles on the one that responds, so a dead primary costs a slow first lookup instead of
//! the whole fleet.
//!
//! ## What this deliberately is not
//!
//! It does not *replace* the system's resolvers — [`with_system_defaults`] keeps them, and they
//! are what a split-horizon name (a company's internal host) still resolves through. It is also
//! not a DNS setting for the machine: this resolver is handed to one iroh endpoint, whose lookups
//! are the relay hosts and `dns.iroh.link` and nothing else. Which is also the answer to "does
//! this leak my queries to Cloudflare": no more than the relay connection itself already says.
//!
//! A machine that must keep every query inside its own network says so by naming that network's
//! resolvers here — a non-empty list replaces the public defaults rather than adding to them.
//!
//! [`with_system_defaults`]: iroh::dns::Builder::with_system_defaults

use std::net::{IpAddr, SocketAddr};

use iroh::dns::{DnsProtocol, DnsResolver};
use tracing::warn;

/// Appended to the system's resolvers when `mesh.toml` names none.
///
/// Two, from different operators, because the failure this guards against is "the one nameserver
/// I was given is dead" and one replacement is the same bet again. They are not ordered by
/// preference: hickory keeps per-server statistics and prefers whichever actually answers.
///
/// Public resolvers rather than ours, deliberately. A relay of ours resolving only through a
/// nameserver of ours would make the fleet's reachability depend on one more thing we run — and
/// the whole point here is to stop depending on a single name server.
pub const DEFAULT_FALLBACK_DNS: &[&str] = &["1.1.1.1", "8.8.8.8"];

/// Port a nameserver is assumed to listen on, per protocol.
const UDP_TCP_PORT: u16 = 53;
const DOH_PORT: u16 = 443;

/// The resolver to hand an endpoint: the system's configuration, plus `servers` (or
/// [`DEFAULT_FALLBACK_DNS`] when that list is empty).
///
/// Never `None`-shaped and never fatal. An entry that does not parse is skipped with a warning,
/// and a list where *nothing* parses lands on the defaults rather than on the system config alone
/// — a typo must not quietly return the machine to the failure mode this module exists for.
#[must_use]
pub fn resolver(servers: &[String]) -> DnsResolver {
    DnsResolver::builder()
        .with_system_defaults()
        .with_nameservers(nameservers(servers))
        .build()
}

/// The parsed nameserver list [`resolver`] appends, split out so the parsing rules are testable
/// without building a resolver (which reads the host's DNS configuration).
#[must_use]
pub fn nameservers(servers: &[String]) -> Vec<(SocketAddr, DnsProtocol)> {
    let parsed: Vec<_> = servers.iter().filter_map(|s| parse(s)).collect();
    if !parsed.is_empty() {
        return parsed;
    }
    if !servers.is_empty() {
        warn!(
            configured = servers.len(),
            "mesh: no configured DNS server parsed; falling back to the public resolvers"
        );
    }
    DEFAULT_FALLBACK_DNS
        .iter()
        .filter_map(|s| parse(s))
        .collect()
}

/// One entry: `1.1.1.1`, `1.1.1.1:5353`, `[2606:4700:4700::1111]:53`, or a scheme —
/// `udp://` (the default), `tcp://`, or `https://` for DNS-over-HTTPS.
///
/// `https://` is the escape hatch for a network that blocks or hijacks port 53 rather than merely
/// having a dead resolver: it is an ordinary HTTPS connection on 443, the port every other thing
/// on the machine is already using. Give it an **address, not a hostname** — a resolver cannot
/// resolve its own server's name — which means the certificate must carry that IP as a SAN.
/// `1.1.1.1`, `8.8.8.8` and `9.9.9.9` all do; most others do not.
fn parse(entry: &str) -> Option<(SocketAddr, DnsProtocol)> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let (protocol, rest) = match entry.split_once("://") {
        Some(("udp", rest)) => (DnsProtocol::Udp, rest),
        Some(("tcp", rest)) => (DnsProtocol::Tcp, rest),
        Some(("https", rest)) => (DnsProtocol::Https, rest),
        Some((scheme, _)) => {
            warn!(%entry, %scheme, "mesh: unknown DNS scheme; skipping this server");
            return None;
        }
        None => (DnsProtocol::Udp, entry),
    };
    // A DoH entry is written as a URL, so tolerate the path every published endpoint carries
    // (`https://1.1.1.1/dns-query`) instead of making the operator remember to strip it. Hickory
    // builds the path itself from the address.
    let rest = rest.split('/').next().unwrap_or(rest);
    let default_port = match protocol {
        DnsProtocol::Https => DOH_PORT,
        _ => UDP_TCP_PORT,
    };
    let addr = parse_addr(rest, default_port).or_else(|| {
        warn!(%entry, "mesh: DNS server is not an address; skipping it");
        None
    })?;
    Some((addr, protocol))
}

/// `host[:port]` where host is an IP address — never a name. `port` defaults to `default_port`.
fn parse_addr(rest: &str, default_port: u16) -> Option<SocketAddr> {
    if let Ok(addr) = rest.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = rest.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, default_port));
    }
    // A bracketed IPv6 with no port (`[::1]`) parses as neither of the above.
    let unbracketed = rest.strip_prefix('[')?.strip_suffix(']')?;
    Some(SocketAddr::new(
        unbracketed.parse::<IpAddr>().ok()?,
        default_port,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn udp(addr: &str) -> (SocketAddr, DnsProtocol) {
        (addr.parse().expect("an address"), DnsProtocol::Udp)
    }

    #[test]
    fn an_empty_list_is_the_public_fallbacks() {
        assert_eq!(
            nameservers(&[]),
            vec![udp("1.1.1.1:53"), udp("8.8.8.8:53")],
            "empty must not mean 'system only' — that is the failure mode this module exists for"
        );
    }

    #[test]
    fn a_configured_list_replaces_the_defaults() {
        // How a machine that must keep every query inside its own network says so.
        assert_eq!(
            nameservers(&servers(&["10.0.0.53"])),
            vec![udp("10.0.0.53:53")],
        );
    }

    #[test]
    fn a_port_and_a_scheme_are_both_optional_and_both_honoured() {
        assert_eq!(
            nameservers(&servers(&[
                "1.1.1.1",
                "9.9.9.9:5353",
                "tcp://8.8.4.4",
                "udp://8.8.8.8:53",
            ])),
            vec![
                udp("1.1.1.1:53"),
                udp("9.9.9.9:5353"),
                ("8.8.4.4:53".parse().expect("an address"), DnsProtocol::Tcp),
                udp("8.8.8.8:53"),
            ],
        );
    }

    #[test]
    fn ipv6_parses_bracketed_with_or_without_a_port() {
        assert_eq!(
            nameservers(&servers(&[
                "2606:4700:4700::1111",
                "[2606:4700:4700::1001]",
                "[2001:4860:4860::8888]:5353",
            ])),
            vec![
                udp("[2606:4700:4700::1111]:53"),
                udp("[2606:4700:4700::1001]:53"),
                udp("[2001:4860:4860::8888]:5353"),
            ],
        );
    }

    #[test]
    fn doh_defaults_to_443_and_tolerates_the_published_path() {
        assert_eq!(
            nameservers(&servers(&["https://1.1.1.1/dns-query"])),
            vec![(
                "1.1.1.1:443".parse().expect("an address"),
                DnsProtocol::Https
            )],
        );
    }

    #[test]
    fn a_hostname_is_refused_because_a_resolver_cannot_resolve_its_own_server() {
        assert_eq!(
            nameservers(&servers(&["dns.quad9.net", "1.0.0.1"])),
            vec![udp("1.0.0.1:53")],
            "the bad entry is skipped, the good one survives",
        );
    }

    #[test]
    fn a_list_where_nothing_parses_lands_on_the_defaults() {
        // Not on the system config alone: a typo must not silently restore the failure mode.
        assert_eq!(
            nameservers(&servers(&["not an address", "https://also-not-one"])),
            vec![udp("1.1.1.1:53"), udp("8.8.8.8:53")],
        );
    }

    #[test]
    fn whitespace_and_blank_entries_are_tolerated() {
        assert_eq!(
            nameservers(&servers(&["  1.1.1.1  ", ""])),
            vec![udp("1.1.1.1:53")],
        );
    }

    #[test]
    fn the_default_fallbacks_all_parse() {
        // DEFAULT_FALLBACK_DNS goes through the same parser as anything an operator writes, so a
        // typo in the constant would silently ship an empty pool.
        assert_eq!(
            DEFAULT_FALLBACK_DNS.len(),
            nameservers(&[]).len(),
            "every default must parse",
        );
    }
}
