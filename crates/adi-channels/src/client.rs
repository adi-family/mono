//! The router client: holds the outbound WebSocket for one `(node, provider)` pair, reconnecting
//! with backoff, acking and deduplicating `event` frames, and handing each decoded
//! [`ChannelMessage`] off to the bus and to [`dispatch`] (`docs/channels.md` §1/§2).
//!
//! **One socket per provider**, per the spec: a node talking to both Telegram and Slack holds two
//! [`RouterClient`]s, so losing one never touches the other.
//!
//! **Open item on the wire detail.** §1's route table spells the node's upgrade call
//! `POST /subscribe`; RFC 6455 §4.1 requires the opening handshake's method to be `GET` ("the
//! method of the request MUST be GET"), which is what every real client (and this one) sends —
//! the table's "POST" most likely means "an action this node takes," read loosely, the way the
//! other rows in it do. Reconcile with whatever `apps/channel-router` actually expects on the
//! method line before this talks to the real router; this crate's own tests (and the fake router
//! they run against) only exercise `GET`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use adi_agents::Agents;
use adi_events::Events;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::connection::Connections;
use crate::error::{Error, Result};
use crate::protocol::{NodeFrame, RouterFrame};
use crate::ws;

/// How many delivered event ids are remembered for dedup across a reconnect (§2: "a node that
/// disconnects mid-delivery gets the same event again on reconnect"). Comfortably past the
/// router's own offline queue cap so a redelivered burst can't push a live id out before it's
/// used.
const DEDUP_CAPACITY: usize = 512;

/// The backoff schedule: 1s, 2s, 4s, … capped at 60s (§2), reset to the start on every successful
/// connect.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// The last [`DEDUP_CAPACITY`] delivered event ids, so a redelivery (§2) is recognized rather than
/// dispatched twice. Pulled out of [`RouterClient::connect_and_serve`] so it's unit-testable on
/// its own, with no socket involved.
#[derive(Debug, Default)]
struct DedupCache {
    order: VecDeque<String>,
    seen: std::collections::HashSet<String>,
}

impl DedupCache {
    /// Record `id`, returning whether this is the first time it's been seen.
    fn mark(&mut self, id: &str) -> bool {
        if !self.seen.insert(id.to_string()) {
            return false;
        }
        self.order.push_back(id.to_string());
        if self.order.len() > DEDUP_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        true
    }
}

/// `ws://host:port` (or `http://`/`https://`, read the same way) split into what [`TcpStream`]
/// needs. `wss://`/`https://` are parsed but not yet connected with TLS — see the crate-level note
/// on `ws.rs`.
pub(crate) fn parse_ws_url(url: &str) -> Result<(String, u16)> {
    let without_scheme = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| Error::Protocol(format!("{url} has no port")))?;
    let port = port
        .parse()
        .map_err(|_| Error::Protocol(format!("{url} has an invalid port")))?;
    Ok((host.to_string(), port))
}

/// Everything one [`RouterClient`] needs, gathered so `adi-app` builds one per `(node, provider)`
/// it has a live token for.
#[derive(Debug, Clone)]
pub struct RouterClient {
    /// `ws://host:port` (or `http`/`https`, read the same) — the router's base address.
    pub router_url: String,
    pub provider: String,
    /// This node's bearer for `/subscribe`, minted by `RouterApi::register`.
    pub node_token: String,
    pub connections: Connections,
    pub agents: Agents,
    pub events: Events,
    /// Set `true` for as long as the socket is actually open, `false` the instant it drops or
    /// before the first connect — what `adi-app`'s `channels::Live::is_connected` (and so the
    /// panel's router-connection pill) reads.
    pub connected: Arc<AtomicBool>,
}

impl RouterClient {
    /// Run the connect/read/reconnect loop until `shutdown` fires. Never returns before that —
    /// call it inside `tokio::spawn`.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut backoff = BACKOFF_START;
        loop {
            let outcome = self.connect_and_serve(&mut shutdown).await;
            // Every path out of `connect_and_serve` means the socket is no longer up, whether it
            // ever got there or not — set unconditionally rather than only on the error arms.
            self.connected.store(false, Ordering::Relaxed);
            match outcome {
                Ok(true) => break, // told to shut down
                Ok(false) => {
                    // A clean close or a dropped socket: reconnect from the start of the backoff,
                    // since the connection was actually up for a while.
                    backoff = BACKOFF_START;
                }
                Err(e) => {
                    warn!(provider = %self.provider, error = %e, "router connection failed");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(backoff) => {}
                _ = shutdown.changed() => break,
            }
            if *shutdown.borrow() {
                break;
            }
            backoff = (backoff * 2).min(BACKOFF_CAP);
        }
    }

    /// One connection's worth of life: handshake, then read frames until the peer closes, the
    /// socket errors, or shutdown fires. `Ok(true)` means shutdown was the reason; `Ok(false)`
    /// means the peer side ended it.
    async fn connect_and_serve(&self, shutdown: &mut watch::Receiver<bool>) -> Result<bool> {
        let (host, port) = parse_ws_url(&self.router_url)?;
        let mut stream = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| Error::Protocol(format!("couldn't reach {host}:{port}: {e}")))?;
        let leftover = ws::connect(&mut stream, &host, "/subscribe", &self.node_token).await?;
        self.connected.store(true, Ordering::Relaxed);
        info!(provider = %self.provider, %host, port, "subscribed to the router");

        let mut reader = ws::Reader::new(leftover);
        let mut dedup = DedupCache::default();

        loop {
            let next = tokio::select! {
                frame = reader.next(&mut stream) => frame,
                _ = shutdown.changed() => {
                    let _ = ws::write_close(&mut stream).await;
                    return Ok(true);
                }
            };
            if *shutdown.borrow() {
                let _ = ws::write_close(&mut stream).await;
                return Ok(true);
            }
            let Some(frame) = next? else {
                return Ok(false); // peer closed the TCP connection
            };
            let ws::Frame::Text(text) = frame else {
                match frame {
                    ws::Frame::Close => return Ok(false),
                    // A transport-level WS ping (distinct from the §2 *application* `{"type":
                    // "ping"}` frame RouterFrame::Ping answers) — RFC 6455 §5.5.2 wants a pong
                    // back with the same payload, which is also what keeps an edge proxy's own
                    // idle timeout from closing the socket out from under the frames above it.
                    ws::Frame::Ping(payload) => {
                        if let Err(e) = ws::write_pong(&mut stream, &payload).await {
                            warn!(provider = %self.provider, error = %e, "couldn't answer a transport-level ping");
                        }
                    }
                    ws::Frame::Pong => {}
                    ws::Frame::Text(_) => unreachable!("matched above"),
                }
                continue;
            };
            let parsed: RouterFrame = match serde_json::from_str(&text) {
                Ok(frame) => frame,
                Err(e) => {
                    warn!(provider = %self.provider, error = %e, frame = %text, "unreadable frame from the router");
                    continue;
                }
            };
            self.handle_frame(parsed, &mut stream, &mut dedup).await;
        }
    }

    async fn handle_frame(&self, frame: RouterFrame, stream: &mut TcpStream, dedup: &mut DedupCache) {
        match frame {
            RouterFrame::Ping => {
                if let Err(e) = write_frame(stream, &NodeFrame::Pong).await {
                    warn!(provider = %self.provider, error = %e, "couldn't answer a ping");
                }
            }
            RouterFrame::Linked {
                connection,
                routing_key,
                ..
            } => {
                // No sender rides on this frame (see the dispatch module's note on the same gap);
                // the owner is claimed from whoever messages first.
                if let Err(e) = self.connections.mark_linked(&connection, &routing_key, "") {
                    warn!(provider = %self.provider, %connection, error = %e, "couldn't record a link");
                }
            }
            RouterFrame::Event { id, message } => {
                // Ack unconditionally — a repeat the router redelivered is exactly what dedup is
                // for, and acking it is what lets the router's queue drop it either way.
                if let Err(e) = write_frame(stream, &NodeFrame::Ack { id: id.clone() }).await {
                    warn!(provider = %self.provider, error = %e, "couldn't ack an event");
                }
                if !dedup.mark(&id) {
                    debug!(provider = %self.provider, %id, "duplicate delivery, already handled");
                    return;
                }

                if let Err(e) = self.events.emit(
                    crate::events::CHANNEL_MESSAGE,
                    serde_json::to_string(&message).unwrap_or_default(),
                ) {
                    warn!(provider = %self.provider, error = %e, "couldn't publish adi.channels.message");
                }

                let connections = self.connections.clone();
                let agents = self.agents.clone();
                let provider = self.provider.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::dispatch::handle(&connections, &agents, &message)
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!(%provider, error = %e, "dispatching a channel message failed"),
                    Err(e) => warn!(%provider, error = %e, "dispatch task panicked"),
                }
            }
        }
    }
}

async fn write_frame(stream: &mut TcpStream, frame: &NodeFrame) -> Result<()> {
    let text = serde_json::to_string(frame).unwrap_or_default();
    ws::write_text(stream, &text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{Connections, Target};
    use crate::message::{ChannelMessage, Sender, VERSION};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    #[test]
    fn parse_ws_url_splits_host_and_port_regardless_of_scheme() {
        assert_eq!(
            parse_ws_url("ws://127.0.0.1:8787").unwrap(),
            ("127.0.0.1".to_string(), 8787)
        );
        assert_eq!(
            parse_ws_url("http://localhost:8787/subscribe").unwrap(),
            ("localhost".to_string(), 8787)
        );
    }

    #[test]
    fn parse_ws_url_rejects_a_url_with_no_port() {
        assert!(parse_ws_url("ws://127.0.0.1").is_err());
    }

    #[test]
    fn dedup_cache_recognizes_a_redelivered_id_but_not_a_fresh_one() {
        let mut dedup = DedupCache::default();
        assert!(dedup.mark("evt_1"), "first sighting");
        assert!(!dedup.mark("evt_1"), "redelivered — already handled");
        assert!(dedup.mark("evt_2"), "a different id is still fresh");
    }

    #[test]
    fn dedup_cache_evicts_the_oldest_past_capacity() {
        let mut dedup = DedupCache::default();
        for n in 0..=DEDUP_CAPACITY {
            assert!(dedup.mark(&format!("evt_{n}")));
        }
        // `evt_0` fell off the front, so it reads as fresh again.
        assert!(dedup.mark("evt_0"));
        // Whatever's still inside the window does not.
        assert!(!dedup.mark(&format!("evt_{DEDUP_CAPACITY}")));
    }

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-client-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    /// An in-process fake router (per this task's own instructions: never the real
    /// `apps/channel-router`). Performs the server half of the handshake by hand, sends one
    /// `event` frame and one `ping`, and hands back everything it read off the socket afterwards
    /// so the test can assert on the ack and the pong.
    ///
    /// Returns the URL and the still-running server task **unawaited** — its first step is
    /// `accept().await`, which only resolves once some client actually dials in, so awaiting it
    /// here (before that client exists) would deadlock the whole test.
    async fn spawn_fake_router(message: ChannelMessage) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let key = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key:").then(|| l.split_once(':').unwrap().1.trim().to_string()))
                .expect("client sent a key");
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                ws::accept_key(&key)
            );
            sock.write_all(response.as_bytes()).await.unwrap();

            let event = serde_json::to_string(&RouterFrame::Event {
                id: "evt_1".into(),
                message: Box::new(message),
            })
            .unwrap();
            ws::write_text(&mut sock, &event).await.unwrap();
            // `ws::write_text` always masks (it's written for the client role) — a real router
            // would send these unmasked instead, which `ws::Reader` accepts just as happily (see
            // `an_unmasked_server_frame_still_reads` in ws.rs); reused here only for convenience.
            let ping = serde_json::to_string(&RouterFrame::Ping).unwrap();
            ws::write_text(&mut sock, &ping).await.unwrap();

            // Read back whatever the client sends until the close frame it answers shutdown
            // with (§2: the client never closes on its own otherwise) — the ack and the pong,
            // both masked text frames, arrive ahead of it.
            let mut collected = Vec::new();
            loop {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                collected.extend_from_slice(&buf[..n]);
                // The ack (`{"type":"ack","id":"evt_1"}`, masked: 33 bytes) and the pong
                // (`{"type":"pong"}`, masked: 21 bytes) together are 54 bytes — stop once both
                // are in, well before the close frame this test never waits for.
                if collected.len() >= 54 {
                    break;
                }
            }
            collected
        });
        (format!("ws://{addr}"), server)
    }

    fn message_for(connection: &str) -> ChannelMessage {
        ChannelMessage {
            v: VERSION,
            id: "evt_1".into(),
            provider: "telegram".into(),
            connection: connection.to_string(),
            thread: "chat-1".into(),
            sender: Sender {
                id: "u1".into(),
                name: "Igor".into(),
            },
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
            raw_kind: "message".into(),
            received_at: 1,
        }
    }

    /// The whole receive path end to end against an in-process fake router: the client completes
    /// the handshake, acks the `event` frame it's handed, answers the `ping` with a `pong`, and
    /// does not crash even though the connection's target agent doesn't exist (dispatch's own
    /// unit tests already cover what happens for a real one).
    #[tokio::test]
    async fn the_client_acks_events_and_answers_pings_from_a_fake_router() {
        let cfg = scratch("roundtrip");
        let connections = Connections::with_config(cfg.clone());
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "ghost".into(),
                },
            )
            .unwrap();

        let (router_url, server) = spawn_fake_router(message_for(&created.id)).await;

        let connected = Arc::new(AtomicBool::new(false));
        let client = RouterClient {
            router_url,
            provider: "telegram".into(),
            node_token: "tok_1".into(),
            connections,
            agents: Agents::with_config(cfg.clone()),
            events: Events::with_config(cfg),
            connected: connected.clone(),
        };
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(client.run(shutdown_rx));
        // The fake router's own task ends once it's read the ack and the pong back.
        let raw = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the fake router timed out waiting for the client's frames")
            .unwrap();
        assert!(
            connected.load(Ordering::Relaxed),
            "the flag flips true the moment the handshake completes"
        );
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(
            !connected.load(Ordering::Relaxed),
            "and false again once the client has shut down"
        );

        // The client's frames are masked (§5.1); `ws::Reader` unmasks either way (see its own
        // doc), so feeding the raw bytes straight back through it is the actual decode, not a
        // byte-presence guess.
        let mut reader = ws::Reader::new(Vec::new());
        let mut remaining = raw.as_slice();
        let first: NodeFrame =
            serde_json::from_str(match reader.next(&mut remaining).await.unwrap().unwrap() {
                ws::Frame::Text(t) => t,
                other => panic!("expected a text frame, got {other:?}"),
            }
            .as_str())
            .unwrap();
        assert_eq!(first, NodeFrame::Ack { id: "evt_1".into() });

        let second: NodeFrame =
            serde_json::from_str(match reader.next(&mut remaining).await.unwrap().unwrap() {
                ws::Frame::Text(t) => t,
                other => panic!("expected a text frame, got {other:?}"),
            }
            .as_str())
            .unwrap();
        assert_eq!(second, NodeFrame::Pong);
    }
}
