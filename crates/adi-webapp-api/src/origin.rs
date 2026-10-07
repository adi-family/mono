//! Browser-origin checks for the unauthenticated loopback API, including WebSocket upgrades.
//!
//! Validate `Host` against this install's private domain and loopback names before comparing
//! it with `Origin`; matching attacker-controlled headers alone cannot prevent DNS rebinding.
//! Local aliases and fleet petnames vary, so names are validated by zone and label shape.
//! This relies on the configured domain being a private split-DNS zone, such as `.adi`.
//!
//! Missing `Host` or `Origin` is allowed for mesh peers and command-line clients. Compare
//! authorities, not schemes: the front door terminates TLS before forwarding plain HTTP.
//! Reject cross-site Fetch Metadata and CORS-simple POST bodies (including untyped bodies),
//! since the JSON parser ignores Content-Type and CORS does not prevent simple POST effects.
//! WebSocket handshakes bypass CORS, so their browser-supplied Origin must also be checked.
//!
//! This blocks browser-driven requests; it does not authenticate local processes.

use adi_config::Flavor;

use crate::http::Request;

#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
}

/// Fetch CORS-safelisted content types that do not trigger a preflight.
const CORS_SIMPLE_TYPES: [&str; 3] = [
    "text/plain",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
];

const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Heuristic for `cdn-when-remote`: loopback and non-fleet names in this install look local.
/// Missing hosts default to local. This selects an asset source, not a security boundary.
#[must_use]
pub fn looks_local(host: Option<&str>) -> bool {
    looks_local_in_zone(host, &Flavor::current().domain)
}

fn looks_local_in_zone(host: Option<&str>, domain: &str) -> bool {
    let Some(host) = host.filter(|h| !h.trim().is_empty()) else {
        return true;
    };
    let authority = normalize_authority(host);
    let name = without_port(&authority);
    LOOPBACK_HOSTS.contains(&name)
        || (is_in_zone(name, domain) && !name.ends_with(&format!(".n.{domain}")))
}

/// Validate a request against this install's names and browser-origin protections.
///
/// # Errors
/// Returns the HTTP status and explanation for an unsafe host, origin, or POST body type.
pub fn check(req: &Request) -> Result<(), Refusal> {
    check_in_zone(req, &Flavor::current().domain)
}

fn check_in_zone(req: &Request, domain: &str) -> Result<(), Refusal> {
    // Cross-site navigations may omit Origin but still carry Fetch Metadata.
    if req
        .header("sec-fetch-site")
        .is_some_and(|site| site.trim().eq_ignore_ascii_case("cross-site"))
    {
        return Err(cross_origin("another site"));
    }

    // Check Host first: matching an attacker-controlled Origin and Host proves nothing.
    if let Some(host) = present(req.header("host"))
        && !is_a_name_we_answer_to(host, domain)
    {
        return Err(not_our_name(host.trim(), domain));
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

fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Describe preflight-free bodies, including an untyped Blob sent by fetch.
/// Untyped bodyless POSTs remain allowed.
fn preflight_free_body(req: &Request) -> Option<String> {
    match present(req.header("content-type")) {
        Some(content_type) => {
            is_cors_simple(content_type).then(|| format!("a {} body", mime_of(content_type)))
        }
        None => (!req.body.is_empty()).then(|| "a body with no Content-Type".to_string()),
    }
}

fn cross_origin(origin: &str) -> Refusal {
    Refusal {
        status: 403,
        message: format!(
            "this control panel does not answer requests from {origin} — it has no login, so a \
             page on another site must not be able to drive it"
        ),
    }
}

fn not_our_name(host: &str, domain: &str) -> Refusal {
    Refusal {
        status: 403,
        message: format!(
            "this control panel is not served at {host} — it answers to its .{domain} names and to \
             loopback, so a request addressed to any other name is a page that pointed its own \
             hostname here, and it has no login to stop that with"
        ),
    }
}

fn is_a_name_we_answer_to(host: &str, domain: &str) -> bool {
    let authority = normalize_authority(host);
    let name = without_port(&authority);
    LOOPBACK_HOSTS.contains(&name) || is_in_zone(name, domain)
}

/// Strip the port while preserving bracketed IPv6 literals.
fn without_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => &authority[..=end],
            None => authority,
        };
    }
    authority.split(':').next().unwrap_or(authority)
}

fn is_in_zone(name: &str, domain: &str) -> bool {
    let suffix = format!(".{}", domain.trim_end_matches('.').to_ascii_lowercase());
    let Some(rest) = name.strip_suffix(&suffix) else {
        return false;
    };
    !rest.is_empty() && rest.split('.').all(is_dns_label)
}

/// RFC 1123 §2.1 hostname label.
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// Reject opaque origins such as `null` from sandboxed iframes and file URLs.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some(origin) = origin_authority(origin) else {
        return false;
    };
    let host = normalize_authority(host);
    !host.is_empty() && origin == host
}

fn origin_authority(origin: &str) -> Option<String> {
    let (scheme, rest) = origin.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = normalize_authority(authority);
    (!authority.is_empty()).then_some(authority)
}

/// Normalize case, trailing root dots, and default ports for scheme-independent comparison.
fn normalize_authority(value: &str) -> String {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    for default in [":80", ":443"] {
        if let Some(bare) = value.strip_suffix(default) {
            return bare.to_string();
        }
    }
    value
}

fn mime_of(content_type: &str) -> &str {
    content_type.split(';').next().unwrap_or_default().trim()
}

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

    fn check(req: &Request) -> Result<(), Refusal> {
        super::check_in_zone(req, "adi")
    }

    fn looks_local(host: Option<&str>) -> bool {
        super::looks_local_in_zone(host, "adi")
    }

    #[test]
    fn a_service_accepts_its_flavor_domain_and_rejects_other_zones() {
        assert_eq!(
            super::check_in_zone(
                &req(
                    "GET",
                    "/api/channels",
                    &[
                        ("host", "channels.adi-dev"),
                        ("origin", "http://channels.adi-dev")
                    ]
                ),
                "adi-dev",
            ),
            Ok(()),
        );
        for host in [
            "channels.adi",
            "channels.adi-dev.evil.example",
            "channels..adi-dev",
            "adi-dev",
        ] {
            assert_eq!(
                super::check_in_zone(&req("GET", "/api/channels", &[("host", host)]), "adi-dev")
                    .unwrap_err()
                    .status,
                403,
                "{host}",
            );
        }
        assert!(super::looks_local_in_zone(
            Some("channels.adi-dev"),
            "adi-dev"
        ));
        assert!(!super::looks_local_in_zone(
            Some("app.other.n.adi-dev"),
            "adi-dev"
        ));
    }

    fn req(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
        with_body(method, path, headers, b"")
    }

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
            ("https://app.adi", "app.adi"),
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
        // DNS rebinding satisfies Origin/Host equality and same-origin Fetch Metadata.
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
            "http://app.adi.evil.example",
            "http://app.adib",
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

        assert_eq!(
            check(&req("POST", "/api/update/check", &[("host", "app.adi")])),
            Ok(())
        );
        assert_eq!(
            check(&with_body("GET", "/api/hive", &[("host", "app.adi")], b"x")),
            Ok(())
        );
    }

    #[test]
    fn the_types_the_panel_actually_posts_are_allowed() {
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
        assert_eq!(
            check(&req(
                "GET",
                "/api/hive",
                &[("host", "app.adi"), ("content-type", "text/plain")]
            )),
            Ok(())
        );
    }

    #[test]
    fn looks_local_is_true_for_loopback_and_this_instances_own_name() {
        for host in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "localhost:8000",
            "app.adi",
            "api.adi",
            "APP.ADI:443",
        ] {
            assert!(looks_local(Some(host)), "{host}");
        }
    }

    #[test]
    fn looks_local_is_false_for_a_fleet_node_or_anything_unrecognized() {
        for host in [
            "app.zomro-de1.n.adi",
            "app.nosh.zomro-de1.n.adi",
            "192.168.1.20:8000",
            "evil.example.com",
        ] {
            assert!(!looks_local(Some(host)), "{host}");
        }
    }

    #[test]
    fn looks_local_treats_a_missing_or_blank_host_as_local() {
        assert!(looks_local(None));
        assert!(looks_local(Some("")));
        assert!(looks_local(Some("   ")));
    }
}
