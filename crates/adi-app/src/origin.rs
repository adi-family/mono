//! Where a request came *from*, and why `/api/*` will not answer a page on another site.
//!
//! This server has no login: it listens on loopback, and whatever reaches it is the operator.
//! A browser breaks that assumption. `.adi` resolves machine-wide through `/etc/resolver`, so
//! `http://app.adi/api/*` is an address that *any* page the operator visits can post to — and a
//! cross-origin POST still lands even though the attacker never sees the reply, because no
//! `Access-Control-Allow-Origin` comes back and none is needed to cause the effect. The end of
//! that chain is root: `POST /api/fs/write` is jailed to `~/.adi/mono`, which holds the
//! `hive/hive.yaml` that the **root** front door re-reads every three seconds and `sh -c`s.
//!
//! So every `/api/*` request passes [`check`] before it is routed, and it turns away what a
//! browser marks as coming from somewhere else. Four decisions worth keeping:
//!
//! * **`Host` has to be a name this panel is served at, and that is asked *first*.** Every other
//!   layer here compares the request against itself — [`origin_matches_host`] asks only whether
//!   `Origin` and `Host` agree — so without this one the check is circular. A name whose DNS an
//!   attacker owns, re-pointed at `127.0.0.1`, reaches this port with
//!   `Origin == Host == evil.example.com:8000` and `Sec-Fetch-Site: same-origin`, and satisfies
//!   every layer while the page driving it is theirs. Comparing `Origin` to `Host` says something
//!   only once `Host` is a name that could not have been served from anywhere but here.
//!
//!   Three shapes pass, and they are *shapes* because none of them can be listed. A name in the
//!   local `.adi` zone: `app.adi`, `api.adi`, and whatever else the operator adds to
//!   `~/.adi/mono/dns/frontdoor.toml`, every one of which the front door proxies to this same
//!   process. A fleet name in the reserved `n.adi` zone — `app.<node>.n.adi`, however deep the
//!   service label — which arrives verbatim because a node cannot know its own petname
//!   (`docs/fleet.md` §2), so the `Host` that lands here is whatever the *caller* calls this
//!   machine. And the loopback literals `localhost`, `127.0.0.1`, `[::1]`. The port is free on all
//!   three, because the port is not what makes them safe: `.adi` is not a public TLD and resolves
//!   only through the split-DNS route this machine installs for itself, and a page can carry a
//!   loopback `Origin` only if it was actually served from loopback.
//!
//!   An **absent** `Host` passes, for the reason an absent `Origin` does: a browser always sends
//!   one, so no `Host` is never a browser — it is a mesh peer or a `curl` from the guides.
//! * **Absent `Origin` passes; present-and-matching passes; anything else is refused.** Absent has
//!   to pass. A request arriving from a mesh peer carries no `Origin` at all — the node side
//!   splices the head verbatim ([`adi_mesh::gateway`]) and the panel's own peer callers write
//!   their own head — and neither does any `curl` in the guides. "Present and matching" would
//!   break the fleet.
//! * **Only the authority is compared, not the scheme.** The front door terminates TLS and proxies
//!   here in the clear, so a page served as `https://app.adi` sends exactly that `Origin` down a
//!   connection that is plain HTTP at this end. Comparing schemes would refuse every HTTPS reader.
//! * **A POST may not carry a CORS-*simple* content type, nor a body with no type at all.** That is
//!   the second, independent barrier: `text/plain`, `application/x-www-form-urlencoded` and
//!   `multipart/form-data` are the three types a cross-origin POST can use without a preflight —
//!   and so is *no* `Content-Type`, which `fetch` produces from a `Blob` body whose type is empty.
//!   `adi_webapp_api::handlers::parse_body` decodes JSON out of a body whatever its head claims,
//!   so all four are the same request to this server. Refusing them makes every cross-origin POST
//!   non-simple, so the browser must preflight it first — and the preflight dies here for want of
//!   a CORS header. Anything else is allowed through, which keeps `application/json` and the two
//!   raw-bytes endpoints (a dictated `audio/webm`, an attached `image/png`) working; a **bodyless**
//!   POST needs no type and keeps working too.
//!
//! `GET /api/ws` is checked by exactly this code and for a reason peculiar to it: a websocket
//! handshake is exempt from CORS, and `new WebSocket()` cannot set a header, so no future token
//! scheme can guard it. The `Origin` a browser always sends on it is the only thing that ever can.
//!
//! What this is *not* is authentication. It stops a web page from driving the panel; it does not
//! stop a process on this machine, which can send whatever head it likes. That is a separate
//! question (an `app.adi` token) and a separate task.

use crate::http::Request;

/// A request that will not be routed: the status to answer with, and the sentence to say.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub status: u16,
    pub message: String,
}

/// The three content types a cross-origin `POST` may use without drawing a preflight
/// (Fetch, §"CORS-safelisted request-header" — the `Content-Type` case).
const CORS_SIMPLE_TYPES: [&str; 3] = [
    "text/plain",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
];

/// The zone every name this machine serves itself at ends in — `app.adi`, `api.adi`, a project's
/// own host, and the reserved `n.adi` the fleet lives in.
const LOCAL_ZONE_SUFFIX: &str = ".adi";

/// The addresses a page served from loopback can name it by. Anything else — `0.0.0.0`, another
/// `127.0.0.0/8` literal, this machine's LAN address — is not a name the panel is read at, and a
/// browser produces none of them for a page it loaded from here.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Whether this `/api/*` request may be routed, or the refusal to answer instead.
pub(crate) fn check(req: &Request) -> Result<(), Refusal> {
    // A browser's own account of where the request came from. Nothing but a browser sends it, so
    // it costs no non-browser caller anything, and it covers the one case `Origin` does not: a
    // cross-site *navigation*, which carries no `Origin` header at all.
    if req
        .header("sec-fetch-site")
        .is_some_and(|site| site.trim().eq_ignore_ascii_case("cross-site"))
    {
        return Err(cross_origin("another site"));
    }

    // Before the comparison below, never after it: `Origin` matching `Host` proves nothing about a
    // `Host` an attacker chose. See the module docs.
    if let Some(host) = present(req.header("host"))
        && !is_a_name_we_answer_to(host)
    {
        return Err(not_our_name(host.trim()));
    }

    if let Some(origin) = req.header("origin") {
        let host = req.header("host").unwrap_or_default();
        if !origin_matches_host(origin, host) {
            return Err(cross_origin(origin.trim()));
        }
    }

    if req.method.eq_ignore_ascii_case("POST")
        && let Some(body) = preflight_free_body(req)
    {
        return Err(Refusal {
            status: 415,
            message: format!(
                "this API does not accept {body} — send Content-Type: application/json. \
                 (Such a body is what lets a page on another site post here without the browser \
                 asking first.)"
            ),
        });
    }

    Ok(())
}

/// A header value that is actually there: `None` for both an absent header and an empty one, since
/// `Host:` with nothing after it is no more a browser than no `Host` at all.
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// How this POST's body would be described in a refusal, or `None` when it needs no preflight
/// exemption — either because it is bodyless or because its type draws a preflight of its own.
///
/// The untyped case is not a special case of the three simple types but the same rule reached from
/// the other side: Fetch treats a missing `Content-Type` as preflight-free too, which is exactly
/// what `fetch` sends for a `Blob` body of empty type. A bodyless POST is left alone — several of
/// this panel's own endpoints are one, and there is nothing in them to smuggle.
fn preflight_free_body(req: &Request) -> Option<String> {
    match present(req.header("content-type")) {
        Some(content_type) => {
            is_cors_simple(content_type).then(|| format!("a {} body", mime_of(content_type)))
        }
        None => (!req.body.is_empty()).then(|| "a body with no Content-Type".to_string()),
    }
}

/// The 403 for a request a browser says came from somewhere else.
fn cross_origin(origin: &str) -> Refusal {
    Refusal {
        status: 403,
        message: format!(
            "this control panel does not answer requests from {origin} — it has no login, so a \
             page on another site must not be able to drive it"
        ),
    }
}

/// The 403 for a request addressed to a name this panel is not served at — which is what a page
/// whose own hostname has been pointed at this machine looks like from in here.
fn not_our_name(host: &str) -> Refusal {
    Refusal {
        status: 403,
        message: format!(
            "this control panel is not served at {host} — it answers to its .adi names and to \
             loopback, so a request addressed to any other name is a page that pointed its own \
             hostname here, and it has no login to stop that with"
        ),
    }
}

/// Whether `Host` names this panel: a `.adi` name (the local zone and the fleet's `n.adi` alike),
/// or a loopback literal. Any port, on either — see the module docs for why the port is not what
/// makes a name safe.
fn is_a_name_we_answer_to(host: &str) -> bool {
    let authority = normalize_authority(host);
    let name = without_port(&authority);
    LOOPBACK_HOSTS.contains(&name) || is_in_the_adi_zone(name)
}

/// An authority without its port: `app.adi:8000` → `app.adi`, `[::1]:8000` → `[::1]`.
///
/// The bracket case first, or an IPv6 literal's own colons would be read as a port separator and
/// leave nothing behind.
fn without_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => &authority[..=end],
            None => authority,
        };
    }
    authority.split(':').next().unwrap_or(authority)
}

/// Whether a name is in this machine's own zone — anything under `.adi`, which includes the
/// reserved `n.adi` a fleet name lives in.
///
/// Matched by shape rather than against a list because neither half can be enumerated from here:
/// the local names are whatever `frontdoor.toml` says, and a fleet name contains the *viewer's*
/// petname for this machine, which this machine has no way of knowing (`docs/fleet.md` §2). What
/// the shape is worth is that `.adi` is not a public TLD: it resolves only through the split-DNS
/// route installed for this machine, so a name in it cannot be pointed anywhere by anybody else.
fn is_in_the_adi_zone(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(LOCAL_ZONE_SUFFIX) else {
        return false;
    };
    !rest.is_empty() && rest.split('.').all(is_dns_label)
}

/// One label of a hostname, by RFC 1123 §2.1: letters, digits and hyphens, never leading or
/// trailing a hyphen, never empty and never longer than 63.
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// Whether an `Origin` header names the very host this request was addressed to.
///
/// `Origin: null` — a sandboxed iframe, a `file://` page, some redirects — parses to nothing and
/// so matches nothing, which is the answer it should get.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some(origin) = origin_authority(origin) else {
        return false;
    };
    let host = normalize_authority(host);
    !host.is_empty() && origin == host
}

/// The `host[:port]` inside an `Origin`, normalized for comparison — or `None` when it is not an
/// http(s) origin at all.
fn origin_authority(origin: &str) -> Option<String> {
    let (scheme, rest) = origin.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = normalize_authority(authority);
    (!authority.is_empty()).then_some(authority)
}

/// An authority reduced to what two sides of the comparison can be expected to agree on:
/// lowercased, without a trailing root dot, and without an explicit default port.
///
/// The port matters because the scheme is not compared: a page at `https://app.adi` and the
/// `Host: app.adi` it arrives with must still read as one host, and so must the `app.adi:443` a
/// hand-typed URL can produce.
fn normalize_authority(value: &str) -> String {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    for default in [":80", ":443"] {
        if let Some(bare) = value.strip_suffix(default) {
            return bare.to_string();
        }
    }
    value
}

/// A `Content-Type` header's media type, without its parameters.
fn mime_of(content_type: &str) -> &str {
    content_type.split(';').next().unwrap_or_default().trim()
}

/// Whether a body of this type would let a cross-origin `POST` skip its preflight.
fn is_cors_simple(content_type: &str) -> bool {
    let mime = mime_of(content_type);
    CORS_SIMPLE_TYPES
        .iter()
        .any(|simple| mime.eq_ignore_ascii_case(simple))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A request with the given method, path and headers — header names lowercased, as
    /// [`crate::http::parse_head`] leaves them.
    fn req(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
        with_body(method, path, headers, b"")
    }

    /// [`req`], carrying a body — which is what makes an untyped POST a POST worth refusing.
    fn with_body(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
            body: body.to_vec(),
            rest: Vec::new(),
        }
    }

    #[test]
    fn a_request_with_no_origin_is_answered() {
        // Every mesh peer and every `curl` in the guides looks like this. Refusing it would take
        // the fleet's control-panel hop down with it.
        assert_eq!(
            check(&req("GET", "/api/health", &[("host", "app.adi")])),
            Ok(())
        );
        assert_eq!(
            check(&req(
                "POST",
                "/api/fs/list",
                &[("host", "127.0.0.1"), ("content-type", "application/json")]
            )),
            Ok(())
        );
    }

    #[test]
    fn the_panels_own_page_is_answered_at_every_name_it_is_read_at() {
        for (origin, host) in [
            ("http://app.adi", "app.adi"),
            ("http://localhost:8000", "localhost:8000"),
            ("http://127.0.0.1:8000", "127.0.0.1:8000"),
            // The front door terminates TLS and proxies here in the clear, so the page's own
            // scheme is not the connection's.
            ("https://app.adi", "app.adi"),
            // Case and an explicit default port are the browser's business, not a mismatch.
            ("http://APP.adi:80", "app.adi"),
            ("https://app.adi", "app.adi:443"),
        ] {
            assert_eq!(
                check(&req(
                    "POST",
                    "/api/fs/write",
                    &[
                        ("origin", origin),
                        ("host", host),
                        ("content-type", "application/json"),
                        ("sec-fetch-site", "same-origin"),
                    ]
                )),
                Ok(()),
                "{origin} at {host}"
            );
        }
    }

    #[test]
    fn a_panel_read_through_a_node_is_answered() {
        // Two mesh gateways in between, and neither rewrites the head: the browser's own
        // `app.<node>.n.adi` arrives as both the `Origin` and the `Host`. However deep the service
        // label is — the node label is the one before the zone, and the rest is the service.
        for host in ["app.zomro-de1.n.adi", "app.nosh.zomro-de1.n.adi"] {
            let origin = format!("http://{host}");
            assert_eq!(
                check(&req(
                    "POST",
                    "/api/agents/run",
                    &[
                        ("origin", origin.as_str()),
                        ("host", host),
                        ("content-type", "application/json"),
                    ]
                )),
                Ok(()),
                "{host}"
            );
        }
    }

    #[test]
    fn a_name_this_panel_is_not_served_at_is_refused_however_well_it_agrees_with_itself() {
        // DNS rebinding: the attacker owns `evil.example.com` and re-points it at 127.0.0.1, so
        // their own page reaches this port with every layer below satisfied — `Origin` equals
        // `Host` because both are *their* name, and the browser calls it same-origin because from
        // the browser's side it is. The only thing wrong with the request is the name it is
        // addressed to, which is why that is checked first.
        let refusal = check(&req(
            "GET",
            "/api/secrets",
            &[
                ("origin", "http://evil.example.com:8000"),
                ("host", "evil.example.com:8000"),
                ("sec-fetch-site", "same-origin"),
            ],
        ))
        .unwrap_err();
        assert_eq!(refusal.status, 403);

        // And with no `Origin` at all, which is how the same page's `<form>` or a redirect arrives.
        let refusal =
            check(&req("GET", "/api/secrets", &[("host", "evil.example.com")])).unwrap_err();
        assert_eq!(refusal.status, 403);

        for host in [
            "app.adi.evil.example", // the zone as a prefix of somebody else's name
            "evil.example",         // no zone at all
            "adi",                  // the zone apex names nothing
            "app..adi",             // an empty label
            "-app.adi",             // not a legal label
            "app.adi evil.example", // whitespace inside what claims to be one name
            "192.168.1.20:8000",    // this machine on the LAN is not loopback
            "127.0.0.2:8000",
        ] {
            let refusal = check(&req("GET", "/api/secrets", &[("host", host)])).unwrap_err();
            assert_eq!(refusal.status, 403, "{host}");
        }
    }

    #[test]
    fn every_name_the_front_door_proxies_here_is_answered() {
        // `~/.adi/mono/dns/frontdoor.toml` lists the hosts the front door points at this one
        // process — `app.adi` and `api.adi` by default, and whatever else the operator adds. They
        // cannot be enumerated from in here, so the check is the zone rather than the list.
        for host in [
            "app.adi",
            "api.adi",
            "app.adi:8000", // the port behind the front door, addressed by name
            "APP.ADI.",     // a browser's case and a hand-typed root dot
            "localhost",
            "[::1]:8000",
        ] {
            assert_eq!(
                check(&req("GET", "/api/health", &[("host", host)])),
                Ok(()),
                "{host}"
            );
        }
    }

    #[test]
    fn a_page_on_another_site_is_refused() {
        for origin in [
            "https://evil.example",
            "http://evil.adi",
            // A lookalike that only shares a suffix, and one that only shares a prefix.
            "http://app.adi.evil.example",
            "http://app.adib",
            // A sandboxed iframe or a `file://` page.
            "null",
        ] {
            let refusal = check(&req(
                "POST",
                "/api/fs/write",
                &[
                    ("origin", origin),
                    ("host", "app.adi"),
                    ("content-type", "application/json"),
                ],
            ))
            .unwrap_err();
            assert_eq!(refusal.status, 403, "{origin}");
        }
    }

    #[test]
    fn a_node_is_not_reachable_through_another_nodes_panel() {
        // `app.other.n.adi` is a third machine, and its page has no business posting here.
        let refusal = check(&req(
            "POST",
            "/api/fs/write",
            &[
                ("origin", "http://app.other.n.adi"),
                ("host", "app.zomro-de1.n.adi"),
                ("content-type", "application/json"),
            ],
        ))
        .unwrap_err();
        assert_eq!(refusal.status, 403);
    }

    #[test]
    fn the_live_channel_is_checked_too() {
        // The one route where this is the *only* guard available: a websocket handshake carries no
        // header the page chose, so nothing else can ever be asked of it.
        let refusal = check(&req(
            "GET",
            "/api/ws",
            &[("origin", "https://evil.example"), ("host", "app.adi")],
        ))
        .unwrap_err();
        assert_eq!(refusal.status, 403);
    }

    #[test]
    fn a_browser_that_says_it_is_cross_site_is_refused_without_an_origin() {
        let refusal = check(&req(
            "GET",
            "/api/secrets",
            &[("host", "app.adi"), ("sec-fetch-site", "cross-site")],
        ))
        .unwrap_err();
        assert_eq!(refusal.status, 403);
        // The other three values are all this machine's own page, or an address bar.
        for site in ["same-origin", "same-site", "none"] {
            assert_eq!(
                check(&req(
                    "GET",
                    "/api/secrets",
                    &[("host", "app.adi"), ("sec-fetch-site", site)]
                )),
                Ok(()),
                "{site}"
            );
        }
    }

    #[test]
    fn a_post_may_not_use_a_type_that_skips_the_preflight() {
        for content_type in [
            "text/plain;charset=UTF-8",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "TEXT/PLAIN",
        ] {
            let refusal = check(&req(
                "POST",
                "/api/fs/write",
                &[("host", "app.adi"), ("content-type", content_type)],
            ))
            .unwrap_err();
            assert_eq!(refusal.status, 415, "{content_type}");
        }
    }

    #[test]
    fn a_post_with_a_body_and_no_type_at_all_is_refused_too() {
        // `fetch(url, { method: 'POST', body: new Blob([json]) })` sends exactly this: a body, and
        // no `Content-Type`, which needs no preflight for the same reason `text/plain` does not —
        // and `parse_body` reads the JSON out of it regardless.
        for headers in [
            vec![("host", "app.adi")],
            vec![("host", "app.adi"), ("content-type", "  ")],
        ] {
            let request = with_body("POST", "/api/secrets/reveal", &headers, b"{\"name\":\"x\"}");
            let refusal = check(&request).unwrap_err();
            assert_eq!(refusal.status, 415, "{headers:?}");
            assert!(
                refusal.message.contains("application/json"),
                "the refusal says what to send instead: {}",
                refusal.message
            );
        }

        // A POST with nothing in it is a different request: several of this panel's own endpoints
        // are one (`/api/update/check`), and an empty body has nothing to smuggle.
        assert_eq!(
            check(&req("POST", "/api/update/check", &[("host", "app.adi")])),
            Ok(())
        );
        // And a GET is not a mutation, whatever it carries.
        assert_eq!(
            check(&with_body("GET", "/api/hive", &[("host", "app.adi")], b"x")),
            Ok(())
        );
    }

    #[test]
    fn the_types_the_panel_actually_posts_are_allowed() {
        // JSON, plus the two endpoints that take raw bytes with the type the browser gave them —
        // none of the three is CORS-simple, so each still draws a preflight cross-origin.
        for content_type in ["application/json", "audio/webm;codecs=opus", "image/png"] {
            assert_eq!(
                check(&req(
                    "POST",
                    "/api/agents/attachment",
                    &[("host", "app.adi"), ("content-type", content_type)]
                )),
                Ok(()),
                "{content_type}"
            );
        }
        // A GET is not a mutation and never carries a body worth typing.
        assert_eq!(
            check(&req(
                "GET",
                "/api/hive",
                &[("host", "app.adi"), ("content-type", "text/plain")]
            )),
            Ok(())
        );
    }
}
