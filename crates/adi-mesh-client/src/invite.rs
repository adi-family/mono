//! Pairing, from the side that dials — `adi/mesh/join/1` (`docs/fleet.md` §8).
//!
//! # Why this direction, and not the one the contract first described
//!
//! §8 was written for the case it was built for: enrolling a headless node without opening a port
//! on it. The **node dials the viewer**, spends a nonce out of an invite the viewer minted, and is
//! filed in the viewer's registry. That works because a viewer is a machine somebody is sitting at,
//! and machines can be dialled.
//!
//! A tab cannot. It registers no ALPN, publishes no address, and is behind whatever the browser's
//! network stack decides — nothing on this mesh has ever dialled a browser, and a client that
//! required it would be a client that pairs only when the phone happens to be reachable.
//!
//! So the roles are swapped, and **the wire protocol did not have to change to allow it**. Read
//! `adi-mesh/src/join.rs` as the symmetric thing it is: the side that *mints the invite* accepts a
//! stream and files the dialler; the side that *spends it* dials and is filed. Which of them is
//! called "the node" is a story about who ran which command, not a fact the bytes carry. So:
//!
//! ```text
//! node:     adi-mono mesh invite          -> adi-invite:<hex(json)>   (carries the NODE's ticket)
//! browser:  paste it into this client     -- dials the node on adi/mesh/join/1
//!             browser -> node   { v, nonce, nickname }
//!             node -> browser   { result: accepted, petname, username, password, grants }
//! ```
//!
//! The node's `decide()` then does exactly what pairing must do for a browser to be any use: it
//! files **this tab's key** in the node's own `fleet.toml`, grants it `http:app` — §8's default,
//! the node's control panel and nothing else — and mints the password its Basic gate will demand.
//! Every property §8 claims still holds, and holds for the same reason: the key is
//! `Connection::remote_id()` and never a payload field, the nonce is single-use and spent before
//! the reply goes out, and the password exists in plaintext exactly once, in that reply.
//!
//! Two things are *not* the same, and both are consequences of who is who rather than changes:
//!
//! * The `petname` in the reply is what the **node** decided to call this browser. It is not a
//!   name for the node, and this client does not store it as one — §2 makes a petname local, so the
//!   reader names the node here.
//! * The node learns nothing about the browser but its key and its offered nickname. There is no
//!   reverse record to write: a browser serves nothing, so there is no grant it could give.
//!
//! # Why the wire types are declared here rather than included
//!
//! [`crate::protocol`] is `#[path]`-included from `adi-mesh` because that file is self-contained.
//! `join.rs` is not: its payloads name [`Grant`](https://docs.rs/), which lives in `fleet.rs` beside
//! the registry and `adi_config::Config`, neither of which exists in a browser. What is restated
//! below is the *frame* (`[len: u32 BE][json]`, capped at 64 KiB) and the four JSON shapes, with
//! grants read as the strings they serialise to — `Grant` is `#[serde(try_from = "String", into =
//! "String")]`, so `Vec<String>` here is the same bytes as `Vec<Grant>` there. `adi-mesh` remains
//! the owner of the meaning; this is a reader of the wire.

use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::mesh::{Mesh, Result, with_timeout};

/// Marks a string as an adi invite. Owned by [`crate::token`], which is also where the reading of
/// a pasted one lives.
use crate::token::PREFIX as INVITE_PREFIX;

/// Marks a string as an `adimesh:` endpoint ticket — what an invite carries inside it.
const TICKET_PREFIX: &str = "adimesh:";

/// The invite and handshake version, mirrored in every payload's `v`.
const VERSION: u8 = 1;

/// Caps a single JSON frame, so a peer that claims a huge length cannot make us allocate for it.
const MAX_FRAME: usize = 64 * 1024;

/// The ALPN pairing is spoken on. Its own, and not a frame inside `adi/mesh/http/1`, because it is
/// the one exchange a node answers from a key its registry has never seen.
pub const JOIN_ALPN: &[u8] = b"adi/mesh/join/1";

/// The payload inside an `adi-invite:` token.
#[derive(Debug, Clone, Deserialize)]
pub struct Invite {
    /// The protocol version.
    pub v: u8,
    /// The minting machine's `adimesh:` ticket — id plus relay and direct addresses.
    pub endpoint: String,
    /// 32 hex characters of one-time secret.
    pub nonce: String,
    /// Unix seconds after which it is refused.
    pub expires: u64,
}

/// What this tab sends: the nonce that authorises the pairing, and the name it would like.
///
/// No key. The key is the QUIC handshake's `remote_id`; a field for it here would be a field an
/// attacker fills in.
#[derive(Debug, Clone, Serialize)]
struct JoinRequest {
    v: u8,
    nonce: String,
    nickname: String,
}

/// What the node answers with when it pairs.
#[derive(Debug, Clone, Deserialize)]
pub struct Accepted {
    /// What the **node** now calls this browser — not a name for the node (§2).
    pub petname: String,
    /// The username its Basic gate will want.
    pub username: String,
    /// The password, sent exactly once. Nothing on either side stores it in plaintext.
    pub password: String,
    /// What this browser may now reach there — `["http:app"]` from a fresh pairing.
    #[serde(default)]
    pub grants: Vec<String>,
}

/// The node's answer.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum JoinReply {
    /// Paired.
    Accepted(Accepted),
    /// Not paired, and why — a sentence, because the three things that go wrong (wrong fleet,
    /// stale token, token already spent) have three different fixes.
    Refused {
        /// The node's own words.
        reason: String,
    },
}

/// Decode an `adi-invite:` token.
///
/// Takes what was pasted and not only what was minted — see [`crate::token`] for the shapes a
/// copy arrives in and why forgiving them is the tab's problem too.
///
/// Expiry is **not** checked here. The authority on that is the node's own invite book, which is
/// the side that minted it; refusing locally against a browser's clock would turn a phone whose
/// time is a minute fast into a phone that cannot pair.
///
/// # Errors
/// If the string is not an invite, is not valid hex, does not decode to an invite, or names a
/// version this build does not speak.
pub fn decode_invite(token: &str) -> Result<Invite> {
    let mut refusal = None;
    for candidate in crate::token::candidates(token) {
        match decode_exact(&candidate) {
            Ok(invite) => return Ok(invite),
            // The first reading's complaint, since the readings are ordered by confidence.
            Err(refused) => drop(refusal.get_or_insert(refused)),
        }
    }
    Err(refusal.unwrap_or_else(|| {
        format!(
            "that is not an adi invite — it should start with `adi-invite:`, but {}",
            crate::token::describe(token)
        )
    }))
}

/// [`decode_invite`] for one already-canonical token.
fn decode_exact(token: &str) -> Result<Invite> {
    let hex = token
        .trim()
        .strip_prefix(INVITE_PREFIX)
        .ok_or("that is not an adi invite — it should start with `adi-invite:`")?;
    let bytes = from_hex(hex)?;
    let invite: Invite = serde_json::from_slice(&bytes)
        .map_err(|e| format!("the invite does not decode to an invite payload: {e}"))?;
    if invite.v != VERSION {
        return Err(format!(
            "this client speaks invite v{VERSION}, but the token is v{}",
            invite.v
        ));
    }
    Ok(invite)
}

/// Spend `invite` against the machine that minted it, and return what it granted.
///
/// One bi-stream, one frame each way, and the connection is dropped afterwards: a pairing is not
/// something to hold a connection open for.
///
/// # Errors
/// If the invite's endpoint does not parse, the node cannot be dialled, or it refuses.
pub async fn join(
    mesh: &Mesh,
    invite: &Invite,
    nickname: &str,
) -> Result<(EndpointAddr, Accepted)> {
    let addr = parse_ticket(&invite.endpoint)
        .map_err(|e| format!("the invite does not name a reachable endpoint: {e}"))?;
    let conn = mesh.connect(&addr, JOIN_ALPN).await?;
    let (mut send, mut recv) = with_timeout("opening the pairing stream", conn.open_bi())
        .await?
        .map_err(|e| format!("opening the pairing stream failed: {e}"))?;

    write_frame(
        &mut send,
        &JoinRequest {
            v: VERSION,
            nonce: invite.nonce.clone(),
            nickname: nickname.to_string(),
        },
    )
    .await?;
    let _ = send.finish();

    let reply: JoinReply =
        with_timeout("waiting for the node's answer", read_frame(&mut recv)).await??;
    conn.close(0u32.into(), b"join complete");
    // The pooled entry would otherwise be a join-ALPN connection that every later HTTP request
    // reused and every one of them failed on.
    mesh.drop_connection(addr.id);

    match reply {
        JoinReply::Accepted(accepted) => Ok((addr, accepted)),
        JoinReply::Refused { reason } => Err(format!("the node refused to pair: {reason}")),
    }
}

/// Wait until the node will actually admit this browser, or give up saying why.
///
/// **A pairing is not usable the instant it is accepted, and this is the reason.** The node's
/// gateway serves from an in-memory snapshot of `fleet.toml` and re-reads it on a timer —
/// `RELOAD_INTERVAL`, five seconds (`adi-mesh/src/gateway.rs`). The join handshake writes the file
/// and answers immediately, so for up to those five seconds `admit` is still consulting a registry
/// that has never heard of this key, and it refuses with exactly the sentence it would use for a
/// stranger: *this machine holds no grant for that service*.
///
/// Measured, not reasoned: without this the first panel open after pairing failed every time.
///
/// The wait lives here rather than as a retry inside [`Mesh::open`](crate::mesh::Mesh::open)
/// deliberately. Only pairing knows the refusal is expected; everywhere else a `NotAuthorized` is
/// the truth and retrying it would turn a clear error into a slow one.
///
/// # Errors
/// The node's own refusal, if it is still refusing after [`ADMIT_WINDOW`].
pub async fn wait_until_admitted(mesh: &Mesh, addr: &EndpointAddr, service: &str) -> Result<()> {
    let deadline = crate::now_ms() + f64::from(ADMIT_WINDOW.as_secs_f32() * 1000.0);
    loop {
        match mesh.open(addr, service).await {
            // Admitted. Nothing is sent on the stream — the question was whether it would open at
            // all — and dropping it resets a stream the node had only just accepted.
            Ok(_) => return Ok(()),
            Err(refusal) if refusal == NOT_AUTHORIZED && crate::now_ms() < deadline => {
                n0_future::time::sleep(ADMIT_POLL).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// How long to keep asking a freshly paired node to admit us. Comfortably over the node's own
/// five-second reload, so the window closes on the node's terms rather than on a race.
const ADMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(12);

/// How often to ask again inside that window.
const ADMIT_POLL: std::time::Duration = std::time::Duration::from_millis(750);

/// `HttpStatus::NotAuthorized`'s reason, which is what [`Mesh::open`](crate::mesh::Mesh::open)
/// returns as its error. Compared as a string because the status is the node's answer and the
/// reason is what it means; both come from `adi-mesh`'s own `protocol.rs`, included here.
const NOT_AUTHORIZED: &str = "this machine holds no grant for that service";

/// A nickname to offer a node for this browser: `browser-<10 hex of the key>`.
///
/// Derived from the key rather than asked for, because it is what the *node's* operator will see
/// in their fleet list and a name they can tie back to a key is worth more there than a name the
/// reader typed on a phone. One DNS label, which is what §2 requires.
#[must_use]
pub fn nickname_for(key: EndpointId) -> String {
    format!("browser-{}", key.fmt_short())
}

/// The [`SecretKey`] 64 hex characters name.
///
/// # Errors
/// If the string is not exactly 64 hex characters.
pub fn secret_from_hex(text: &str) -> Result<SecretKey> {
    let bytes = from_hex(text.trim())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "a secret key is 64 hex characters".to_string())?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// The hex form of a secret key, for the store.
#[must_use]
pub fn secret_to_hex(secret: &SecretKey) -> String {
    to_hex(&secret.to_bytes())
}

/// An address to dial: an `adimesh:` ticket, or a bare endpoint id plus the relay it calls home.
///
/// # Errors
/// If neither form parses.
pub fn addr_from(node: &str, relay: &str) -> Result<EndpointAddr> {
    let node = node.trim();
    if node.starts_with(TICKET_PREFIX) {
        return parse_ticket(node);
    }
    let id: EndpointId = node
        .parse()
        .map_err(|e| format!("{node:?} is neither an adimesh ticket nor an endpoint id: {e}"))?;
    let relay = relay.trim();
    if relay.is_empty() {
        return Ok(EndpointAddr::new(id));
    }
    let relay: RelayUrl = relay
        .parse()
        .map_err(|e| format!("the relay {relay:?} does not parse as a URL: {e}"))?;
    Ok(EndpointAddr::new(id).with_relay_url(relay))
}

/// Decode an `adimesh:` ticket into the address it names.
fn parse_ticket(token: &str) -> Result<EndpointAddr> {
    let hex = token
        .trim()
        .strip_prefix(TICKET_PREFIX)
        .ok_or("not an adimesh ticket")?;
    let bytes = from_hex(hex)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("the ticket does not decode to an endpoint address: {e}"))
}

/// Write one length-prefixed JSON frame: `[len: u32 BE][json]`.
///
/// Length-prefixed rather than "write JSON and close": both directions of this handshake live on
/// one bi-stream, and a reader that waited for EOF could not also expect a reply.
async fn write_frame<W, T>(w: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let json = serde_json::to_vec(value).map_err(|e| format!("serialising the frame: {e}"))?;
    let len = u32::try_from(json.len()).map_err(|_| "the frame is too long".to_string())?;
    w.write_all(&len.to_be_bytes())
        .await
        .map_err(|e| format!("writing the frame length: {e}"))?;
    w.write_all(&json)
        .await
        .map_err(|e| format!("writing the frame: {e}"))?;
    w.flush()
        .await
        .map_err(|e| format!("flushing the frame: {e}"))
}

/// Read one length-prefixed JSON frame.
async fn read_frame<R, T>(r: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .await
        .map_err(|e| format!("reading the frame length: {e}"))?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(format!("the node's answer claims {len} bytes"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .await
        .map_err(|e| format!("reading the frame: {e}"))?;
    serde_json::from_slice(&buf).map_err(|e| format!("the node's answer does not parse: {e}"))
}

/// Hex, lowercase.
fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// The inverse of [`to_hex`].
fn from_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| format!("invalid hex at byte {}", i / 2))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 15, 16, 127, 128, 254, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).expect("decode"), bytes);
        assert!(from_hex("abc").is_err(), "odd length is refused");
        assert!(from_hex("zz").is_err(), "non-hex is refused");
    }

    #[test]
    fn an_invite_decodes_and_a_wrong_version_does_not() {
        let json = br#"{"v":1,"endpoint":"adimesh:00","nonce":"aa","expires":9}"#;
        let token = format!("{INVITE_PREFIX}{}", to_hex(json));
        let invite = decode_invite(&token).expect("decode");
        assert_eq!(invite.nonce, "aa");
        assert_eq!(invite.endpoint, "adimesh:00");

        let future = br#"{"v":2,"endpoint":"adimesh:00","nonce":"aa","expires":9}"#;
        let token = format!("{INVITE_PREFIX}{}", to_hex(future));
        assert!(
            decode_invite(&token).is_err(),
            "a future version is refused"
        );

        assert!(
            decode_invite("adimesh:00").is_err(),
            "a ticket is not an invite"
        );
    }

    #[test]
    fn an_expired_invite_still_decodes_here() {
        // The node's invite book is the authority on expiry (see `decode_invite`). Refusing it
        // locally would make a phone with a fast clock unable to pair against a valid token.
        let json = br#"{"v":1,"endpoint":"adimesh:00","nonce":"aa","expires":0}"#;
        let token = format!("{INVITE_PREFIX}{}", to_hex(json));
        assert!(decode_invite(&token).is_ok());
    }

    #[test]
    fn a_reply_reads_grants_as_the_strings_they_serialise_to() {
        // `Grant` is `#[serde(into = "String")]` on the far side, so this is the same bytes.
        let reply: JoinReply = serde_json::from_str(
            r#"{"result":"accepted","petname":"browser-aa","username":"adi",
                "password":"s3cret","grants":["http:app"]}"#,
        )
        .expect("parse");
        match reply {
            JoinReply::Accepted(accepted) => {
                assert_eq!(accepted.grants, ["http:app"]);
                assert_eq!(accepted.username, "adi");
            }
            JoinReply::Refused { .. } => panic!("accepted"),
        }

        let refused: JoinReply =
            serde_json::from_str(r#"{"result":"refused","reason":"that invite has expired"}"#)
                .expect("parse");
        assert!(matches!(refused, JoinReply::Refused { .. }));
    }

    #[test]
    fn a_nickname_is_one_dns_label() {
        let key = SecretKey::from_bytes(&[7u8; 32]).public();
        let nickname = nickname_for(key);
        assert!(
            crate::protocol::is_dns_label(&nickname),
            "{nickname:?} must satisfy the rule the node validates against"
        );
    }

    #[test]
    fn a_secret_key_round_trips_through_its_hex() {
        let secret = SecretKey::from_bytes(&[3u8; 32]);
        let text = secret_to_hex(&secret);
        assert_eq!(text.len(), 64);
        assert_eq!(
            secret_from_hex(&text).expect("parse").public(),
            secret.public()
        );
        assert!(secret_from_hex("nope").is_err());
    }

    #[test]
    fn an_address_comes_from_a_bare_key_plus_a_relay() {
        let id = SecretKey::from_bytes(&[5u8; 32]).public();
        let addr = addr_from(&id.to_string(), "https://mad.mono-relay.withadi.dev").expect("addr");
        assert_eq!(addr.id, id);
        assert!(!addr.addrs.is_empty(), "the relay travels with the address");

        let bare = addr_from(&id.to_string(), "").expect("addr");
        assert!(bare.addrs.is_empty());
        assert!(addr_from("not-a-key", "").is_err());
    }
}
