//! The two HTTP calls a node makes to the router directly (everything else is the WebSocket):
//! `POST /register` to mint a node token and a fresh connection, and `POST /send` to post a reply
//! through one (`docs/channels.md` §1/§2). Blocking, like every other HTTP call this workspace
//! makes from a synchronous context (see `adi-agents`' harness loop) — both calls happen off the
//! tokio runtime (a webapp handler on the blocking pool, or the run-finished observer's own
//! `spawn_blocking`), never inside it.
//!
//! The request/response shapes here are read off `apps/channel-router`'s own
//! `src/router.ts`/`do.ts` (ADI-MONO-120), not guessed — this crate's tests still run only
//! against a fake router (below), per this task's own instructions, but the fake one is shaped to
//! match the real one's wire contract exactly.

use serde::{Deserialize, Serialize};

use crate::connection::{Allowlist, Target};
use crate::error::{Error, Result};

/// Install `ring` as the process-wide rustls provider, once — see `adi-agents`' harness loop for
/// why this is called at each client build rather than once at start-up (there is no start-up here
/// to rely on: a webapp handler runs on tokio's blocking pool, never `main`).
fn ensure_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

fn client() -> Result<reqwest::blocking::Client> {
    ensure_provider();
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| Error::Http(e.to_string()))
}

#[derive(Debug, Serialize)]
struct RegisterRequest<'a> {
    node: &'a str,
    provider: &'a str,
    target: &'a Target,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowlist: Option<&'a Allowlist>,
}

/// What `/register` hands back: a node token (good for this `(node, provider)` forever, or until
/// revoked), and a fresh connection — `do.ts`'s `register` creates both in the one call, on the
/// router's own clock, so this crate's connect flow never mints a connection id of its own; it
/// uses this one verbatim. `install_url` is `null` for a provider with no known way to build one
/// from a bare code (every provider but Telegram, today).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Registered {
    pub token: String,
    pub connection: String,
    pub link_code: String,
    #[serde(default)]
    pub install_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    connection: &'a str,
    text: &'a str,
}

/// A thin client over the router's own two node-facing HTTP routes.
#[derive(Debug, Clone)]
pub struct RouterApi {
    /// `http://host:port`, no trailing slash — `ws://`'s HTTP-scheme twin (see
    /// [`crate::client::parse_ws_url`] for how the two are kept in step).
    base_url: String,
}

impl RouterApi {
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self { base_url }
    }

    /// Mint a node token for `(node_id, provider)` (first call only — later ones return the same
    /// one) and a fresh connection pointed at `target`/`allowlist`. Bearer `admin_secret` is this
    /// node's own copy of `ROUTER_ADMIN_SECRET` (§8: acceptable because it gates minting, not
    /// ordinary traffic).
    ///
    /// # Errors
    /// [`Error::Http`] if the request can't be sent; [`Error::Router`] on a non-2xx status or a
    /// body that doesn't carry a `token`.
    pub fn register(
        &self,
        node_id: &str,
        provider: &str,
        target: &Target,
        allowlist: Option<&Allowlist>,
        admin_secret: &str,
    ) -> Result<Registered> {
        let response = client()?
            .post(format!("{}/register", self.base_url))
            .bearer_auth(admin_secret)
            .json(&RegisterRequest {
                node: node_id,
                provider,
                target,
                allowlist,
            })
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(Error::Router(format!("register failed ({status}): {body}")));
        }
        response
            .json()
            .map_err(|e| Error::Router(format!("register response wasn't the expected shape: {e}")))
    }

    /// Post `text` back through `connection`, over the router's own credential for whichever
    /// service the connection belongs to — the only path a reply ever takes (§1: "a node never
    /// calls Telegram/Slack directly").
    ///
    /// # Errors
    /// [`Error::Http`] if the request can't be sent; [`Error::Router`] on a non-2xx status.
    pub fn send(&self, node_token: &str, connection: &str, text: &str) -> Result<()> {
        let response = client()?
            .post(format!("{}/send", self.base_url))
            .bearer_auth(node_token)
            .json(&SendRequest { connection, text })
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(Error::Router(format!("send failed ({status}): {body}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::TcpListener;

    /// A fake router answering exactly the two routes this client calls, over a real loopback
    /// socket — per this task's instructions, never the real `apps/channel-router`.
    fn fake_router(status_and_body: impl Into<String>) -> String {
        let status_and_body = status_and_body.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();

            let mut stream = stream;
            stream.write_all(status_and_body.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    fn agent_target() -> Target {
        Target::Agent {
            agent: "adi-agent".into(),
        }
    }

    #[test]
    fn register_parses_the_token_and_connection_out_of_a_successful_response() {
        let body = "{\"token\":\"tok_123\",\"connection\":\"conn_abc\",\"link_code\":\"code_1\"}";
        let base = fake_router(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let registered = RouterApi::new(base)
            .register("node-1", "telegram", &agent_target(), None, "admin-secret")
            .expect("register");
        assert_eq!(registered.token, "tok_123");
        assert_eq!(registered.connection, "conn_abc");
        assert_eq!(registered.link_code, "code_1");
        assert_eq!(registered.install_url, None, "absent in this response");
    }

    #[test]
    fn register_carries_an_install_url_when_the_router_sends_one() {
        let body = "{\"token\":\"tok_123\",\"connection\":\"conn_abc\",\"link_code\":\"code_1\",\
                     \"install_url\":\"https://t.me/AdiBot?start=code_1\"}";
        let base = fake_router(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let registered = RouterApi::new(base)
            .register("node-1", "telegram", &agent_target(), None, "admin-secret")
            .expect("register");
        assert_eq!(
            registered.install_url,
            Some("https://t.me/AdiBot?start=code_1".to_string())
        );
    }

    #[test]
    fn register_surfaces_a_non_2xx_as_a_router_error() {
        let base = fake_router("HTTP/1.1 403 Forbidden\r\nContent-Length: 7\r\n\r\nrefused");
        let err = RouterApi::new(base)
            .register("node-1", "telegram", &agent_target(), None, "wrong-secret")
            .expect_err("should refuse");
        assert!(matches!(err, Error::Router(msg) if msg.contains("refused")));
    }

    #[test]
    fn send_succeeds_on_a_bare_200() {
        let base = fake_router("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        RouterApi::new(base)
            .send("node-token", "conn-1", "hello back")
            .expect("send");
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_is_tolerated() {
        let base = fake_router("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        RouterApi::new(format!("{base}/"))
            .send("node-token", "conn-1", "hi")
            .expect("send");
    }
}
