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
//! * **Absent `Origin` passes; present-and-matching passes; anything else is refused.** Absent has
//!   to pass. A request arriving from a mesh peer carries no `Origin` at all — the node side
//!   splices the head verbatim ([`adi_mesh::gateway`]) and the panel's own peer callers write
//!   their own head — and neither does any `curl` in the guides. "Present and matching" would
//!   break the fleet.
//! * **The match is against this request's own `Host`**, never a hardcoded `app.adi`. The same
//!   process is read at `app.adi`, at `localhost:8000`, at `127.0.0.1:8000` and — through two mesh
//!   gateways — at `app.<node>.n.adi`, whose `Host` is deliberately never rewritten (see
//!   `adi-webapp/src/origin.rs` for why).
//! * **Only the authority is compared, not the scheme.** The front door terminates TLS and proxies
//!   here in the clear, so a page served as `https://app.adi` sends exactly that `Origin` down a
//!   connection that is plain HTTP at this end. Comparing schemes would refuse every HTTPS reader.
//! * **A POST may not carry a CORS-*simple* content type.** That is the second, independent
//!   barrier: `text/plain`, `application/x-www-form-urlencoded` and `multipart/form-data` are the
//!   three types a cross-origin POST can use without a preflight, and
//!   `adi_webapp_api::handlers::parse_body` decodes JSON out of a body whatever its type claims to
//!   be. Refusing them makes every cross-origin POST non-simple, so the browser must preflight it
//!   first — and the preflight dies here for want of a CORS header. Anything else is allowed
//!   through, which keeps `application/json` and the two raw-bytes endpoints (a dictated
//!   `audio/webm`, an attached `image/png`) working, and keeps a caller that sends no type at all
//!   working too.
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

    if let Some(origin) = req.header("origin") {
        let host = req.header("host").unwrap_or_default();
        if !origin_matches_host(origin, host) {
            return Err(cross_origin(origin.trim()));
        }
    }

    if req.method.eq_ignore_ascii_case("POST")
        && let Some(content_type) = req.header("content-type")
        && is_cors_simple(content_type)
    {
        return Err(Refusal {
            status: 415,
            message: format!(
                "this API does not accept a {} body — send Content-Type: application/json. \
                 (That type is what lets a page on another site post here without the browser \
                 asking first.)",
                mime_of(content_type)
            ),
        });
    }

    Ok(())
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
        Request {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
            body: Vec::new(),
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
        // `app.<node>.n.adi` arrives as both the `Origin` and the `Host`.
        assert_eq!(
            check(&req(
                "POST",
                "/api/agents/run",
                &[
                    ("origin", "http://app.zomro-de1.n.adi"),
                    ("host", "app.zomro-de1.n.adi"),
                    ("content-type", "application/json"),
                ]
            )),
            Ok(())
        );
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
