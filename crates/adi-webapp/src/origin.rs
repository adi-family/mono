//! Where this panel is being *viewed from*, and what that does to the addresses it prints.
//!
//! The same `adi-app` serves this page on two different names. On the machine that runs it the
//! address bar says `app.adi`; from another machine on the fleet it says `app.<node>.n.adi`, and
//! the bytes reach the browser through the viewer's front door, its mesh gateway, and the node's
//! gateway (`docs/fleet.md` §3). Nothing in the page changes — the `Host` header is deliberately
//! never rewritten — so the node cannot tell the two apart, and it certainly cannot know what the
//! viewer calls it. Only the URL the browser already holds knows that.
//!
//! That matters because every service name this panel prints is *local to the node*:
//! `nosh-status.adi` is a name the node's own front door routes. Typed into a browser on another
//! machine it hits **that** machine's front door instead, where it names nothing (or, worse, names
//! something else). The address that reaches the service being described is
//! `nosh-status.<node>.n.adi` — the same rule [`adi-app`'s `mesh_url`](../../adi-app/src/node.rs)
//! applies server-side when this panel asks a node what it runs.
//!
//! So: read the node out of our own location, and map every service host through it.

/// The node this page is being viewed through — `None` when it is being viewed on the machine that
/// serves it.
///
/// Read once. A document's location cannot change without loading a new document, which starts a
/// new wasm instance anyway, and this is consulted once per link per render.
pub(crate) fn viewing_node() -> Option<String> {
    thread_local! {
        static NODE: Option<String> = web_sys::window()
            .and_then(|w| w.location().host().ok())
            .and_then(|host| node_of(&host));
    }
    NODE.with(Clone::clone)
}

/// Where to open a service whose hostname on its own machine is `host`, or `None` when there is no
/// address for it from here.
pub(crate) fn service_url(host: &str) -> Option<String> {
    service_host(host).map(|host| format!("http://{host}/"))
}

/// Rewrite the host inside an absolute URL the same way, keeping its scheme and path.
///
/// For the addresses the server hands down already assembled — a node's dashboard arrives as a
/// whole `http://<service>.<node>.n.adi/`, built by the node listing against *this* machine's
/// registry (`adi-app/src/node.rs`). Read through a node that URL names a third machine, which is
/// one hop more than the gateway routes, and this says so by refusing it.
pub(crate) fn mapped_url(url: &str) -> Option<String> {
    mapped_url_via(url, viewing_node().as_deref())
}

/// [`mapped_url`] with the viewing node passed in, so the mapping is testable off a browser.
fn mapped_url_via(url: &str, node: Option<&str>) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    service_host_via(host, node).map(|host| format!("{scheme}://{host}/{path}"))
}

/// The hostname `host` answers to from where this page is being read, or `None` when it answers to
/// nothing here.
///
/// Viewed locally that is the host itself. Viewed through a node, a `.adi` name becomes that same
/// name with the node label spliced in before the zone — `nosh.adi` → `nosh.<node>.n.adi`,
/// `app.nosh.adi` → `app.nosh.<node>.n.adi`, however many labels the service is. A name already in
/// the reserved `n.adi` zone is refused rather than re-pointed: it names a third machine, which is
/// one hop more than the gateway routes. A name outside `.adi` is a real domain and means the same
/// thing from anywhere.
fn service_host(host: &str) -> Option<String> {
    service_host_via(host, viewing_node().as_deref())
}

/// [`service_host`] with the viewing node passed in, so the mapping is testable off a browser.
fn service_host_via(host: &str, node: Option<&str>) -> Option<String> {
    let host = host.trim().trim_end_matches('.').trim();
    if host.is_empty() {
        return None;
    }
    let Some(node) = node else {
        return Some(host.to_string());
    };
    let labels: Vec<&str> = host.split('.').collect();
    let [service @ .., "adi"] = labels.as_slice() else {
        // Not a `.adi` name at all: a real domain, which answers where it is published.
        return Some(host.to_string());
    };
    match service {
        // `adi` alone, or a name already in the fleet zone — a chain of nodes the gateway will
        // not route. Neither has an address from here.
        [] | [.., "n"] => None,
        // An empty label would splice into `.<node>.n.adi`, which resolves to the node's apex
        // rather than to nothing — refuse it here rather than build a link to the wrong place.
        _ if service.iter().any(|label| label.is_empty()) => None,
        _ => Some(format!("{}.{node}.n.adi", service.join("."))),
    }
}

/// The node label in a fleet hostname — `app.zomro-de1.n.adi` → `zomro-de1`, and
/// `app.nosh.zomro-de1.n.adi` → `zomro-de1` too: the node is always the label just before the
/// zone, whatever is to its left.
///
/// `None` for anything that is not a `<service>.<node>.n.adi`, which is the same shape
/// `adi_mesh::protocol::parse_fleet_host` accepts on the routing side. An `:port` is dropped and a
/// trailing root dot tolerated, since a location's host carries whatever was typed at it.
fn node_of(view_host: &str) -> Option<String> {
    let name = view_host.trim();
    let name = name.split(':').next()?;
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let labels: Vec<&str> = name.split('.').collect();
    let [service @ .., node, "n", "adi"] = labels.as_slice() else {
        return None;
    };
    (!service.is_empty() && !node.is_empty()).then(|| (*node).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fleet_location_names_the_node_it_came_through() {
        assert_eq!(node_of("app.zomro-de1.n.adi").as_deref(), Some("zomro-de1"));
        assert_eq!(node_of("APP.Laptop-B.N.ADI").as_deref(), Some("laptop-b"));
        assert_eq!(node_of("app.laptop-b.n.adi.").as_deref(), Some("laptop-b"));
        assert_eq!(node_of("app.laptop-b.n.adi:8443").as_deref(), Some("laptop-b"));
        // The node is the label before the zone, however deep the service name is.
        assert_eq!(
            node_of("app.nosh.zomro-de1.n.adi").as_deref(),
            Some("zomro-de1")
        );
    }

    #[test]
    fn every_other_location_is_local() {
        for host in [
            "app.adi",            // the ordinary local panel
            "localhost:8000",     // …and the port behind it
            "127.0.0.1:8000",
            "n.adi",              // the suffix alone names no node
            "app.n.adi",          // a node with no service
            "app.laptop-b.n.adi.example.com", // a lookalike that is not in the zone
        ] {
            assert_eq!(node_of(host), None, "{host} is not a fleet host");
        }
    }

    #[test]
    fn viewed_locally_a_service_keeps_its_own_name() {
        assert_eq!(
            service_host_via("nosh-status.adi", None).as_deref(),
            Some("nosh-status.adi")
        );
        assert_eq!(service_host_via("  ", None), None);
    }

    #[test]
    fn viewed_through_a_node_a_service_moves_into_that_nodes_zone() {
        assert_eq!(
            service_host_via("nosh-status.adi", Some("zomro-de1")).as_deref(),
            Some("nosh-status.zomro-de1.n.adi")
        );
        // Trailing root dot and surrounding space come from hand-edited hive files.
        assert_eq!(
            service_host_via(" nosh.adi. ", Some("laptop-b")).as_deref(),
            Some("nosh.laptop-b.n.adi")
        );
        // A real domain is the same address from either side.
        assert_eq!(
            service_host_via("status.example.com", Some("laptop-b")).as_deref(),
            Some("status.example.com")
        );
        // A node's own hosts are not all one label; the whole name moves into its zone.
        assert_eq!(
            service_host_via("app.nosh.adi", Some("zomro-de1")).as_deref(),
            Some("app.nosh.zomro-de1.n.adi")
        );
    }

    #[test]
    fn an_unmappable_adi_name_gets_no_link_from_a_node() {
        // Each of these would resolve on the *viewer's* front door and open the wrong thing.
        for host in ["nosh.other.n.adi", "app.nosh.other.n.adi", "adi", ".adi"] {
            assert_eq!(service_host_via(host, Some("laptop-b")), None, "{host}");
        }
    }

    #[test]
    fn an_assembled_url_keeps_its_scheme_and_path() {
        assert_eq!(
            mapped_url_via("http://nosh.adi/panel?x=1", None).as_deref(),
            Some("http://nosh.adi/panel?x=1")
        );
        assert_eq!(
            mapped_url_via("https://nosh.adi", Some("laptop-b")).as_deref(),
            Some("https://nosh.laptop-b.n.adi/")
        );
        // The node listing builds these against *this* machine's registry, so read through a node
        // they name a third machine — one hop more than the gateway routes.
        assert_eq!(mapped_url_via("http://nosh.other.n.adi/", Some("laptop-b")), None);
        assert_eq!(mapped_url_via("nosh.adi", None), None);
    }
}
