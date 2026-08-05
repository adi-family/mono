//! Turning `mesh.toml`'s [`relays`](crate::config::MeshConfig::relays) into an iroh relay map.
//!
//! One module for one decision, because that decision is made in two crates that cannot see each
//! other: [`Daemon`](crate::Daemon) builds the endpoint a Mac or a node runs, and the iOS viewer
//! (`adi-mesh-ffi`) builds its own. Written twice, the two would eventually disagree about what an
//! unparseable URL means — and a machine that quietly fell back to n0's relays while its operator
//! believed it was on theirs is a hard thing to notice: everything works, just not where you think.
//!
//! ## Why the relay is worth configuring at all
//!
//! Off the local network a peer is reachable **only** through a relay until a direct path is
//! hole-punched, and between two consumer NATs that often never happens (`docs/fleet.md` §9). So
//! the relay is not a discovery service you can treat as best-effort — it is the fallback data
//! path, and its stability is the fleet's stability. Ours, near us, beats a shared one across the
//! continent.
//!
//! ## Why a list
//!
//! iroh probes every relay in the map and picks the lowest-latency one as this machine's *home*
//! relay. So the map is not a failover chain to be ordered by preference — it is a menu each
//! machine chooses from on its own. A fleet spread over two continents wants both entries in one
//! list, not two configs.
//!
//! Peers are unaffected by any of it: a machine reaches another through *that* machine's home
//! relay, which it learns from discovery, whether or not the relay is in its own map. Which is why
//! configuring a viewer is optional — see [`relay_mode`].

use iroh::{RelayMap, RelayMode, RelayUrl};
use tracing::warn;

/// The [`RelayMode`] for a configured relay list, or `None` to leave the endpoint's preset alone.
///
/// `None` for an empty list is deliberate and is not the same as [`RelayMode::Disabled`]: a
/// machine that has configured no relay should keep working exactly as it did before this setting
/// existed, on n0's public relays. Turning relaying *off* is a thing you have to ask for, because
/// off the local network it means "unreachable" far more often than it means "direct only".
///
/// Entries that do not parse are **skipped with a warning, not fatal**. A typo in one URL must not
/// cost a machine its whole mesh — but if it costs every URL, the caller gets `None` and lands back
/// on the public relays rather than on an empty map, which would silently be `Disabled`.
#[must_use]
pub fn relay_mode(relays: &[String]) -> Option<RelayMode> {
    let urls = relay_urls(relays);
    if urls.is_empty() {
        if !relays.is_empty() {
            warn!(
                configured = relays.len(),
                "mesh: no configured relay URL parsed; falling back to the public relays"
            );
        }
        return None;
    }
    Some(RelayMode::Custom(RelayMap::from_iter(urls)))
}

/// The schemes a relay can actually be reached over. A relay is an HTTPS endpoint that upgrades to
/// a websocket; `http` is here only because a `--dev` relay on a trusted LAN skips TLS.
const RELAY_SCHEMES: &[&str] = &["https", "http"];

/// The parsed subset of a configured relay list, in the order given.
///
/// Split out so the parsing rules are testable without constructing a [`RelayMode`], whose
/// `RelayMap` deliberately exposes little.
///
/// The scheme is checked because parsing alone lets far too much through: `RelayUrl` wraps a
/// generic `Url`, so a typo like `also::not::one` is a *valid* URL with the scheme `also` and
/// would be filed as a working relay this machine could never reach. Better to drop it here, where
/// the reason can be said out loud, than to leave one dead entry in the map that a latency probe
/// will simply never rank.
fn relay_urls(relays: &[String]) -> Vec<RelayUrl> {
    relays
        .iter()
        .map(|raw| raw.trim())
        .filter(|raw| !raw.is_empty())
        .filter_map(|raw| match raw.parse::<RelayUrl>() {
            Ok(url) if RELAY_SCHEMES.contains(&url.scheme()) => Some(url),
            Ok(url) => {
                warn!(
                    relay = %raw,
                    scheme = url.scheme(),
                    "mesh: ignoring a relay URL that is not http(s)"
                );
                None
            }
            Err(e) => {
                warn!(relay = %raw, error = %e, "mesh: ignoring an unusable relay URL");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(relays: &[&str]) -> Vec<String> {
        relays.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn no_configured_relay_leaves_the_preset_alone() {
        // Not `Disabled`: a machine that configured nothing must keep the behaviour it had before
        // this setting existed, which is n0's public relays.
        assert!(relay_mode(&[]).is_none());
        assert!(relay_mode(&urls(&["", "   "])).is_none());
    }

    #[test]
    fn a_configured_relay_becomes_a_custom_map() {
        let mode = relay_mode(&urls(&["https://mad.mono-relay.withadi.dev"])).expect("custom mode");
        assert!(matches!(mode, RelayMode::Custom(_)));
        assert_eq!(mode.relay_map().len(), 1);
    }

    #[test]
    fn several_relays_all_land_in_one_map() {
        // The map is a menu each machine picks its nearest from, not an ordered failover chain —
        // so every entry has to survive into it.
        let mode = relay_mode(&urls(&[
            "https://mad.mono-relay.withadi.dev",
            "https://fra.mono-relay.withadi.dev",
        ]))
        .expect("custom mode");
        assert_eq!(mode.relay_map().len(), 2);
    }

    #[test]
    fn one_bad_url_does_not_cost_the_good_ones() {
        let mode = relay_mode(&urls(&[
            "not a url",
            "https://mad.mono-relay.withadi.dev",
        ]))
        .expect("the usable one survives");
        assert_eq!(mode.relay_map().len(), 1);
    }

    #[test]
    fn all_urls_unusable_falls_back_rather_than_disabling_relays() {
        // The dangerous shape: an empty custom map is `Disabled` in all but name, and a machine
        // with relaying off is simply unreachable from anywhere but its own LAN.
        assert!(
            relay_mode(&urls(&["not a url", "also::not::one"])).is_none(),
            "a config that parsed to nothing must fall back, never disable"
        );
    }

    #[test]
    fn a_url_we_could_never_reach_is_not_filed_as_a_relay() {
        // `RelayUrl` wraps a generic `Url`, so this parses cleanly as scheme `also` — the exact
        // shape a typo takes, and it would otherwise sit in the map as a relay that simply never
        // answers a latency probe.
        assert!("also::not::one".parse::<RelayUrl>().is_ok(), "premise: it parses");
        assert!(relay_urls(&urls(&["also::not::one"])).is_empty());
        assert!(relay_urls(&urls(&["ftp://relay.example.org"])).is_empty());

        // …while both schemes a relay is actually served over survive. `http` matters because a
        // `--dev` relay on a trusted LAN skips TLS.
        assert_eq!(relay_urls(&urls(&["https://a.example.org"])).len(), 1);
        assert_eq!(relay_urls(&urls(&["http://192.168.0.9:3340"])).len(), 1);
    }

    #[test]
    fn whitespace_around_a_url_is_forgiven() {
        let mode = relay_mode(&urls(&["  https://mad.mono-relay.withadi.dev  "]))
            .expect("trimmed and parsed");
        assert_eq!(mode.relay_map().len(), 1);
    }
}
