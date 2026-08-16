//! The live channel behind `GET /api/ws`: the control panel says what it is looking at, and the
//! server sends it back whenever the answer changes.
//!
//! What it replaces: every open tab re-asked the same questions on a timer — a chat's transcript
//! and run list once a second, the agent/session/dashboard rails every four — so the request rate
//! grew with the number of tabs, and nearly every answer was byte-identical to the one before it.
//!
//! A **topic** here is not a new concept to maintain alongside the API: it *is* a request, the same
//! `(method, path, body)` the browser used to poll, and it is answered by the very same routing.
//! The difference is who repeats it and how often the answer travels:
//!
//! * a topic is computed **once for everyone** watching it, on its own interval, not once per
//!   watcher per tick;
//! * the result is sent only when it **differs from the last one sent** — a settled page costs
//!   nothing on the wire;
//! * a topic with no watchers is not computed at all, so closing a page really does stop its work.
//!
//! Only reads are watchable ([`watchable`]) — a subscription can never reach a mutation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::debug;

use crate::{App, http, ws};

/// How often the hub looks for topics that have come due. The floor on how promptly a change is
/// noticed, and deliberately shorter than the fastest topic interval.
const TICK: Duration = Duration::from_millis(250);

/// Live views — a transcript being written, a terminal pane, a log still being appended to.
const FAST: Duration = Duration::from_secs(1);

/// Everything else: lists that change when a person changes them.
const SLOW: Duration = Duration::from_secs(3);

/// `/api/health` alone. Its uptime counts seconds, so unlike every other topic it is *never* the
/// same answer twice — polled at [`SLOW`] it would be the one thing still talking to a page
/// nobody is touching. Nothing is lost by asking rarely: the socket being open is what says the
/// backend is up, and the uptime it reports is displayed in minutes and hours.
const IDLE: Duration = Duration::from_secs(15);

/// How often the server pings an idle socket, so a connection dropped by something in the middle
/// (a proxy, a sleeping laptop) is noticed rather than silently kept.
const PING_EVERY: Duration = Duration::from_secs(30);

/// Cap what one connection may watch. The busiest page subscribes to a dozen or so topics.
const MAX_WATCHES: usize = 64;

/// How many undelivered messages a connection may bank up before it is dropped. A client not
/// reading its socket is not a client to keep buffering for; the browser reconnects.
const QUEUE: usize = 256;

/// How long a read may be outstanding before the topic is offered again.
///
/// A topic is claimed while it is being computed, so a slow read is never started twice over. If
/// the task doing it ever went away without publishing, that claim would be permanent and the
/// topic would go quiet for the life of the process — a page that silently stops updating being
/// far worse than an occasional duplicated read.
const STALE: Duration = Duration::from_secs(60);

/// Which reads the live channel may watch, and how often each is recomputed.
///
/// The allowlist is the security boundary of this whole module: a subscription is dispatched with
/// the same routing an HTTP request uses, so anything reachable here would be callable over the
/// socket. Every entry is a read, and reads are all that belongs on a channel whose whole purpose
/// is to repeat itself.
///
/// Matched on the route alone, as the HTTP side matches it: a read that takes a query parameter
/// (`/api/agents/runs/all?limit=100`) is the same read either way. The *topic* is still keyed by
/// the full path, so two pages watching different pages of it get an answer each.
fn watchable(method: &str, path: &str) -> Option<Duration> {
    let path = path.split('?').next().unwrap_or(path);
    match method {
        "GET" => match path {
            "/api/health" => Some(IDLE),
            "/api/agents"
            | "/api/agents/runs/all"
            | "/api/dashboards"
            | "/api/db"
            | "/api/fleet"
            | "/api/hive"
            | "/api/mesh"
            | "/api/meta"
            | "/api/ports"
            | "/api/ports/used"
            | "/api/projects"
            | "/api/secrets"
            | "/api/tasks"
            | "/api/tools"
            | "/api/triggers" => Some(SLOW),
            // One project's detail page. The exact paths above are matched first, so this is only
            // ever an id.
            p if p.starts_with("/api/projects/") => Some(SLOW),
            _ => None,
        },
        // Reads that are POSTs because they carry a subject — an agent name, a run id — in a body.
        "POST" => match path {
            "/api/agents/peek"
            | "/api/agents/goals"
            | "/api/agents/run/peek"
            | "/api/agents/runs"
            | "/api/projects/hook/log"
            | "/api/projects/workspaces/terminal/peek"
            | "/api/triggers/log" => Some(FAST),
            "/api/projects/workspaces" => Some(SLOW),
            _ => None,
        },
        _ => None,
    }
}

/// One read a connection is watching: exactly the request it would otherwise have polled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watch {
    method: String,
    path: String,
    body: String,
}

impl Watch {
    /// The identity of this read — and, being `method path\nbody`, the same string the HTTP side
    /// collapses concurrent identical reads under.
    fn key(&self) -> String {
        format!("{} {}\n{}", self.method, self.path, self.body)
    }
}

/// A read being watched: who wants it, when it is next due, and what was last sent for it.
#[derive(Debug)]
struct Topic {
    watch: Watch,
    every: Duration,
    watchers: HashSet<u64>,
    /// The last message published, which is both the change baseline and the snapshot a newly
    /// subscribing connection gets straight away.
    last: Option<Arc<String>>,
    due: Instant,
    /// When the outstanding read was started, if there is one — see [`STALE`].
    computing: Option<Instant>,
}

/// Every connection and every topic, behind one lock.
#[derive(Debug, Default)]
struct Inner {
    topics: HashMap<String, Topic>,
    conns: HashMap<u64, mpsc::Sender<Arc<String>>>,
}

/// The live channel's shared state.
#[derive(Debug, Default)]
pub struct Hub {
    inner: std::sync::Mutex<Inner>,
    next_conn: AtomicU64,
}

impl Hub {
    /// A previous panic while holding this lock says nothing about the maps, so a poisoned lock
    /// is taken anyway rather than failing every later connection.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a connection, returning its id and the receiver its messages arrive on.
    fn attach(&self) -> (u64, mpsc::Receiver<Arc<String>>) {
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(QUEUE);
        self.lock().conns.insert(id, tx);
        (id, rx)
    }

    /// Drop a connection, and with it any topic it was the last watcher of — which is what makes
    /// a closed tab stop costing anything.
    fn detach(&self, id: u64) {
        let mut inner = self.lock();
        inner.conns.remove(&id);
        inner.topics.retain(|_, topic| {
            topic.watchers.remove(&id);
            !topic.watchers.is_empty()
        });
    }

    /// Replace what one connection watches.
    ///
    /// Returns `(snapshots, fresh)`: messages to send immediately because the topic already has an
    /// answer, and topics nobody has computed yet — which the caller starts at once, so a page
    /// paints on subscribe rather than on the next tick.
    fn subscribe(&self, id: u64, wanted: Vec<Watch>) -> (Vec<Arc<String>>, Vec<(String, Watch)>) {
        let now = Instant::now();
        let mut inner = self.lock();
        let mut snapshots = Vec::new();
        let mut fresh = Vec::new();
        let mut keys = HashSet::new();

        for watch in wanted.into_iter().take(MAX_WATCHES) {
            let Some(every) = watchable(&watch.method, &watch.path) else {
                debug!(method = %watch.method, path = %watch.path, "refusing to watch");
                continue;
            };
            let key = watch.key();
            keys.insert(key.clone());
            let topic = inner.topics.entry(key.clone()).or_insert_with(|| Topic {
                watch: watch.clone(),
                every,
                watchers: HashSet::new(),
                last: None,
                due: now,
                computing: None,
            });
            // Already watching it: nothing to send, the topic's own schedule carries on.
            if !topic.watchers.insert(id) {
                continue;
            }
            match &topic.last {
                Some(message) => snapshots.push(Arc::clone(message)),
                None if topic.computing.is_none() => {
                    topic.computing = Some(now);
                    fresh.push((key, watch));
                }
                // Someone else's subscribe is already computing it; its result reaches us too.
                None => {}
            }
        }

        // Whatever this connection no longer names, it no longer watches.
        inner.topics.retain(|key, topic| {
            if !keys.contains(key) {
                topic.watchers.remove(&id);
            }
            !topic.watchers.is_empty()
        });
        (snapshots, fresh)
    }

    /// The topics that have come due, marked as computing so the next tick passes them by.
    fn take_due(&self, now: Instant) -> Vec<(String, Watch)> {
        let mut inner = self.lock();
        inner
            .topics
            .iter_mut()
            .filter(|(_, topic)| {
                let free = match topic.computing {
                    None => true,
                    // A read that never came back — take the topic over rather than leave it mute.
                    Some(started) => now.saturating_duration_since(started) > STALE,
                };
                free && topic.due <= now
            })
            .map(|(key, topic)| {
                topic.computing = Some(now);
                (key.clone(), topic.watch.clone())
            })
            .collect()
    }

    /// Hand a computed answer to everyone watching — but only if it differs from the last one
    /// they were sent. Re-arms the topic either way.
    fn publish(&self, key: &str, message: String) {
        let mut inner = self.lock();
        let Some(topic) = inner.topics.get_mut(key) else {
            return; // the last watcher left while the read was out
        };
        topic.computing = None;
        topic.due = Instant::now() + topic.every;
        if topic.last.as_deref() == Some(&message) {
            return;
        }
        let message = Arc::new(message);
        topic.last = Some(Arc::clone(&message));
        let watchers: Vec<u64> = topic.watchers.iter().copied().collect();

        // A connection that can't keep up is dropped rather than buffered for; closing its
        // channel ends its task, and the browser reconnects.
        let mut stalled = Vec::new();
        for id in watchers {
            if let Some(tx) = inner.conns.get(&id)
                && tx.try_send(Arc::clone(&message)).is_err()
            {
                stalled.push(id);
            }
        }
        drop(inner);
        for id in stalled {
            debug!(conn = id, "dropping a websocket client that stopped reading");
            self.detach(id);
        }
    }
}

/// Compute one topic and publish it.
async fn compute(app: &Arc<App>, key: String, watch: Watch) {
    let req = http::Request {
        method: watch.method,
        path: watch.path,
        headers: HashMap::new(),
        body: watch.body.into_bytes(),
        rest: Vec::new(),
    };
    let response = crate::answer(app, req).await;
    app.live.publish(&key, message(&key, response.status, &response.body));
}

/// Render what goes over the wire: the topic it answers, its status, and the response body
/// spliced in as-is — the very bytes the same request would have returned over HTTP.
fn message(key: &str, status: u16, body: &str) -> String {
    let key = Value::String(key.to_string());
    format!("{{\"key\":{key},\"status\":{status},\"data\":{body}}}")
}

/// Run the hub's clock for as long as the app: recompute what has come due, off the connections'
/// tasks so one slow read never holds up a socket.
pub fn start(app: Arc<App>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            for (key, watch) in app.live.take_due(Instant::now()) {
                let app = Arc::clone(&app);
                tokio::spawn(async move { compute(&app, key, watch).await });
            }
        }
    });
}

/// Serve one `/api/ws` connection: finish the handshake, then pump subscriptions in and messages
/// out until either side hangs up.
///
/// # Errors
/// Fails on a socket error or a client that breaks framing; either way the connection is over.
pub async fn serve(
    mut stream: TcpStream,
    req: &http::Request,
    app: &Arc<App>,
) -> anyhow::Result<()> {
    let Some(key) = req.header("sec-websocket-key").filter(|k| !k.is_empty()) else {
        return http::write_json(
            &mut stream,
            400,
            "{\"error\":\"/api/ws expects a websocket upgrade\"}",
        )
        .await;
    };
    ws::write_upgrade(&mut stream, key).await?;

    let (mut rd, mut wr) = stream.into_split();
    let mut reader = ws::Reader::new(req.rest.clone());
    let (id, mut rx) = app.live.attach();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // the first tick is immediate; we want the first ping a period out

    // Every exit from here goes through the detach below — a connection left registered would
    // keep every topic it watches alive, and being computed, for the life of the process. So the
    // loop breaks with its outcome rather than using `?`.
    let outcome = loop {
        let step = tokio::select! {
            frame = reader.next(&mut rd) => match frame {
                Ok(Some(ws::Frame::Text(text))) => {
                    match take_subscription(app, id, &text, &mut wr).await {
                        Ok(fresh) => {
                            for (key, watch) in fresh {
                                let app = Arc::clone(app);
                                tokio::spawn(async move { compute(&app, key, watch).await });
                            }
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                Ok(Some(ws::Frame::Ping(payload))) => ws::write_pong(&mut wr, &payload).await,
                Ok(Some(ws::Frame::Pong)) => Ok(()),
                Ok(Some(ws::Frame::Close) | None) => break Ok(()),
                Err(e) => Err(e),
            },
            message = rx.recv() => match message {
                Some(message) => ws::write_text(&mut wr, &message).await,
                // The hub dropped us (a client that stopped reading).
                None => break Ok(()),
            },
            _ = ping.tick() => ws::write_ping(&mut wr).await,
        };
        if let Err(e) = step {
            break Err(e);
        }
    };

    app.live.detach(id);
    let _ = ws::write_close(&mut wr).await;
    outcome
}

/// Apply one client message — `{"sub":[{"method":…,"path":…,"body":…}, …]}`, the connection's
/// whole watch list — sending back whatever is already known. Returns the topics still to compute.
///
/// A message that isn't a subscription is ignored: this is the only thing a client may say, and a
/// socket is not worth closing over a stray frame.
async fn take_subscription<W: tokio::io::AsyncWrite + Unpin>(
    app: &Arc<App>,
    id: u64,
    text: &str,
    wr: &mut W,
) -> anyhow::Result<Vec<(String, Watch)>> {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
        return Ok(Vec::new());
    };
    let Some(Value::Array(subs)) = message.get("sub") else {
        return Ok(Vec::new());
    };
    let wanted = subs
        .iter()
        .filter_map(|sub| {
            Some(Watch {
                method: sub.get("method")?.as_str()?.to_string(),
                path: sub.get("path")?.as_str()?.to_string(),
                body: sub.get("body").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect();

    let (snapshots, fresh) = app.live.subscribe(id, wanted);
    for snapshot in snapshots {
        ws::write_text(wr, &snapshot).await?;
    }
    Ok(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(method: &str, path: &str, body: &str) -> Watch {
        Watch {
            method: method.into(),
            path: path.into(),
            body: body.into(),
        }
    }

    #[test]
    fn only_reads_are_watchable() {
        assert_eq!(watchable("GET", "/api/health"), Some(IDLE));
        assert_eq!(watchable("GET", "/api/tasks"), Some(SLOW));
        assert_eq!(watchable("POST", "/api/agents/peek"), Some(FAST));
        assert_eq!(watchable("GET", "/api/projects/acme"), Some(SLOW));
        assert_eq!(watchable("GET", "/api/fleet"), Some(SLOW));
        // A query parameter is part of the read, not a different one: the rail's page of the
        // session index is watchable exactly as the whole index is.
        assert_eq!(watchable("GET", "/api/agents/runs/all?limit=100"), Some(SLOW));
        // Mutations are not on the list, whatever they look like.
        assert_eq!(watchable("POST", "/api/fleet/unpair"), None);
        assert_eq!(watchable("POST", "/api/projects/remove"), None);
        assert_eq!(watchable("POST", "/api/agents/run"), None);
        assert_eq!(watchable("DELETE", "/api/health"), None);
        // …and a query cannot smuggle one on: the route is what is matched, still.
        assert_eq!(watchable("POST", "/api/agents/run?limit=1"), None);
    }

    /// Two pages of the same read are two topics: the path they are keyed by carries the query, so
    /// a rail showing a hundred sessions and one showing two hundred each get their own answer.
    #[test]
    fn a_page_of_a_read_is_its_own_topic() {
        assert_ne!(
            watch("GET", "/api/agents/runs/all?limit=100", "").key(),
            watch("GET", "/api/agents/runs/all?limit=200", "").key()
        );
    }

    #[test]
    fn a_topic_is_keyed_by_the_request_it_repeats() {
        assert_eq!(
            watch("POST", "/api/agents/peek", "{\"name\":\"adi-agent\"}").key(),
            "POST /api/agents/peek\n{\"name\":\"adi-agent\"}"
        );
        // Same read, different subject: two topics.
        assert_ne!(
            watch("POST", "/api/agents/peek", "{\"name\":\"a\"}").key(),
            watch("POST", "/api/agents/peek", "{\"name\":\"b\"}").key()
        );
    }

    #[test]
    fn the_message_carries_the_body_verbatim() {
        assert_eq!(
            message("GET /api/health\n", 200, "{\"ok\":true}"),
            "{\"key\":\"GET /api/health\\n\",\"status\":200,\"data\":{\"ok\":true}}"
        );
    }

    #[test]
    fn subscribing_creates_the_topic_and_asks_for_it_once() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let (snapshots, fresh) = hub.subscribe(a, vec![watch("GET", "/api/health", "")]);
        assert!(snapshots.is_empty());
        assert_eq!(fresh.len(), 1, "nothing cached yet, so it must be computed");

        // A second connection joins the same topic while the first read is still out: it does not
        // start a second read of the same thing.
        let (b, _rx_b) = hub.attach();
        let (snapshots, fresh) = hub.subscribe(b, vec![watch("GET", "/api/health", "")]);
        assert!(snapshots.is_empty() && fresh.is_empty());
        assert_eq!(hub.lock().topics.len(), 1);
    }

    #[tokio::test]
    async fn a_published_change_reaches_every_watcher_once() {
        let hub = Hub::default();
        let (a, mut rx_a) = hub.attach();
        let (b, mut rx_b) = hub.attach();
        let key = watch("GET", "/api/health", "").key();
        hub.subscribe(a, vec![watch("GET", "/api/health", "")]);
        hub.subscribe(b, vec![watch("GET", "/api/health", "")]);

        hub.publish(&key, message(&key, 200, "{\"up\":1}"));
        assert!(rx_a.recv().await.is_some());
        assert!(rx_b.recv().await.is_some());

        // The same answer again is not a change, so nothing is sent…
        hub.publish(&key, message(&key, 200, "{\"up\":1}"));
        assert!(rx_a.try_recv().is_err());
        // …and a different one is.
        hub.publish(&key, message(&key, 200, "{\"up\":2}"));
        assert!(rx_a.try_recv().is_ok());
    }

    #[test]
    fn a_later_subscribe_gets_what_is_already_known() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let key = watch("GET", "/api/health", "").key();
        hub.subscribe(a, vec![watch("GET", "/api/health", "")]);
        hub.publish(&key, message(&key, 200, "{\"up\":1}"));

        let (b, _rx_b) = hub.attach();
        let (snapshots, fresh) = hub.subscribe(b, vec![watch("GET", "/api/health", "")]);
        assert_eq!(snapshots.len(), 1, "the cached answer, straight away");
        assert!(fresh.is_empty(), "and no recomputation to get it");
    }

    #[test]
    fn dropping_what_you_watch_stops_the_work() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        hub.subscribe(
            a,
            vec![
                watch("GET", "/api/health", ""),
                watch("GET", "/api/tasks", ""),
            ],
        );
        assert_eq!(hub.lock().topics.len(), 2);

        // Navigating away: the new list replaces the old one wholesale.
        hub.subscribe(a, vec![watch("GET", "/api/tasks", "")]);
        assert_eq!(hub.lock().topics.len(), 1);

        // And closing the tab leaves nothing behind.
        hub.detach(a);
        assert!(hub.lock().topics.is_empty());
        assert!(hub.lock().conns.is_empty());
    }

    #[test]
    fn a_topic_survives_one_of_its_two_watchers_leaving() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let (b, _rx_b) = hub.attach();
        hub.subscribe(a, vec![watch("GET", "/api/health", "")]);
        hub.subscribe(b, vec![watch("GET", "/api/health", "")]);
        hub.detach(a);
        assert_eq!(hub.lock().topics.len(), 1);
    }

    #[test]
    fn only_due_topics_are_taken_and_only_once() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let key = watch("GET", "/api/tasks", "").key();
        hub.subscribe(a, vec![watch("GET", "/api/tasks", "")]);
        // Subscribe already claimed it for its first read.
        assert!(hub.take_due(Instant::now()).is_empty());

        hub.publish(&key, message(&key, 200, "{}"));
        // Just published: due one interval out, not now.
        assert!(hub.take_due(Instant::now()).is_empty());
        assert_eq!(hub.take_due(Instant::now() + SLOW).len(), 1);
        // Claimed by that call, so a second look finds nothing outstanding.
        assert!(hub.take_due(Instant::now() + SLOW).is_empty());
    }

    #[test]
    fn a_read_that_never_came_back_does_not_mute_its_topic_forever() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        // Subscribe claims the topic for a first read that, here, never publishes.
        hub.subscribe(a, vec![watch("GET", "/api/tasks", "")]);
        assert!(hub.take_due(Instant::now() + SLOW).is_empty());
        // Past the stale window the topic is offered again rather than staying silent.
        assert_eq!(hub.take_due(Instant::now() + STALE + SLOW).len(), 1);
    }
}
