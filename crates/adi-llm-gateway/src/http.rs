//! The client leg: reading one request off a socket, and the two small writes that answer it
//! without an upstream (an error, and the index).
//!
//! Hand-rolled rather than a framework, for the same reason `adi-app` and `adi-hive` are: the
//! surface is one request per connection and the bytes are the point. What this reader must do
//! that theirs need not is keep the header list *in order and unfolded*, because every one of them
//! is about to be replayed at a provider that may well care — and accept a chunked request body,
//! because an SDK streaming a large prompt is entitled to send one.

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Cap the head so a client that never sends the blank line cannot grow memory.
const MAX_HEAD: usize = 64 * 1024;

/// Cap a buffered request body. A prompt carrying images and files is large but bounded; anything
/// past this is refused rather than truncated, so the provider never sees half a document.
const MAX_BODY: usize = 64 << 20;

/// So a silent client cannot hold a connection open forever. This bounds the *client* leg only —
/// how long a provider may take to answer is [`crate::proxy`]'s business, and much longer.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// One request as it arrived: method, target, headers in the order and spelling they were sent,
/// and the fully buffered body.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// One header's value, matched case-insensitively as HTTP/1.1 field names are.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A socket being read one request at a time, holding whatever it read past the last one.
#[derive(Debug)]
pub struct Reader<'a> {
    stream: &'a mut TcpStream,
    buf: Vec<u8>,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(stream: &'a mut TcpStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    /// Read one request; `Ok(None)` if the peer closed without starting one.
    ///
    /// # Errors
    /// Fails on a socket error or timeout, an oversized head or body, a body that ends early, or a
    /// malformed chunked encoding.
    pub async fn request(&mut self) -> anyhow::Result<Option<Request>> {
        let Some(head) = self.head().await? else {
            return Ok(None);
        };
        let (method, target, headers) = parse_head(&head);

        let chunked = header(&headers, "transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let body = if chunked {
            self.chunked_body().await?
        } else {
            let length = header(&headers, "content-length")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            anyhow::ensure!(
                length <= MAX_BODY,
                "request body of {length} bytes is over the {MAX_BODY}-byte limit"
            );
            self.take(length).await?
        };

        Ok(Some(Request {
            method,
            target,
            headers,
            body,
        }))
    }

    /// Read up to and including the blank line, returning the head without it.
    async fn head(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = self.buf.drain(..end + 4).collect::<Vec<_>>();
                return Ok(Some(head[..end].to_vec()));
            }
            anyhow::ensure!(self.buf.len() <= MAX_HEAD, "request head too large");
            if self.fill().await? == 0 {
                anyhow::ensure!(self.buf.is_empty(), "connection closed mid-head");
                return Ok(None);
            }
        }
    }

    /// Exactly `n` more bytes.
    async fn take(&mut self, n: usize) -> anyhow::Result<Vec<u8>> {
        while self.buf.len() < n {
            anyhow::ensure!(self.fill().await? > 0, "connection closed mid-body");
        }
        Ok(self.buf.drain(..n).collect())
    }

    /// One CRLF-terminated line, without the terminator.
    async fn line(&mut self) -> anyhow::Result<String> {
        loop {
            if let Some(end) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = self.buf.drain(..end + 2).collect::<Vec<_>>();
                return Ok(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            anyhow::ensure!(self.buf.len() <= MAX_HEAD, "chunk header too large");
            anyhow::ensure!(self.fill().await? > 0, "connection closed mid-chunk");
        }
    }

    /// RFC 9112 §7.1 chunked decoding, flattened into the same buffer a `Content-Length` body
    /// lands in — the upstream leg re-frames it either way, so the encoding is not carried across.
    async fn chunked_body(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let header = self.line().await?;
            // A chunk size may carry extensions after a `;`, which nothing here needs.
            let size_text = header.split(';').next().unwrap_or_default().trim();
            let size = usize::from_str_radix(size_text, 16)
                .map_err(|_| anyhow::anyhow!("invalid chunk size {size_text:?}"))?;
            if size == 0 {
                // Trailers, then the blank line that ends them.
                while !self.line().await?.is_empty() {}
                return Ok(body);
            }
            anyhow::ensure!(
                body.len() + size <= MAX_BODY,
                "chunked request body is over the {MAX_BODY}-byte limit"
            );
            body.extend_from_slice(&self.take(size).await?);
            anyhow::ensure!(self.take(2).await? == b"\r\n", "chunk not CRLF-terminated");
        }
    }

    /// Read more bytes into the buffer; `0` means the peer closed.
    async fn fill(&mut self) -> anyhow::Result<usize> {
        let mut chunk = [0u8; 8192];
        let n = tokio::time::timeout(READ_TIMEOUT, self.stream.read(&mut chunk)).await??;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }
}

/// Split the raw head into method, target and headers, keeping the field names as sent.
fn parse_head(head: &[u8]) -> (String, String, Vec<(String, String)>) {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let target = request_line.next().unwrap_or("/").to_string();

    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(field, value)| (field.trim().to_string(), value.trim().to_string()))
        .collect();

    (method, target, headers)
}

/// One header's value out of a list, case-insensitively.
#[must_use]
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Answer with a JSON document and close — the gateway's own replies, never a provider's.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_json(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Serve one request from a socket fed `raw`, so the reader is exercised over a real stream.
    async fn read_one(raw: &'static [u8]) -> Request {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut client = TcpStream::connect(addr).await.unwrap();
            client.write_all(raw).await.unwrap();
            client.flush().await.unwrap();
        });
        let (mut server, _) = listener.accept().await.unwrap();
        Reader::new(&mut server).request().await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn reads_a_content_length_body_and_keeps_header_order() {
        let req = read_one(
            b"POST /anthropic/v1/messages HTTP/1.1\r\nHost: llm.adi\r\nX-Api-Key: k\r\n\
              Content-Length: 7\r\n\r\n{\"a\":1}",
        )
        .await;
        assert_eq!(req.method, "POST");
        assert_eq!(req.target, "/anthropic/v1/messages");
        assert_eq!(req.body, b"{\"a\":1}");
        assert_eq!(req.headers[0].0, "Host");
        assert_eq!(req.header("x-api-key"), Some("k"));
    }

    #[tokio::test]
    async fn decodes_a_chunked_body_into_the_same_buffer() {
        let req = read_one(
            b"POST /openai/v1/chat HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
              4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n",
        )
        .await;
        assert_eq!(req.body, b"{\"a\":1}");
    }
}
