//! Self-host mode's `ip_allowlist`: a minimal, dependency-free CIDR matcher — IPv4 and IPv6,
//! `a.b.c.d/n` or a bare address (an exact match, `/32` or
//! `/128`). Deliberately not a crate dependency: a CIDR string and an IP prefix comparison are
//! both already in `std::net`, and this is the one place in the tree that needs either.

use std::net::IpAddr;

/// Parse one `ip_allowlist` entry into the address/prefix-length pair [`matches`] compares
/// against. `None` for anything that isn't a valid address, optionally followed by `/<bits>`
/// within the address family's own width (32 for IPv4, 128 for IPv6) — a malformed entry is the
/// operator's own config mistake, left for whoever starts the relay to notice, not guessed at
/// here.
#[must_use]
fn parse(entry: &str) -> Option<(IpAddr, u8)> {
    let entry = entry.trim();
    let (addr, bits) = match entry.split_once('/') {
        Some((addr, bits)) => (addr, bits.parse::<u8>().ok()?),
        None => (entry, 0), // filled in below, once the address family is known.
    };
    let addr: IpAddr = addr.parse().ok()?;
    let max_bits = match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    let bits = if entry.contains('/') { bits } else { max_bits };
    (bits <= max_bits).then_some((addr, bits))
}

/// Does `remote` fall inside any entry of `allowlist`? An empty allowlist is unrestricted — the
/// caller decides whether to consult this at all; an allowlist
/// that is present but names only malformed entries matches nothing, which is a misconfigured
/// relay refusing every connection rather than one silently wide open.
#[must_use]
pub fn allows(allowlist: &[String], remote: IpAddr) -> bool {
    allowlist.is_empty()
        || allowlist
            .iter()
            .filter_map(|e| parse(e))
            .any(|(addr, bits)| in_prefix(remote, addr, bits))
}

fn in_prefix(remote: IpAddr, addr: IpAddr, bits: u8) -> bool {
    match (remote, addr) {
        (IpAddr::V4(r), IpAddr::V4(a)) => mask_matches(&r.octets(), &a.octets(), bits),
        (IpAddr::V6(r), IpAddr::V6(a)) => mask_matches(&r.octets(), &a.octets(), bits),
        _ => false, // an IPv4 remote never matches an IPv6 entry, and vice versa.
    }
}

/// Compare the leading `bits` bits of two same-length byte strings.
fn mask_matches(remote: &[u8], prefix: &[u8], bits: u8) -> bool {
    let full_bytes = usize::from(bits) / 8;
    let remainder_bits = bits % 8;
    if remote[..full_bytes] != prefix[..full_bytes] {
        return false;
    }
    if remainder_bits == 0 {
        return true;
    }
    let mask = 0xFFu8 << (8 - remainder_bits);
    (remote[full_bytes] & mask) == (prefix[full_bytes] & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_empty_allowlist_is_unrestricted() {
        assert!(allows(&[], ip("203.0.113.1")));
    }

    #[test]
    fn a_bare_address_matches_only_itself() {
        let list = vec!["10.0.0.5".to_string()];
        assert!(allows(&list, ip("10.0.0.5")));
        assert!(!allows(&list, ip("10.0.0.6")));
    }

    #[test]
    fn an_ipv4_cidr_matches_its_whole_block_and_nothing_outside_it() {
        let list = vec!["10.0.0.0/8".to_string()];
        assert!(allows(&list, ip("10.255.0.1")));
        assert!(!allows(&list, ip("11.0.0.1")));
    }

    #[test]
    fn a_non_byte_aligned_prefix_matches_at_the_bit_boundary() {
        let list = vec!["192.168.1.0/30".to_string()]; // .0-.3
        assert!(allows(&list, ip("192.168.1.3")));
        assert!(!allows(&list, ip("192.168.1.4")));
    }

    #[test]
    fn an_ipv6_cidr_matches_its_block() {
        let list = vec!["2001:db8::/32".to_string()];
        assert!(allows(&list, ip("2001:db8::1")));
        assert!(!allows(&list, ip("2001:db9::1")));
    }

    #[test]
    fn address_families_never_cross_match() {
        let list = vec!["10.0.0.0/8".to_string()];
        assert!(!allows(&list, ip("::1")));
    }

    #[test]
    fn a_malformed_entry_matches_nothing_rather_than_panicking() {
        let list = vec!["not-an-ip/8".to_string(), "10.0.0.0/99".to_string()];
        assert!(!allows(&list, ip("10.0.0.1")));
    }

    #[test]
    fn one_good_entry_among_bad_ones_still_matches() {
        let list = vec!["not-an-ip".to_string(), "10.0.0.0/8".to_string()];
        assert!(allows(&list, ip("10.1.2.3")));
    }
}
