//! What a node has, asked over the mesh: its dashboards, and permission to open one.
//!
//! ## Why the list comes from the control panel and not from the protocol
//!
//! `adi/mesh/http/1` deliberately cannot answer "what do you serve?". The node refuses an
//! unauthorized peer *before* it consults its route table, precisely so nobody can enumerate a
//! machine's services by watching `ServiceUnknown` and `NotAuthorized` differ
//! (`adi-mesh/src/gateway.rs`, `admit`). Adding a listing frame would undo that on the wire.
//!
//! So the list comes from somewhere that already knows who is asking: the node's own control
//! panel. `app` is a service like any other — it is what the default grant names
//! (`docs/fleet.md` §8), it sits behind the node's Basic-auth gate (§5), and it already publishes
//! `GET /api/dashboards`. A phone holding that node's password is entitled to the answer, and
//! asking for it needs no new wire format, no new grant kind and no version bump: it is an
//! ordinary authenticated request that happens to travel over the mesh.
//!
//! ## Why this may also *grant*
//!
//! A dashboard is not reachable until the node names it — §8 makes the default grant `http:app`
//! and nothing else, so that no dashboard is exposed until someone says so. A list on its own
//! would therefore be a list of rows that all fail to open, which is not a feature.
//!
//! The same panel serves `POST /api/fleet/grants/add`, and reaching for it escalates nothing:
//! `http:app` plus the password *is* the control panel, which can already create dashboards, move
//! ports and run tasks. What the grant adds is not authority but reach — the browser gets the
//! page on its own origin (§4) instead of driving it through the panel.
//!
//! Both calls are one HTTP request each, and neither ever holds the answer: nothing here is
//! cached, so what the UI shows is what the node said this second.

use adi_mesh::fleet::{Grant, Target};
use adi_mesh::protocol::{self, HttpStatus};
use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;
use tokio::time::timeout;
use tracing::{debug, info};

use super::{STEP_TIMEOUT, Shared};

/// The service label a node's control panel answers to, on every node (`docs/fleet.md` §1).
const PANEL: &str = "app";

/// The most of a panel reply we will read. Generous for a JSON listing and small enough that a
/// misrouted request cannot make a phone buffer a video.
const MAX_REPLY: u64 = 4 * 1024 * 1024;

/// The zone a node's own services answer under locally, and therefore the only suffix a host can
/// have and still be a service label the mesh can route to.
const LOCAL_ZONE: &str = "adi";

// ---------------------------------------------------------------------------------------
// What crosses the FFI
// ---------------------------------------------------------------------------------------

/// A node's dashboards, as this phone may see them.
#[derive(Debug, Serialize)]
pub struct Catalog {
    /// What the node calls *this phone* in its own registry — the petname a grant is filed
    /// under. `None` when the panel could not be asked, which is also why a grant would fail.
    pub me: Option<String>,
    /// Live dashboards, in the order the panel listed them (by name).
    pub dashboards: Vec<DashboardInfo>,
}

/// One dashboard on a node.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DashboardInfo {
    /// The dashboard's directory name on the node — stable, and what the panel keys it by.
    pub id: String,
    /// Its display name from the manifest.
    pub name: String,
    pub description: Option<String>,
    /// The service label to open, taken from the single host the dashboard declares (§4):
    /// `nosh.adi` → `nosh`. `None` when it declares none, or declares a name outside the local
    /// zone — either way there is nothing the mesh could route to, and the UI must say so rather
    /// than offer a tap that cannot work.
    pub service: Option<String>,
    /// Whether the node's supervisor has the page's own server up. A dashboard that is down is
    /// still worth listing — the failure is then the node's to fix, not a row that vanished.
    pub running: bool,
    /// Whether this phone's grants on the node already cover it. False means one tap has to ask
    /// for the grant first.
    pub allowed: bool,
}

// ---------------------------------------------------------------------------------------
// What the panel sends
// ---------------------------------------------------------------------------------------

/// `GET /api/dashboards`. Every field is optional here even where the panel's own DTO makes it
/// required: this is the *client* half of a version skew, and one missing key should cost the row
/// its detail, never the whole listing.
#[derive(Debug, Deserialize)]
struct DashboardsReply {
    #[serde(default)]
    dashboards: Vec<RawDashboard>,
}

#[derive(Debug, Deserialize)]
struct RawDashboard {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    frontend_running: bool,
    #[serde(default)]
    archived_at: Option<u64>,
}

/// `GET /api/fleet` — the node's own view of who it has paired with, which is where this phone
/// finds both its petname there and the grants it actually holds.
#[derive(Debug, Deserialize)]
struct FleetReply {
    #[serde(default)]
    nodes: Vec<RawPeer>,
}

#[derive(Debug, Deserialize)]
struct RawPeer {
    petname: String,
    key: String,
    #[serde(default)]
    grants: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// The two calls
// ---------------------------------------------------------------------------------------

/// List `node`'s dashboards and say which of them this phone may open.
///
/// `local_grants` is the phone's own copy of what it was granted at pairing, used only when the
/// node's fleet page cannot be read: a panel that predates `/api/fleet` should still yield a
/// usable list, and the mirror the pairing wrote is the honest second-best answer.
///
/// # Errors
/// If the node cannot be reached, refuses the `app` service, rejects the password, or answers
/// something that is not the listing.
pub async fn fetch(
    shared: &Shared,
    key: EndpointId,
    me: &str,
    credential: (&str, &str),
    local_grants: &[Grant],
) -> anyhow::Result<Catalog> {
    let listing: DashboardsReply = serde_json::from_slice(
        &request(shared, key, credential, "GET", "/api/dashboards", None).await?,
    )
    .context("the node's control panel did not answer with a dashboard listing")?;

    // A failure here is not fatal: the dashboards are already in hand, and the only thing the
    // fleet page adds is *whose* grants they are checked against.
    let mine = match request(shared, key, credential, "GET", "/api/fleet", None).await {
        Ok(body) => find_me(&body, me),
        Err(e) => {
            debug!(error = %format!("{e:#}"), "could not read the node's fleet page");
            None
        }
    };
    let (petname, grants) = match mine {
        Some((petname, grants)) => (Some(petname), grants),
        None => (None, local_grants.to_vec()),
    };

    Ok(Catalog {
        me: petname,
        dashboards: assemble(listing.dashboards, &grants),
    })
}

/// Ask `node` to let this phone open `service`, and return the petname it was filed under.
///
/// The petname is re-read here rather than carried over from [`fetch`]: a grant names a peer, and
/// the name the node uses for this phone is the node's to change (§2 rule 5).
///
/// # Errors
/// If the node cannot be reached, if its fleet page does not list this phone's key (which means
/// the pairing is gone on that side), or if the panel refuses the grant.
pub async fn allow(
    shared: &Shared,
    key: EndpointId,
    me: &str,
    service: &str,
    credential: (&str, &str),
) -> anyhow::Result<String> {
    anyhow::ensure!(
        protocol::is_service_name(service),
        "{service:?} is not a service name, so no grant could name it"
    );

    let fleet = request(shared, key, credential, "GET", "/api/fleet", None).await?;
    let (petname, _) = find_me(&fleet, me).with_context(|| {
        format!(
            "this node no longer lists this phone (key {me}) as paired, so it has no peer to \
             grant — pair again"
        )
    })?;

    let body = serde_json::to_vec(&serde_json::json!({
        "petname": petname,
        "grant": format!("http:{service}"),
    }))?;
    request(
        shared,
        key,
        credential,
        "POST",
        "/api/fleet/grants/add",
        Some(&body),
    )
    .await?;
    info!(%service, %petname, "the node now lets this phone open a dashboard");
    Ok(petname)
}

// ---------------------------------------------------------------------------------------
// One HTTP request, over the mesh
// ---------------------------------------------------------------------------------------

/// Send one request to the node's control panel and return its body.
///
/// The pool is retired on a *transport* failure and only then. An HTTP status — a `401`, a `404`
/// from an older panel — proves the connection carried a full round trip, so throwing it away
/// would cost a redial to learn nothing. A timeout is the opposite: it is what a connection the
/// OS froze looks like from here (see [`STEP_TIMEOUT`]), and leaving that in the pool makes the
/// next call hang exactly as this one did.
async fn request(
    shared: &Shared,
    key: EndpointId,
    credential: (&str, &str),
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> anyhow::Result<Vec<u8>> {
    let raw = match timeout(STEP_TIMEOUT, exchange(shared, key, credential, method, path, body))
        .await
    {
        Ok(Ok(raw)) => raw,
        Ok(Err(e)) => {
            shared.reset_pool();
            return Err(e);
        }
        Err(_) => {
            shared.reset_pool();
            anyhow::bail!(
                "this node did not answer in time — if the app has just come back from the \
                 background, try again: the next attempt dials fresh"
            );
        }
    };

    let reply = parse_reply(&raw)?;
    match reply.status {
        200..=299 => Ok(reply.body),
        401 => anyhow::bail!(
            "this node did not accept the password stored for it — pair again to rotate it"
        ),
        404 => anyhow::bail!(
            "this node's control panel has no {path} — it is older than this app"
        ),
        status => anyhow::bail!(
            "this node's control panel answered {status}: {}",
            snippet(&reply.body)
        ),
    }
}

/// The wire half of [`request`]: a bi-stream, the service frame, the head, and the reply.
async fn exchange(
    shared: &Shared,
    key: EndpointId,
    credential: (&str, &str),
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> anyhow::Result<Vec<u8>> {
    let conn = shared.pool().get(key).await.context("reaching the node")?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("opening a stream to the node")?;

    protocol::write_http_request(&mut send, PANEL).await?;
    match protocol::read_http_status(&mut recv).await? {
        HttpStatus::Ok => {}
        refused => anyhow::bail!("{}", refused.reason()),
    }

    send.write_all(&head(method, path, credential, body)).await?;
    if let Some(body) = body {
        send.write_all(body).await?;
    }
    // The request is complete, and saying so is what makes the node's splice shut its upstream
    // write half — without it the panel would sit waiting for a body that is never coming.
    send.finish().context("finishing the request")?;

    let mut raw = Vec::new();
    recv.take(MAX_REPLY)
        .read_to_end(&mut raw)
        .await
        .context("reading the node's answer")?;
    Ok(raw)
}

/// The request head. `Connection: close` matches what the panel answers with anyway
/// (`adi-app/src/http.rs`), and is what lets the reply be read to end-of-stream rather than
/// parsed for a length that a chunked answer would not carry.
fn head(method: &str, path: &str, credential: (&str, &str), body: Option<&[u8]>) -> Vec<u8> {
    let (username, password) = credential;
    let authorization = B64.encode(format!("{username}:{password}"));
    // Declared only when there is one: a `Content-Length: 0` on a GET is legal but says something
    // about the request that is not true of it.
    let framing = body.map_or_else(String::new, |body| {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    });
    format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Basic {authorization}\r\n\
         Accept: application/json\r\n\
         Connection: close\r\n\
         {framing}\r\n"
    )
    .into_bytes()
}

/// A parsed HTTP response: the status, and everything after the blank line.
#[derive(Debug, PartialEq, Eq)]
struct HttpReply {
    status: u16,
    body: Vec<u8>,
}

/// Split a response into its status and its body.
fn parse_reply(raw: &[u8]) -> anyhow::Result<HttpReply> {
    anyhow::ensure!(
        !raw.is_empty(),
        "this node closed the connection without answering"
    );
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("this node's answer was not a complete HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .context("this node's answer carried no status code")?;
    Ok(HttpReply {
        status,
        body: raw[head_end + 4..].to_vec(),
    })
}

/// The first line or so of an unexpected body, for an error a person reads on a phone.
fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    match text.char_indices().nth(200) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------------------

/// This phone's petname and grants on a node, out of its fleet page.
///
/// Matched **by key**, never by name: the node names this phone whatever it likes, and the key is
/// the only identity of record (§2).
fn find_me(fleet: &[u8], me: &str) -> Option<(String, Vec<Grant>)> {
    let fleet: FleetReply = serde_json::from_slice(fleet).ok()?;
    let peer = fleet.nodes.into_iter().find(|peer| peer.key == me)?;
    let grants = peer
        .grants
        .iter()
        // An unparseable grant is one rule this build does not know, not a reason to report the
        // peer as holding nothing.
        .filter_map(|raw| raw.parse::<Grant>().ok())
        .collect();
    Some((peer.petname, grants))
}

/// Turn the panel's listing into the rows the UI shows.
///
/// Archived dashboards are dropped: archiving takes both of a dashboard's services out of the
/// supervisor's imports, so its host resolves to nothing and a row for it could only ever fail.
fn assemble(dashboards: Vec<RawDashboard>, grants: &[Grant]) -> Vec<DashboardInfo> {
    dashboards
        .into_iter()
        .filter(|dashboard| dashboard.archived_at.is_none())
        .map(|dashboard| {
            let service = dashboard.host.as_deref().and_then(service_label);
            let allowed = service.as_deref().is_some_and(|label| {
                grants.iter().any(|grant| grant.allows(Target::Http(label)))
            });
            DashboardInfo {
                name: dashboard.name.unwrap_or_else(|| dashboard.id.clone()),
                id: dashboard.id,
                description: dashboard.description,
                service,
                running: dashboard.frontend_running,
                allowed,
            }
        })
        .collect()
}

/// The service name inside a dashboard's host: `nosh.adi` → `nosh`, `app.nosh.adi` → `app.nosh`.
///
/// Everything left of the local zone, because that is what the node's gateway resolves — it asks
/// its route table for `<service>.adi` (`adi-mesh/src/gateway.rs`, `resolve`), and a node's own
/// hosts are not all one label. A dashboard published under a real domain answers there, not here,
/// so it gets no name rather than a guess that would route to nothing.
fn service_label(host: &str) -> Option<String> {
    let name = host.strip_suffix(LOCAL_ZONE)?.strip_suffix('.')?;
    protocol::is_service_name(name).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(raw: &[&str]) -> Vec<Grant> {
        raw.iter().map(|g| g.parse().expect("a valid grant")).collect()
    }

    fn dashboard(id: &str, host: Option<&str>) -> RawDashboard {
        RawDashboard {
            id: id.to_string(),
            name: Some(id.to_uppercase()),
            description: None,
            host: host.map(ToString::to_string),
            frontend_running: true,
            archived_at: None,
        }
    }

    #[test]
    fn a_head_carries_the_credential_and_declares_its_body() {
        let head = String::from_utf8(head(
            "POST",
            "/api/fleet/grants/add",
            ("adi", "hunter2"),
            Some(br#"{"petname":"phone"}"#),
        ))
        .expect("utf-8");

        assert!(head.starts_with("POST /api/fleet/grants/add HTTP/1.1\r\n"));
        // `adi:hunter2` — the header a browser would have sent, since the node's gate is the same
        // one either way (§5).
        assert!(
            head.contains("Authorization: Basic YWRpOmh1bnRlcjI=\r\n"),
            "the credential must travel as Basic: {head}"
        );
        assert!(head.contains("Content-Length: 19\r\n"), "{head}");
        assert!(head.ends_with("\r\n\r\n"), "the head must be terminated");
    }

    #[test]
    fn a_body_less_head_declares_no_length() {
        let head = String::from_utf8(head("GET", "/api/dashboards", ("adi", "x"), None))
            .expect("utf-8");
        assert!(!head.contains("Content-Length"), "{head}");
        assert!(head.contains("Connection: close\r\n"), "{head}");
    }

    #[test]
    fn a_reply_splits_into_a_status_and_a_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"dashboards\":[]}";
        assert_eq!(
            parse_reply(raw).expect("parses"),
            HttpReply {
                status: 200,
                body: br#"{"dashboards":[]}"#.to_vec(),
            }
        );
    }

    #[test]
    fn a_truncated_or_empty_reply_is_an_error_and_not_a_guess() {
        // Nothing at all: the node accepted the stream and then said nothing.
        assert!(parse_reply(b"").is_err());
        // A head that never ends — reading further would only buffer more of the same.
        assert!(parse_reply(b"HTTP/1.1 200 OK\r\nContent-Type: application/json").is_err());
        // A status line without a code.
        assert!(parse_reply(b"HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn a_body_with_a_blank_line_in_it_keeps_every_byte() {
        // The split is on the *first* terminator only; a JSON body containing `\r\n\r\n` (or a
        // 502 page, which does) must come back whole.
        let raw = b"HTTP/1.1 502 Bad Gateway\r\n\r\nfirst\r\n\r\nsecond";
        let reply = parse_reply(raw).expect("parses");
        assert_eq!(reply.status, 502);
        assert_eq!(reply.body, b"first\r\n\r\nsecond".to_vec());
    }

    #[test]
    fn a_host_yields_the_label_the_node_routes_on() {
        assert_eq!(service_label("nosh.adi").as_deref(), Some("nosh"));
        assert_eq!(service_label("app.adi").as_deref(), Some("app"));
        // A node's own hosts are not all one label, and the whole name is what it resolves.
        assert_eq!(service_label("app.nosh.adi").as_deref(), Some("app.nosh"));
        // A real domain answers at its own front door, not over the mesh's local zone.
        assert_eq!(service_label("nosh.example.com"), None);
        // Neither a bare name nor an upper-case one is a label a gateway would resolve.
        assert_eq!(service_label("nosh"), None);
        assert_eq!(service_label("Nosh.adi"), None);
        assert_eq!(service_label(".adi"), None);
    }

    #[test]
    fn a_listing_becomes_rows_and_the_grants_decide_which_are_open() {
        let rows = assemble(
            vec![
                dashboard("nosh", Some("nosh.adi")),
                dashboard("books", Some("books.adi")),
                // Declares no host at all: listed, but nothing to open.
                dashboard("draft", None),
            ],
            &grants(&["http:app", "http:nosh"]),
        );

        assert_eq!(rows.len(), 3, "every live dashboard is listed");
        assert_eq!(rows[0].name, "NOSH");
        assert!(rows[0].allowed, "http:nosh covers it");
        assert!(!rows[1].allowed, "no grant names `books`");
        assert_eq!(rows[2].service, None, "no host is no label");
        assert!(!rows[2].allowed, "and nothing unroutable is ever open");
    }

    #[test]
    fn a_wildcard_grant_opens_every_dashboard() {
        let rows = assemble(
            vec![dashboard("nosh", Some("nosh.adi")), dashboard("books", Some("books.adi"))],
            &grants(&["http:*"]),
        );
        assert!(rows.iter().all(|row| row.allowed), "http:* covers the lot");
    }

    #[test]
    fn an_archived_dashboard_is_not_offered() {
        let archived = RawDashboard {
            archived_at: Some(1),
            ..dashboard("old", Some("old.adi"))
        };
        let rows = assemble(vec![archived, dashboard("nosh", Some("nosh.adi"))], &grants(&["http:*"]));
        assert_eq!(rows.len(), 1, "archiving stops the services; the row would only fail");
        assert_eq!(rows[0].id, "nosh");
    }

    #[test]
    fn a_dashboard_without_a_name_falls_back_to_its_directory() {
        let unnamed = RawDashboard {
            name: None,
            ..dashboard("nosh", Some("nosh.adi"))
        };
        assert_eq!(assemble(vec![unnamed], &[])[0].name, "nosh");
    }

    #[test]
    fn this_phone_finds_itself_in_a_fleet_page_by_key_alone() {
        let fleet = br#"{"nodes":[
            {"petname":"desk","key":"aaaa","grants":["http:*"],"nickname":"desk",
             "paired_at":1,"has_password":true},
            {"petname":"pocket","key":"bbbb","grants":["http:app","tcp:127.0.0.1:22"],
             "nickname":"pocket","paired_at":2,"has_password":true}
        ]}"#;

        let (petname, grants) = find_me(fleet, "bbbb").expect("this phone is listed");
        assert_eq!(petname, "pocket", "the node's name for us, not ours for it");
        assert_eq!(grants.len(), 2, "every grant it holds, whatever kind");
        assert!(grants.iter().any(|g| g.allows(Target::Http("app"))));

        assert!(find_me(fleet, "cccc").is_none(), "an unpaired key is not there");
        assert!(find_me(b"not json", "bbbb").is_none(), "and neither is a bad page");
    }

    #[test]
    fn an_unparseable_grant_does_not_cost_the_peer_its_others() {
        let fleet = br#"{"nodes":[{"petname":"pocket","key":"bbbb",
            "grants":["http:app","quantum:entangle"],"nickname":"p","paired_at":1,
            "has_password":true}]}"#;
        let (_, grants) = find_me(fleet, "bbbb").expect("listed");
        assert_eq!(grants.len(), 1, "the rule we do not know is skipped, not fatal");
        assert!(grants[0].allows(Target::Http("app")));
    }

    #[test]
    fn a_listing_from_a_panel_that_predates_a_field_still_reads() {
        // Only `id` is load-bearing; the rest of the DTO may be older or newer than this build.
        let listing: DashboardsReply =
            serde_json::from_slice(br#"{"dashboards":[{"id":"nosh","name":"Nosh"}]}"#)
                .expect("an older payload still deserializes");
        let rows = assemble(listing.dashboards, &grants(&["http:*"]));
        assert_eq!(rows[0].id, "nosh");
        assert_eq!(rows[0].service, None, "no host declared is no service to open");
        assert!(!rows[0].running);
    }

    /// A real `GET /api/dashboards` row, kept verbatim.
    ///
    /// This crate cannot depend on `adi-webapp-api` for the panel's own DTO — that would pull the
    /// whole control panel into a phone's staticlib — so the field names here are a copy, and a
    /// copy is a thing that drifts. Every `#[serde(default)]` above means a renamed field costs a
    /// row its detail *silently*: a moved `host` would read as "no address on this node yet" on
    /// every dashboard, with nothing failing. This is the sample that says what the wire looked
    /// like when the copy was made.
    #[test]
    fn a_row_from_a_live_panel_reads_field_for_field() {
        let listing: DashboardsReply = serde_json::from_slice(
            br#"{"dashboards":[{"id":"02dc9d07-cdf8-4fea-aa8c-8eedde240151",
            "dir":"/Users/x/.adi/mono/dashboards/02dc9d07-cdf8-4fea-aa8c-8eedde240151",
            "name":"Bugbounty","description":"Bug bounty control panel","project":"bugbounty",
            "host":"bugbounty.adi","frontend_port":8010,"backend_port":8009,
            "frontend_running":true,"backend_running":true,"modules":["board"],
            "routes":["agent","target"],"archived_at":null}]}"#,
        )
        .expect("a live payload deserializes");

        let rows = assemble(listing.dashboards, &grants(&["http:app"]));
        assert_eq!(rows[0].name, "Bugbounty");
        assert_eq!(rows[0].service.as_deref(), Some("bugbounty"), "host → label");
        assert!(rows[0].running, "frontend_running is what the row reports");
        assert!(
            !rows[0].allowed,
            "the grant a pairing actually leaves behind is `http:app` and nothing else (§8), \
             which is exactly why opening a dashboard has to ask first"
        );
    }

    #[test]
    fn a_snippet_is_bounded_and_never_splits_a_character() {
        assert_eq!(snippet(b"  502 Bad Gateway\n "), "502 Bad Gateway");
        let long = "\u{e9}".repeat(400);
        let cut = snippet(long.as_bytes());
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(cut.chars().count(), 201, "200 characters and the ellipsis");
    }
}
