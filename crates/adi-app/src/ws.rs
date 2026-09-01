//! A minimal RFC 6455 server: the opening handshake, and the frames the control panel's live
//! channel actually uses — text one way, ping/pong/close both.
//!
//! Hand-rolled for the same reason [`crate::http`] is: this server speaks exactly the slice of
//! the protocol its one client needs, and the whole of it fits in a file you can read. Only the
//! server half is here — frames we *send* are never masked, frames we *receive* always are
//! (§5.1), and a client that breaks that rule is dropped rather than accommodated.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The magic string the handshake concatenates onto the client's key (RFC 6455 §1.3).
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Cap one inbound frame. The client sends subscription lists — a few hundred bytes; anything
/// this large is a client gone wrong, and refusing it keeps a socket from growing memory.
const MAX_PAYLOAD: usize = 64 * 1024;

/// Opcodes, as they appear in the low nibble of a frame's first byte (§5.2).
const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// One complete message (fragments already reassembled) or control frame from the client.
#[derive(Debug)]
pub enum Frame {
    /// A text message. Binary messages are answered with a close — this protocol is JSON.
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// The value of the `Sec-WebSocket-Accept` header for a client's `Sec-WebSocket-Key`.
#[must_use]
pub fn accept_key(key: &str) -> String {
    use base64::Engine as _;
    let digest = sha1(format!("{key}{GUID}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Write the `101 Switching Protocols` response that ends the handshake.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_upgrade<W: AsyncWrite + Unpin>(w: &mut W, key: &str) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n",
        accept = accept_key(key),
    );
    w.write_all(head.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

/// Reads frames off a socket, reassembling fragmented messages.
///
/// Carries its own buffer because a socket read returns whatever arrived, which is as likely to
/// be half a frame as three of them.
#[derive(Debug)]
pub struct Reader {
    buf: Vec<u8>,
    /// The payload of a message still being fragmented, and the opcode it started with.
    fragment: Vec<u8>,
    fragment_op: u8,
}

impl Reader {
    /// A reader primed with bytes already taken off the socket — whatever the HTTP head read
    /// consumed past the request (see [`crate::http::Request::rest`]).
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
    /// Fails on a socket error, an oversized payload, or a client frame that isn't masked.
    pub async fn next<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> anyhow::Result<Option<Frame>> {
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
                other => anyhow::bail!("unknown websocket opcode {other:#x}"),
            }
            // A data frame: either the whole message, or one piece of one.
            if opcode != OP_CONTINUATION {
                self.fragment.clear();
                self.fragment_op = opcode;
            }
            anyhow::ensure!(
                self.fragment.len() + payload.len() <= MAX_PAYLOAD,
                "websocket message too large"
            );
            self.fragment.extend_from_slice(&payload);
            if !fin {
                continue;
            }
            let message = std::mem::take(&mut self.fragment);
            // Binary is not part of this protocol; treat it as the client hanging up.
            if self.fragment_op != OP_TEXT {
                return Ok(Some(Frame::Close));
            }
            return Ok(Some(Frame::Text(String::from_utf8(message)?)));
        }
    }

    /// One raw frame: `(fin, opcode, unmasked payload)`.
    async fn read_frame<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
    ) -> anyhow::Result<Option<(bool, u8, Vec<u8>)>> {
        if !self.fill(r, 2).await? {
            return Ok(None);
        }
        let fin = self.buf[0] & 0x80 != 0;
        let opcode = self.buf[0] & 0x0F;
        let masked = self.buf[1] & 0x80 != 0;
        let short_len = usize::from(self.buf[1] & 0x7F);

        // The length is 7 bits, or an escape into the 2- or 8-byte form that follows it (§5.2).
        let (len, len_bytes) = match short_len {
            126 => {
                anyhow::ensure!(self.fill(r, 4).await?, "websocket frame ended mid-length");
                let mut wide = [0u8; 2];
                wide.copy_from_slice(&self.buf[2..4]);
                (usize::from(u16::from_be_bytes(wide)), 2)
            }
            127 => {
                anyhow::ensure!(self.fill(r, 10).await?, "websocket frame ended mid-length");
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&self.buf[2..10]);
                let len = u64::from_be_bytes(wide);
                (usize::try_from(len).unwrap_or(usize::MAX), 8)
            }
            n => (n, 0),
        };
        anyhow::ensure!(
            len <= MAX_PAYLOAD,
            "websocket frame too large ({len} bytes)"
        );
        // §5.1: every frame from a client is masked. One that isn't is either a broken client or
        // something that isn't a browser at all.
        anyhow::ensure!(masked, "unmasked frame from a websocket client");

        let header = 2 + len_bytes + 4; // the two fixed bytes, any extended length, the mask
        anyhow::ensure!(
            self.fill(r, header + len).await?,
            "websocket frame ended mid-payload"
        );
        let mut mask = [0u8; 4];
        mask.copy_from_slice(&self.buf[header - 4..header]);
        let mut payload = self.buf[header..header + len].to_vec();
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
        self.buf.drain(..header + len);
        Ok(Some((fin, opcode, payload)))
    }

    /// Read until the buffer holds at least `n` bytes; `false` if the peer closed first.
    async fn fill<R: AsyncRead + Unpin>(&mut self, r: &mut R, n: usize) -> anyhow::Result<bool> {
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

/// Send a text message.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_text<W: AsyncWrite + Unpin>(w: &mut W, text: &str) -> anyhow::Result<()> {
    write_frame(w, OP_TEXT, text.as_bytes()).await
}

/// Send a ping, whose payload the peer echoes back.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_ping<W: AsyncWrite + Unpin>(w: &mut W) -> anyhow::Result<()> {
    write_frame(w, OP_PING, b"adi").await
}

/// Answer a ping with its own payload, as §5.5.3 requires.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_pong<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> anyhow::Result<()> {
    write_frame(w, OP_PONG, payload).await
}

/// Send a close frame (`1000 Normal Closure`).
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_close<W: AsyncWrite + Unpin>(w: &mut W) -> anyhow::Result<()> {
    write_frame(w, OP_CLOSE, &1000u16.to_be_bytes()).await
}

/// Write one unmasked frame — the only form a server may send (§5.1).
async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    opcode: u8,
    payload: &[u8],
) -> anyhow::Result<()> {
    let mut head = Vec::with_capacity(10);
    head.push(0x80 | opcode); // FIN: this server never fragments what it sends
    let len = payload.len();
    if len < 126 {
        // The cast is the point of the branch: this arm is only reached below 126.
        #[allow(clippy::cast_possible_truncation)]
        head.push(len as u8);
    } else if let Ok(len) = u16::try_from(len) {
        head.push(126);
        head.extend_from_slice(&len.to_be_bytes());
    } else {
        head.push(127);
        head.extend_from_slice(&(len as u64).to_be_bytes());
    }
    head.extend_from_slice(payload);
    // One write, so a frame can't interleave with another on the same socket.
    w.write_all(&head).await?;
    w.flush().await?;
    Ok(())
}

/// SHA-1 (FIPS 180-4), needed by the handshake and nowhere else in this codebase.
///
/// Kept here rather than pulled in as a dependency: the handshake's use of it is not a security
/// property — it exists so a cache or proxy can't accidentally complete the upgrade — and forty
/// lines of well-known arithmetic with the standard test vectors below is cheaper than a crate.
// `h`, `w` and the `a`–`e` working variables are the names FIPS 180-4 gives them; anything more
// descriptive would make this harder, not easier, to check against the specification.
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

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    #[test]
    fn sha1_matches_the_standard_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        // Two blocks, so the message-schedule loop is exercised more than once.
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn accept_key_matches_the_rfc_example() {
        // RFC 6455 §1.3's worked example.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// Frame a text payload the way a browser does: masked, with a mask we can predict.
    fn client_text(text: &str, mask: [u8; 4]) -> Vec<u8> {
        let mut frame = vec![0x80 | OP_TEXT, 0x80 | u8::try_from(text.len()).unwrap()];
        frame.extend_from_slice(&mask);
        for (i, byte) in text.bytes().enumerate() {
            frame.push(byte ^ mask[i % 4]);
        }
        frame
    }

    #[tokio::test]
    async fn reads_a_masked_text_frame() {
        let bytes = client_text("{\"sub\":[]}", [0x37, 0xFA, 0x21, 0x3D]);
        let mut reader = Reader::new(Vec::new());
        let frame = reader.next(&mut bytes.as_slice()).await.unwrap();
        assert!(matches!(frame, Some(Frame::Text(t)) if t == "{\"sub\":[]}"));
    }

    #[tokio::test]
    async fn reassembles_a_fragmented_message() {
        let mask = [1u8, 2, 3, 4];
        let mut bytes = Vec::new();
        // "ab" as a non-final text frame…
        bytes.extend_from_slice(&[OP_TEXT, 0x80 | 2]);
        bytes.extend_from_slice(&mask);
        bytes.extend_from_slice(&[b'a' ^ mask[0], b'b' ^ mask[1]]);
        // …then "c" as the final continuation.
        bytes.extend_from_slice(&[0x80 | OP_CONTINUATION, 0x80 | 1]);
        bytes.extend_from_slice(&mask);
        bytes.push(b'c' ^ mask[0]);

        let mut reader = Reader::new(Vec::new());
        let frame = reader.next(&mut bytes.as_slice()).await.unwrap();
        assert!(matches!(frame, Some(Frame::Text(t)) if t == "abc"));
    }

    #[tokio::test]
    async fn a_message_split_across_reads_still_arrives() {
        // The prefix carries the head; the "socket" carries the rest — the ordinary case of a
        // frame arriving in pieces.
        let bytes = client_text("hi", [9, 9, 9, 9]);
        let (head, tail) = bytes.split_at(3);
        let mut reader = Reader::new(head.to_vec());
        let frame = reader.next(&mut &tail[..]).await.unwrap();
        assert!(matches!(frame, Some(Frame::Text(t)) if t == "hi"));
    }

    #[tokio::test]
    async fn rejects_an_unmasked_client_frame() {
        let mut reader = Reader::new(vec![0x80 | OP_TEXT, 1, b'x']);
        assert!(reader.next(&mut [].as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn a_closed_socket_ends_the_stream() {
        let mut reader = Reader::new(Vec::new());
        assert!(reader.next(&mut [].as_slice()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn writes_an_unmasked_text_frame() {
        let mut out = Vec::new();
        write_text(&mut out, "hi").await.unwrap();
        assert_eq!(out, vec![0x80 | OP_TEXT, 2, b'h', b'i']);
    }
}
