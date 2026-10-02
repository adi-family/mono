//! A blocking HTTP client over *this node's own* `/api/channels/*` endpoints
//! (`docs/channels.md` §7) — never the router directly (that is [`crate::router_api::RouterApi`]).
//! `adi-mono channels` is the only consumer: the panel calls the same endpoints straight from the
//! browser, so it has no need of a Rust client at all.
//!
//! The node's address is read from [`crate::node_port`] — the port `adi-app` wrote at its own
//! start-up — so this never guesses one, and every verb here fails the same understandable way
//! when nothing is running: there is no socket to subscribe or wait on without it.
//!
//! Errors here are plain `String`s, not [`crate::error::Error`] — that enum's `Router`/`Http`
//! variants are phrased for a call *to the router* ("couldn't reach the router"), and a failure
//! reaching this node's own loopback API needs its own words, not a borrowed and misleading pair.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::connection::{Allowlist, Target};

fn ensure_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

fn client() -> Result<reqwest::blocking::Client, String> {
    ensure_provider();
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

/// One connection, as the node's API answers it (`docs/channels.md` §5), flattened for the wire —
/// mirrors `adi_webapp_api::types::ChannelConnectionDto`, which this crate can't depend on (that
/// crate depends on this one).
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionView {
    pub id: String,
    pub provider: String,
    #[serde(default)]
    pub routing_key: String,
    pub target: Target,
    pub allowlist: Allowlist,
    pub paused: bool,
    pub linked: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ConnectionsView {
    connections: Vec<ConnectionView>,
}

/// What `POST /api/channels/connect` answers: the new, still-unlinked connection, and the
/// install/link URL to show the operator — empty for a provider the router has no way to build
/// one for from a bare code. Named apart from [`crate::connect::Connected`] (what `connect::connect`
/// itself returns, one layer closer to the store) since this is the HTTP response shape, not that
/// one — the two happen to carry the same two fields today, but nothing here should be read as a
/// promise they always will. `install_url_group` (ADI-MONO-125) is Telegram's "add to a group"
/// twin of `install_url`, empty for every other provider.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectResponse {
    pub connection: ConnectionView,
    #[serde(default)]
    pub install_url: String,
    #[serde(default)]
    pub install_url_group: String,
}

#[derive(Debug, Serialize)]
struct ConnectBody<'a> {
    provider: &'a str,
    target: &'a Target,
}

#[derive(Debug, Serialize)]
struct RefBody<'a> {
    id: &'a str,
}

#[derive(Debug, Serialize)]
struct RouteBody<'a> {
    id: &'a str,
    target: &'a Target,
}

#[derive(Debug, Serialize)]
struct PauseBody<'a> {
    id: &'a str,
    paused: bool,
}

#[derive(Debug, Serialize)]
struct AllowBody<'a> {
    id: &'a str,
    allowlist: &'a Allowlist,
}

/// This node's own small HTTP API, over whatever port [`crate::node_port`] names.
#[derive(Debug, Clone)]
pub struct NodeApi {
    base_url: String,
}

impl NodeApi {
    /// Find the running node and build a client for it.
    ///
    /// # Errors
    /// A message naming the reason, if [`crate::node_port::read`] fails or finds nothing — the
    /// one error every verb in this module can hit before it ever sends a request.
    pub fn local(config: &adi_config::Config) -> Result<Self, String> {
        let port = crate::node_port::read(config)
            .map_err(|e| format!("reading this node's own port: {e}"))?
            .ok_or_else(|| {
                "no node is running on this machine (nothing has written a port under \
                 ~/.adi/mono/channels/) — start adi-app first"
                    .to_string()
            })?;
        Ok(Self {
            base_url: format!("http://127.0.0.1:{port}"),
        })
    }

    /// `GET /api/channels` — every connection this node holds.
    ///
    /// # Errors
    /// A message naming the failure: the request couldn't be sent, or the node answered an error.
    pub fn list(&self) -> Result<Vec<ConnectionView>, String> {
        let view: ConnectionsView = get(&client()?, &format!("{}/api/channels", self.base_url))?;
        Ok(view.connections)
    }

    /// `GET /api/channels/<id>` — one connection, for `connect`'s own poll on `linked`.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn get(&self, id: &str) -> Result<ConnectionView, String> {
        get(&client()?, &format!("{}/api/channels/{id}", self.base_url))
    }

    /// `POST /api/channels/connect` — register this node for `provider` and mirror the fresh,
    /// unlinked connection the router created in the same call.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn connect(&self, provider: &str, target: &Target) -> Result<ConnectResponse, String> {
        post(
            &client()?,
            &format!("{}/api/channels/connect", self.base_url),
            &ConnectBody { provider, target },
        )
    }

    /// `POST /api/channels/route` — change a connection's target.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn route(&self, id: &str, target: &Target) -> Result<Vec<ConnectionView>, String> {
        let view: ConnectionsView = post(
            &client()?,
            &format!("{}/api/channels/route", self.base_url),
            &RouteBody { id, target },
        )?;
        Ok(view.connections)
    }

    /// `POST /api/channels/pause` — stop or resume delivery without disconnecting.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn pause(&self, id: &str, paused: bool) -> Result<Vec<ConnectionView>, String> {
        let view: ConnectionsView = post(
            &client()?,
            &format!("{}/api/channels/pause", self.base_url),
            &PauseBody { id, paused },
        )?;
        Ok(view.connections)
    }

    /// `POST /api/channels/allow` — change who may talk.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn allow(&self, id: &str, allowlist: &Allowlist) -> Result<Vec<ConnectionView>, String> {
        let view: ConnectionsView = post(
            &client()?,
            &format!("{}/api/channels/allow", self.base_url),
            &AllowBody { id, allowlist },
        )?;
        Ok(view.connections)
    }

    /// `POST /api/channels/disconnect` — tell the router to drop the connection, then drop it
    /// here too.
    ///
    /// # Errors
    /// As [`list`](Self::list).
    pub fn disconnect(&self, id: &str) -> Result<Vec<ConnectionView>, String> {
        let view: ConnectionsView = post(
            &client()?,
            &format!("{}/api/channels/disconnect", self.base_url),
            &RefBody { id },
        )?;
        Ok(view.connections)
    }
}

fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<T, String> {
    let response = client.get(url).send().map_err(|e| e.to_string())?;
    finish(response)
}

fn post<B: Serialize, T: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &B,
) -> Result<T, String> {
    let response = client.post(url).json(body).send().map_err(|e| e.to_string())?;
    finish(response)
}

/// Turn a response into `T`, or the node's own `{ "error": "…" }` message, or the status line
/// when the body isn't that shape.
fn finish<T: serde::de::DeserializeOwned>(response: reqwest::blocking::Response) -> Result<T, String> {
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{status} {text}"));
        return Err(message);
    }
    serde_json::from_str(&text).map_err(|e| format!("unexpected response from this node: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::TcpListener;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-node-api-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    /// A fake node answering exactly one request with a fixed status and JSON body.
    fn fake_node(status_and_body: impl Into<String>) -> String {
        let status_and_body = status_and_body.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" {
                    break;
                }
            }
            stream
                .try_clone()
                .unwrap()
                .write_all(status_and_body.as_bytes())
                .unwrap();
        });
        format!("{addr}")
    }

    #[test]
    fn local_reports_no_node_when_nothing_wrote_a_port() {
        let cfg = scratch("no-node");
        let err = NodeApi::local(&cfg).expect_err("nothing is running");
        assert!(err.contains("no node is running"), "{err}");
    }

    #[test]
    fn local_finds_the_port_a_running_node_wrote() {
        let cfg = scratch("found");
        crate::node_port::write(&cfg, 12345).unwrap();
        let api = NodeApi::local(&cfg).expect("a port was written");
        assert_eq!(api.base_url, "http://127.0.0.1:12345");
    }

    #[test]
    fn list_parses_the_connections_array() {
        let body = "{\"connections\":[{\"id\":\"c1\",\"provider\":\"telegram\",\"routing_key\":\"\",\
                     \"target\":{\"kind\":\"agent\",\"agent\":\"a\"},\
                     \"allowlist\":{\"kind\":\"owner_only\"},\"paused\":false,\"linked\":false,\
                     \"created_at\":1,\"updated_at\":1}]}";
        let addr = fake_node(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let api = NodeApi {
            base_url: format!("http://{addr}"),
        };
        let connections = api.list().expect("list");
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].id, "c1");
    }

    #[test]
    fn a_non_2xx_surfaces_the_nodes_own_error_message() {
        let body = "{\"error\":\"no such thing\"}";
        let addr = fake_node(format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let api = NodeApi {
            base_url: format!("http://{addr}"),
        };
        let err = api.get("ghost").expect_err("should refuse");
        assert_eq!(err, "no such thing");
    }
}
