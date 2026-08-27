//! A WebSocket **client**, RFC 6455, spoken over a mesh [`Stream`](crate::mesh::Stream).
//!
//! The control panel's live channel is one websocket per tab (`adi-webapp/src/live.rs`), and a
//! websocket is the one thing a browser cannot be made to route through anything: `new WebSocket()`
//! goes to the platform's own network stack, a service worker's `fetch` event never sees it, and
//! there is no interception point anywhere in between. So if the panel's socket is to cross the
//! mesh, *this* has to be the client — the tab speaks RFC 6455 itself over the QUIC stream, and the
//! page's `new WebSocket()` is answered by a shim that talks to it (see `js/panel-shim.js`).
//!
//! The node needs no part in this. `adi/mesh/http/1` splices raw bytes both ways once a stream is
//! admitted (`adi-mesh/src/tunnel.rs`), and `gateway::negotiate` explicitly exempts an upgrade from
//! the `Connection: close` it forces on a carved host — so an upgrade handshake and everything
//! after it is just bytes on a stream that was already going to carry bytes.
//!
//! Only what a client needs is here: masked frames out (§5.3 requires it), any frame in,
//! continuation reassembled, ping answered. There is no server half and no permessage-deflate.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::http::{Head, Request};
use crate::mesh::{Reader, Result, Stream};

/// The GUID RFC 6455 §1.3 has both sides append to the client's key.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The most a single inbound message may be before the connection is treated as hostile.
///
/// Matches `adi-app`'s own server-side ceiling (`ws.rs`, `MAX_PAYLOAD`), because the panel is what
/// this carries and a message it would refuse to send is one we need not be able to receive.
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// What kind of frame this is. Only the four a client acts on; the rest are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    /// A UTF-8 message.
    Text,
    /// A binary message.
    Binary,
    /// The peer is closing.
    Close,
    /// A liveness probe, to be answered with a pong.
    Ping,
    /// The answer to one.
    Pong,
}

/// One inbound message, already reassembled from its continuation frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// What it is.
    pub opcode: Opcode,
    /// Its payload.
    pub payload: Vec<u8>,
}

/// Perform the opening handshake on an already-open mesh stream.
///
/// `origin` is sent because the panel refuses an `/api/ws` whose `Origin` names another site, and
/// a websocket handshake is the one request where that header is the *only* guard available
/// (`adi-app/src/origin.rs`). It must therefore name the host the request is addressed to — which
/// over the mesh is whatever `Host` this request carries.
///
/// # Errors
/// If the peer does not answer `101`, or answers with a `Sec-WebSocket-Accept` that is not the
/// digest of the key we sent. The second check is not ceremony: it is exactly the check that
/// caught n0's relay answering an upgrade it had not really understood (`docs/fleet.md` §9).
pub async fn handshake(stream: &mut Stream, request: Request) -> Result<Head> {
    let key = B64.encode(crate::random_bytes::<16>());
    let request = request
        .with("Upgrade", "websocket")
        .with("Connection", "Upgrade")
        .with("Sec-WebSocket-Key", &key)
        .with("Sec-WebSocket-Version", "13");
    stream.write(&request.encode()).await?;

    let head = Head::parse(&stream.read_head().await?)?;
    if head.status != 101 {
        return Err(format!(
            "the service answered {} {} to a websocket upgrade",
            head.status, head.reason
        ));
    }
    let expected = accept_for(&key);
    match head.get("sec-websocket-accept") {
        Some(got) if got == expected => Ok(head),
        Some(got) => Err(format!(
            "the service answered 101 with sec-websocket-accept {got:?}, not {expected:?}"
        )),
        None => Err("the service answered 101 with no sec-websocket-accept".into()),
    }
}

/// The `Sec-WebSocket-Accept` value for a client key (RFC 6455 §4.2.2 step 5).
#[must_use]
pub fn accept_for(key: &str) -> String {
    B64.encode(sha1(format!("{key}{GUID}").as_bytes()))
}

/// Encode one frame, masked as §5.3 requires of every client frame.
#[must_use]
pub fn encode(opcode: Opcode, payload: &[u8]) -> Vec<u8> {
    let code = match opcode {
        Opcode::Text => 0x1,
        Opcode::Binary => 0x2,
        Opcode::Close => 0x8,
        Opcode::Ping => 0x9,
        Opcode::Pong => 0xA,
    };
    let mut out = Vec::with_capacity(payload.len() + 14);
    // FIN set: this client never fragments what it sends. Nothing it sends is big enough for
    // fragmentation to buy anything, and a receiver must accept an unfragmented message anyway.
    out.push(0x80 | code);
    let mask_bit = 0x80;
    match payload.len() {
        len if len < 126 => out.push(mask_bit | u8::try_from(len).unwrap_or(125)),
        len if u16::try_from(len).is_ok() => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&u16::try_from(len).unwrap_or(u16::MAX).to_be_bytes());
        }
        len => {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    let mask = crate::random_bytes::<4>();
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Read one whole message, reassembling continuation frames.
///
/// Returns `None` at end of stream. A `Ping` is returned rather than answered here, so the caller
/// owns the write half and there is no second writer to interleave with.
///
/// # Errors
/// A reserved opcode, a masked frame from a server (§5.1 forbids it), or an oversized payload.
pub async fn read_message(stream: &mut Reader) -> Result<Option<Message>> {
    let mut assembled: Option<Message> = None;
    loop {
        let Some((fin, code, payload)) = read_frame(stream).await? else {
            return Ok(None);
        };
        // A control frame may arrive *between* the fragments of a message and is never part of it.
        if code >= 0x8 {
            return Ok(Some(Message {
                opcode: opcode_of(code)?,
                payload,
            }));
        }
        match (&mut assembled, code) {
            (None, 0x0) => return Err("a continuation frame with nothing to continue".into()),
            (None, _) => {
                assembled = Some(Message {
                    opcode: opcode_of(code)?,
                    payload,
                });
            }
            (Some(message), 0x0) => message.payload.extend_from_slice(&payload),
            (Some(_), _) => return Err("a new message began before the last one ended".into()),
        }
        if fin {
            return Ok(assembled);
        }
        if assembled
            .as_ref()
            .is_some_and(|m| m.payload.len() > MAX_PAYLOAD)
        {
            return Err("a websocket message ran past its limit".into());
        }
    }
}

/// One frame: its FIN bit, its opcode, and its payload. `None` at end of stream.
async fn read_frame(stream: &mut Reader) -> Result<Option<(bool, u8, Vec<u8>)>> {
    let Some(head) = take(stream, 2).await? else {
        return Ok(None);
    };
    let fin = head[0] & 0x80 != 0;
    if head[0] & 0x70 != 0 {
        // The three reserved bits are only meaningful under an extension, and we negotiate none.
        return Err("a websocket frame set a reserved bit".into());
    }
    let code = head[0] & 0x0F;
    let masked = head[1] & 0x80 != 0;
    if masked {
        return Err("a masked frame from a server (RFC 6455 §5.1 forbids it)".into());
    }
    let len = match head[1] & 0x7F {
        126 => {
            let ext = take(stream, 2).await?.ok_or("a frame ended mid-length")?;
            usize::from(u16::from_be_bytes([ext[0], ext[1]]))
        }
        127 => {
            let ext = take(stream, 8).await?.ok_or("a frame ended mid-length")?;
            let len = u64::from_be_bytes(ext.try_into().unwrap_or([0; 8]));
            usize::try_from(len).map_err(|_| "a frame longer than this machine can address")?
        }
        short => usize::from(short),
    };
    if len > MAX_PAYLOAD {
        return Err(format!("a websocket frame claims {len} bytes"));
    }
    let payload = match len {
        0 => Vec::new(),
        len => take(stream, len)
            .await?
            .ok_or("a frame ended mid-payload")?,
    };
    Ok(Some((fin, code, payload)))
}

/// Read exactly `n` bytes, or `None` if the stream ends first.
async fn take(stream: &mut Reader, n: usize) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let chunk = stream.read().await?;
        if chunk.is_empty() {
            return Ok(None);
        }
        let want = n - out.len();
        if chunk.len() > want {
            stream.unread(&chunk[want..]);
            out.extend_from_slice(&chunk[..want]);
        } else {
            out.extend_from_slice(&chunk);
        }
    }
    Ok(Some(out))
}

fn opcode_of(code: u8) -> Result<Opcode> {
    match code {
        0x1 => Ok(Opcode::Text),
        0x2 => Ok(Opcode::Binary),
        0x8 => Ok(Opcode::Close),
        0x9 => Ok(Opcode::Ping),
        0xA => Ok(Opcode::Pong),
        other => Err(format!("unknown websocket opcode {other:#x}")),
    }
}

// ---------------------------------------------------------------------------------------
// SHA-1
// ---------------------------------------------------------------------------------------

/// SHA-1, here and not from a crate.
///
/// RFC 6455 uses it as a **magic-string transform, not as a security primitive**: both sides
/// already know the key and the GUID, and the digest exists so a client can tell an endpoint that
/// understood the upgrade from one that merely answered `101`. Nothing about the connection's
/// security rests on it, so the usual reason to reach for a maintained implementation does not
/// apply — and the tree carries no SHA-1 anywhere else to share (`sha2` is the workspace's hash,
/// and it is a different algorithm, not a different version of this one).
#[expect(
    clippy::many_single_char_names,
    reason = "a, b, c, d, e and h are FIPS 180-4's own names for SHA-1's working variables; \
              renaming them would make this unreadable against the spec it has to match"
)]
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&(message.len() as u64 * 8).to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_its_own_test_vectors() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xA9, 0x99, 0x3E, 0x36, 0x47, 0x06, 0x81, 0x6A, 0xBA, 0x3E, 0x25, 0x71, 0x78, 0x50,
                0xC2, 0x6C, 0x9C, 0xD0, 0xD8, 0x9D
            ]
        );
        // Two blocks, so the padding path that spills into a second one is exercised.
        assert_eq!(
            sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            [
                0x84, 0x98, 0x3E, 0x44, 0x1C, 0x3B, 0xD2, 0x6E, 0xBA, 0xAE, 0x4A, 0xA1, 0xF9, 0x51,
                0x29, 0xE5, 0xE5, 0x46, 0x70, 0xF1
            ]
        );
        assert_eq!(sha1(b"")[..4], [0xDA, 0x39, 0xA3, 0xEE]);
    }

    #[test]
    fn the_accept_matches_the_rfcs_worked_example() {
        // RFC 6455 §1.3, verbatim.
        assert_eq!(
            accept_for("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn a_client_frame_is_masked_and_unmasks_back() {
        let frame = encode(Opcode::Text, b"hello");
        assert_eq!(frame[0], 0x81, "FIN + text");
        assert_eq!(frame[1], 0x85, "masked, 5 bytes");
        let mask = &frame[2..6];
        let unmasked: Vec<u8> = frame[6..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        assert_eq!(unmasked, b"hello");
    }

    #[test]
    fn a_long_frame_uses_the_length_the_rfc_says_it_should() {
        let medium = encode(Opcode::Binary, &[0u8; 200]);
        assert_eq!(medium[1] & 0x7F, 126, "16-bit length");
        assert_eq!(u16::from_be_bytes([medium[2], medium[3]]), 200);

        // On the heap: 70 KB is past what clippy will let a test put on the stack, and the point
        // of the case is the length field, not where the bytes live.
        let large = encode(Opcode::Binary, &vec![0u8; 70_000]);
        assert_eq!(large[1] & 0x7F, 127, "64-bit length");
        assert_eq!(
            u64::from_be_bytes(large[2..10].try_into().expect("8 bytes")),
            70_000
        );
    }
}
