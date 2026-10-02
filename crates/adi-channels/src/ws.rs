//! A minimal RFC 6455 **client**: the opening handshake, and the frames the router protocol
//! (`docs/channels.md` §2) actually uses — text one way, ping/pong/close both.
//!
//! Hand-rolled rather than pulling in a crate, the same call `adi-app/src/ws.rs` makes for the
//! panel's live channel: this client speaks exactly the slice of the protocol its one server
//! needs, and the whole of it fits in a file you can read. The two differ in exactly the one way
//! the RFC requires a client and a server to: a frame this side **sends** is always masked (§5.1);
//! a frame it **receives** is accepted whether or not the peer bothered to mask it, since nothing
//! this side talks to has a reason to.
//!
//! `ws://` only — no TLS of its own. `wss://` (needed before Telegram/Slack go live against
//! `hooks.withadi.dev`, ADI-MONO-124) is `tls.rs`'s job: it wraps the `TcpStream` before any of
//! this module ever sees it, so the handshake and framing below run unchanged over either.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::error::{Error, Result};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Cap one frame. The router never sends more than one small JSON object at a time (§2: "kept
/// flat and small on purpose"); anything larger is a protocol violation, not a bigger message.
const MAX_PAYLOAD: usize = 256 * 1024;

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// A complete message (fragments already reassembled) or control frame from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// A fresh `Sec-WebSocket-Key` — sixteen random bytes, base64-encoded (RFC 6455 §4.1).
#[must_use]
pub fn generate_key() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(random_bytes::<16>())
}

/// `N` random bytes off the OS source.
///
/// # Panics
/// Never in practice — only if the OS random source is unavailable, which is not a condition a
/// handshake can meaningfully continue past (same call `adi-mesh`'s own `random_nonce` makes).
fn random_bytes<const N: usize>() -> [u8; N] {
    use rand::TryRng as _;
    let mut bytes = [0u8; N];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the OS random source is unavailable");
    bytes
}

/// The value of `Sec-WebSocket-Accept` a correct server answers `key` with.
#[must_use]
pub fn accept_key(key: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(sha1(format!("{key}{GUID}").as_bytes()))
}

/// Write the upgrade request and read the response, validating the `101` and the accept key.
/// Returns whatever bytes were read past the header block — the start of the frame stream, to
/// seed a [`Reader`] with (a socket read never stops exactly at a header boundary).
///
/// # Errors
/// [`Error::Protocol`] on a socket error, a response that isn't a valid upgrade, or an accept key
/// that doesn't match — which also catches a proxy or load balancer answering instead of the
/// router, rather than silently treating that body as frames.
pub async fn connect<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    path_and_query: &str,
    bearer: &str,
) -> Result<Vec<u8>> {
    let key = generate_key();
    let request = format!(
        "GET {path_and_query} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Authorization: Bearer {bearer}\r\n\
         \r\n",
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let (head, leftover) = read_head(stream).await?;
    let head = String::from_utf8_lossy(&head);
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains(" 101 ") {
        return Err(Error::Protocol(format!(
            "router didn't upgrade the connection: {status_line}"
        )));
    }
    let accept = header_value(&head, "sec-websocket-accept")
        .ok_or_else(|| Error::Protocol("response carried no Sec-WebSocket-Accept".into()))?;
    if accept != accept_key(&key) {
        return Err(Error::Protocol(
            "Sec-WebSocket-Accept didn't match this handshake's key".into(),
        ));
    }
    Ok(leftover)
}

/// The value of header `name` (case-insensitive), from a raw `\r\n`-joined header block.
fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// Read until `\r\n\r\n`, returning `(header block without the trailing blank line, bytes read
/// past it)`.
async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(at) = find_double_crlf(&buf) {
            let head = buf[..at].to_vec();
            let rest = buf[at + 4..].to_vec();
            return Ok((head, rest));
        }
        if buf.len() > MAX_PAYLOAD {
            return Err(Error::Protocol("handshake response too large".into()));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(Error::Protocol(
                "the router closed the connection mid-handshake".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Reads frames off the router's side of the socket, reassembling fragmented messages.
#[derive(Debug)]
pub struct Reader {
    buf: Vec<u8>,
    fragment: Vec<u8>,
    fragment_op: u8,
}

impl Reader {
    /// A reader primed with whatever [`connect`] read past the handshake response.
    #[must_use]
    pub fn new(prefix: Vec<u8>) -> Self {
        Self {
            buf: prefix,
            fragment: Vec::new(),
            fragment_op: 0,
        }
    }

    /// The next message, or `None` once the peer closes.
    ///
    /// # Errors
    /// [`Error::Protocol`] on a socket error, an oversized payload, or an unknown opcode.
    pub async fn next<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<Option<Frame>> {
        loop {
            let Some(frame) = self.read_frame(r).await? else {
                return Ok(None);
            };
            let (fin, opcode, payload) = frame;
            match opcode {
                OP_PING => return Ok(Some(Frame::Ping(payload))),
                OP_PONG => return Ok(Some(Frame::Pong)),
                OP_CLOSE => return Ok(Some(Frame::Close)),
                OP_TEXT | OP_BINARY | OP_CONTINUATION => {}
                other => {
                    return Err(Error::Protocol(format!(
                        "unknown websocket opcode {other:#x}"
                    )));
                }
            }
            if opcode != OP_CONTINUATION {
                self.fragment.clear();
                self.fragment_op = opcode;
            }
            if self.fragment.len() + payload.len() > MAX_PAYLOAD {
                return Err(Error::Protocol("websocket message too large".into()));
            }
            self.fragment.extend_from_slice(&payload);
            if !fin {
                continue;
            }
            let message = std::mem::take(&mut self.fragment);
            if self.fragment_op != OP_TEXT {
                return Ok(Some(Frame::Close));
            }
            let text = String::from_utf8(message)
                .map_err(|e| Error::Protocol(format!("non-utf8 text frame: {e}")))?;
            return Ok(Some(Frame::Text(text)));
        }
    }

    /// One raw frame: `(fin, opcode, unmasked payload)`. Accepts a masked or unmasked frame — the
    /// RFC says a server must not mask, but unmasking an already-unmasked frame costs nothing and
    /// a strict refusal here would only make this client more fragile than the peers it talks to.
    async fn read_frame<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
    ) -> Result<Option<(bool, u8, Vec<u8>)>> {
        if !self.fill(r, 2).await? {
            return Ok(None);
        }
        let fin = self.buf[0] & 0x80 != 0;
        let opcode = self.buf[0] & 0x0F;
        let masked = self.buf[1] & 0x80 != 0;
        let short_len = usize::from(self.buf[1] & 0x7F);

        let (len, len_bytes) = match short_len {
            126 => {
                if !self.fill(r, 4).await? {
                    return Err(Error::Protocol("frame ended mid-length".into()));
                }
                let mut wide = [0u8; 2];
                wide.copy_from_slice(&self.buf[2..4]);
                (usize::from(u16::from_be_bytes(wide)), 2)
            }
            127 => {
                if !self.fill(r, 10).await? {
                    return Err(Error::Protocol("frame ended mid-length".into()));
                }
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&self.buf[2..10]);
                let len = u64::from_be_bytes(wide);
                (usize::try_from(len).unwrap_or(usize::MAX), 8)
            }
            n => (n, 0),
        };
        if len > MAX_PAYLOAD {
            return Err(Error::Protocol(format!("frame too large ({len} bytes)")));
        }

        let mask_bytes = if masked { 4 } else { 0 };
        let header = 2 + len_bytes + mask_bytes;
        if !self.fill(r, header + len).await? {
            return Err(Error::Protocol("frame ended mid-payload".into()));
        }
        let mut payload = self.buf[header..header + len].to_vec();
        if masked {
            let mut mask = [0u8; 4];
            mask.copy_from_slice(&self.buf[header - 4..header]);
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }
        self.buf.drain(..header + len);
        Ok(Some((fin, opcode, payload)))
    }

    async fn fill<R: AsyncRead + Unpin>(&mut self, r: &mut R, n: usize) -> Result<bool> {
        let mut chunk = [0u8; 4096];
        while self.buf.len() < n {
            let read = r.read(&mut chunk).await?;
            if read == 0 {
                return Ok(false);
            }
            self.buf.extend_from_slice(&chunk[..read]);
        }
        Ok(true)
    }
}

/// Send a text message, masked (§5.1: every frame a client sends must be).
///
/// # Errors
/// [`Error::Protocol`] if the write fails.
pub async fn write_text<W: AsyncWrite + Unpin>(w: &mut W, text: &str) -> Result<()> {
    write_frame(w, OP_TEXT, text.as_bytes()).await
}

/// Answer a ping with its own payload (§5.5.3).
///
/// # Errors
/// [`Error::Protocol`] if the write fails.
pub async fn write_pong<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> Result<()> {
    write_frame(w, OP_PONG, payload).await
}

/// Send a close frame (`1000 Normal Closure`).
///
/// # Errors
/// [`Error::Protocol`] if the write fails.
pub async fn write_close<W: AsyncWrite + Unpin>(w: &mut W) -> Result<()> {
    write_frame(w, OP_CLOSE, &1000u16.to_be_bytes()).await
}

/// Write one masked frame — the only form a client may send (§5.1).
async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, opcode: u8, payload: &[u8]) -> Result<()> {
    let mask = random_bytes::<4>();

    let mut head = Vec::with_capacity(14);
    head.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        #[allow(clippy::cast_possible_truncation)]
        head.push(0x80 | len as u8);
    } else if let Ok(len) = u16::try_from(len) {
        head.push(0x80 | 126);
        head.extend_from_slice(&len.to_be_bytes());
    } else {
        head.push(0x80 | 127);
        head.extend_from_slice(&(len as u64).to_be_bytes());
    }
    head.extend_from_slice(&mask);
    let mut masked_payload = payload.to_vec();
    for (i, byte) in masked_payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
    head.extend_from_slice(&masked_payload);
    w.write_all(&head).await?;
    w.flush().await?;
    Ok(())
}

/// SHA-1, needed by the handshake and nowhere else here — see `adi-app/src/ws.rs`'s copy of the
/// same forty lines for why this isn't a dependency.
#[allow(clippy::many_single_char_names)]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());

    for block in msg.chunks_exact(64) {
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
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (slot, add) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(add);
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
    use tokio::io::duplex;
    use tokio::net::{TcpListener, TcpStream};

    #[test]
    fn sha1_matches_the_standard_vectors() {
        fn hex(bytes: &[u8]) -> String {
            use std::fmt::Write as _;
            bytes.iter().fold(String::new(), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            })
        }
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn accept_key_matches_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// A full handshake over a real loopback socket: a tiny fake server answers the GET with a
    /// correct `101`, and [`connect`] accepts it.
    #[tokio::test]
    async fn connect_succeeds_against_a_correct_upgrade_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let key = header_value(&head, "sec-websocket-key").unwrap();
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                accept_key(&key)
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let leftover = connect(&mut client, "127.0.0.1", "/subscribe", "tok_1")
            .await
            .expect("handshake");
        assert!(leftover.is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_rejects_a_non_101_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await.unwrap();
            sock.write_all(b"HTTP/1.1 401 Unauthorized\r\n\r\n")
                .await
                .unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let err = connect(&mut client, "127.0.0.1", "/subscribe", "bad-token")
            .await
            .expect_err("should refuse");
        assert!(matches!(err, Error::Protocol(_)));
        server.await.unwrap();
    }

    /// Write then read a text frame over an in-memory duplex pipe — the framing round trip without
    /// a real socket.
    #[tokio::test]
    async fn a_written_text_frame_reads_back_whole() {
        let (mut a, mut b) = duplex(4096);
        write_text(&mut a, "hello router").await.unwrap();
        let mut reader = Reader::new(Vec::new());
        let frame = reader.next(&mut b).await.unwrap();
        assert_eq!(frame, Some(Frame::Text("hello router".into())));
    }

    #[tokio::test]
    async fn ping_and_close_are_recognized() {
        let (mut a, mut b) = duplex(4096);
        write_frame(&mut a, OP_PING, b"ping-payload").await.unwrap();
        let mut reader = Reader::new(Vec::new());
        assert_eq!(
            reader.next(&mut b).await.unwrap(),
            Some(Frame::Ping(b"ping-payload".to_vec()))
        );

        write_close(&mut a).await.unwrap();
        assert_eq!(reader.next(&mut b).await.unwrap(), Some(Frame::Close));
    }

    #[tokio::test]
    async fn an_unmasked_server_frame_still_reads() {
        // The RFC says the server must not mask; this proves the reader doesn't *require* it to.
        let (mut a, mut b) = duplex(4096);
        let payload = b"{\"type\":\"ping\"}";
        a.write_all(&[0x80 | OP_TEXT, u8::try_from(payload.len()).unwrap()])
            .await
            .unwrap();
        a.write_all(payload).await.unwrap();
        let mut reader = Reader::new(Vec::new());
        let frame = reader.next(&mut b).await.unwrap();
        assert_eq!(
            frame,
            Some(Frame::Text("{\"type\":\"ping\"}".to_string()))
        );
    }

    #[tokio::test]
    async fn a_closed_pipe_ends_the_stream() {
        let (a, mut b) = duplex(4096);
        drop(a);
        let mut reader = Reader::new(Vec::new());
        assert_eq!(reader.next(&mut b).await.unwrap(), None);
    }
}
