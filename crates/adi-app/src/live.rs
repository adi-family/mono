//! Shared subscriptions over `GET /api/ws`, keyed by `(method, path, body)`.
//!
//! Each watched topic is computed once per interval; only changed answers are broadcast.
//! Unwatched topics stop computing but retain a snapshot until [`LINGER`] expires.
//! [`watchable`] restricts subscriptions to reads; they must never dispatch mutations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::debug;

use crate::{App, http, ws};

const TICK: Duration = Duration::from_millis(250);

/// Transcripts, terminal panes, and active logs.
const FAST: Duration = Duration::from_secs(1);

/// Lists that change when a person changes them.
const SLOW: Duration = Duration::from_secs(3);

/// Health uptime changes every second, so poll it less often to avoid constant broadcasts.
const IDLE: Duration = Duration::from_secs(15);

const PING_EVERY: Duration = Duration::from_secs(30);

const MAX_WATCHES: usize = 64;

/// Drop clients that exceed this backlog; the browser reconnects.
const QUEUE: usize = 256;

/// Retain unwatched snapshots for quick navigation back, without recomputing them.
const LINGER: Duration = Duration::from_secs(10 * 60);

/// Bound retained snapshots, evicting the longest-unwatched first.
const MAX_LINGERING: usize = 32;

/// Retry abandoned reads after this timeout so a lost task cannot permanently mute a topic.
const STALE: Duration = Duration::from_secs(60);

/// Read-only allowlist: subscriptions dispatch through the same routing as HTTP requests.
/// Match the route without its query; topic keys still include the full path.
fn watchable(method: &str, path: &str) -> Option<Duration> {
    let path = path.split('?').next().unwrap_or(path);
    // Forwarded reads use the same allowlist, with a slower interval for mesh round trips.
    if let Some((_, inner)) = crate::viewer::split_node_path(path) {
        return watchable(method, inner).map(|every| every.max(SLOW));
    }
    match method {
        "GET" => match path {
            "/api/health" => Some(IDLE),
            "/api/agents"
            | "/api/agents/runs/all"
            | "/api/dashboards"
            | "/api/db"
            | "/api/fleet"
            | "/api/fleet/nodes"
            | "/api/hive"
            | "/api/llm/backends"
            | "/api/marketplace"
            | "/api/mesh"
            | "/api/meta"
            | "/api/ports"
            | "/api/ports/used"
            | "/api/projects"
            | "/api/secrets"
            | "/api/settings/shared-assets"
            | "/api/tasks"
            | "/api/tools"
            | "/api/triggers" => Some(SLOW),
            p if p.starts_with("/api/projects/") => Some(SLOW),
            _ => None,
        },
        // POST reads carry their subject in the body.
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

/// One subscribed HTTP read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watch {
    method: String,
    path: String,
    body: String,
}

impl Watch {
    /// Keep this key identical to the HTTP request deduplication key.
    fn key(&self) -> String {
        format!("{} {}\n{}", self.method, self.path, self.body)
    }
}

#[derive(Debug)]
struct Topic {
    watch: Watch,
    every: Duration,
    watchers: HashSet<u64>,
    /// Both the change baseline and the snapshot for new subscribers.
    last: Option<Arc<String>>,
    due: Instant,
    /// Start of the outstanding read; expired claims can be retried after [`STALE`].
    computing: Option<Instant>,
    /// Start of the retention window while no watchers remain.
    unwatched_since: Option<Instant>,
}

#[derive(Debug, Default)]
struct Inner {
    topics: HashMap<String, Topic>,
    conns: HashMap<u64, mpsc::Sender<Arc<String>>>,
}

impl Inner {
    fn release(&mut self, id: u64, keep: impl Fn(&str) -> bool, now: Instant) {
        for (key, topic) in &mut self.topics {
            if !keep(key) && topic.watchers.remove(&id) && topic.watchers.is_empty() {
                topic.unwatched_since = Some(now);
            }
        }
        self.prune(now);
    }

    fn prune(&mut self, now: Instant) {
        self.topics.retain(|_, topic| {
            topic
                .unwatched_since
                .is_none_or(|since| now.saturating_duration_since(since) <= LINGER)
        });
        let mut idle: Vec<(Instant, String)> = self
            .topics
            .iter()
            .filter_map(|(key, topic)| topic.unwatched_since.map(|since| (since, key.clone())))
            .collect();
        if idle.len() <= MAX_LINGERING {
            return;
        }
        idle.sort_unstable();
        let excess = idle.len() - MAX_LINGERING;
        for (_, key) in idle.into_iter().take(excess) {
            self.topics.remove(&key);
        }
    }
}

#[derive(Debug, Default)]
pub struct Hub {
    inner: std::sync::Mutex<Inner>,
    next_conn: AtomicU64,
}

impl Hub {
    /// Recover poisoned locks so one panic does not fail all later connections.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn attach(&self) -> (u64, mpsc::Receiver<Arc<String>>) {
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(QUEUE);
        self.lock().conns.insert(id, tx);
        (id, rx)
    }

    fn detach(&self, id: u64) {
        let mut inner = self.lock();
        inner.conns.remove(&id);
        inner.release(id, |_| false, Instant::now());
    }

    /// Replace a connection's watch list, returning cached snapshots and reads to start now.
    fn subscribe(&self, id: u64, wanted: Vec<Watch>) -> (Vec<Arc<String>>, Vec<(String, Watch)>) {
        let now = Instant::now();
        let mut inner = self.lock();
        let mut snapshots = Vec::new();
        let mut fresh = Vec::new();
        let mut keys = HashSet::new();

        for watch in wanted.into_iter().take(MAX_WATCHES) {
            let Some(every) = watchable(&watch.method, &watch.path) else {
                debug!(method = %watch.method, path = %watch.path, "refusing to watch");
                // Silence would leave the client waiting indefinitely for its first answer.
                snapshots.push(Arc::new(message(
                    &watch.key(),
                    400,
                    &refused(&watch.method, &watch.path),
                )));
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
                unwatched_since: None,
            });
            if !topic.watchers.insert(id) {
                continue;
            }
            let was_unwatched = topic.unwatched_since.take().is_some();
            match &topic.last {
                Some(message) => {
                    snapshots.push(Arc::clone(message));
                    // Retained snapshots may be stale; show them immediately and refresh now.
                    if was_unwatched && topic.computing.is_none() {
                        topic.computing = Some(now);
                        fresh.push((key, watch));
                    }
                }
                None if topic.computing.is_none() => {
                    topic.computing = Some(now);
                    fresh.push((key, watch));
                }
                // An in-flight read will publish to this subscriber too.
                None => {}
            }
        }

        inner.release(id, |key| keys.contains(key), now);
        (snapshots, fresh)
    }

    /// Claim due topics before releasing the lock, preventing duplicate reads on the next tick.
    fn take_due(&self, now: Instant) -> Vec<(String, Watch)> {
        let mut inner = self.lock();
        inner.prune(now);
        inner
            .topics
            .iter_mut()
            .filter(|(_, topic)| !topic.watchers.is_empty())
            .filter(|(_, topic)| {
                let free = match topic.computing {
                    None => true,
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

    /// Publish changed answers and re-arm the topic even when unchanged.
    fn publish(&self, key: &str, message: String) {
        let mut inner = self.lock();
        let Some(topic) = inner.topics.get_mut(key) else {
            return; // let go while the read was out (see `LINGER`)
        };
        topic.computing = None;
        topic.due = Instant::now() + topic.every;
        if topic.last.as_deref() == Some(&message) {
            return;
        }
        let message = Arc::new(message);
        topic.last = Some(Arc::clone(&message));
        let watchers: Vec<u64> = topic.watchers.iter().copied().collect();

        // Closing a stalled client's channel ends its task and lets the browser reconnect.
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
            debug!(
                conn = id,
                "dropping a websocket client that stopped reading"
            );
            self.detach(id);
        }
    }
}

async fn compute(app: &Arc<App>, key: String, watch: Watch) {
    let req = http::Request {
        method: watch.method,
        path: watch.path,
        headers: HashMap::new(),
        body: watch.body.into_bytes(),
        rest: Vec::new(),
    };
    let response = crate::answer(app, req).await;
    app.live
        .publish(&key, message(&key, response.status, &response.body));
}

fn refused(method: &str, path: &str) -> String {
    crate::handlers::error(
        400,
        &format!("the live channel cannot watch {method} {path}"),
    )
    .body
}

/// Insert the HTTP response body verbatim as JSON data.
fn message(key: &str, status: u16, body: &str) -> String {
    let key = Value::String(key.to_string());
    format!("{{\"key\":{key},\"status\":{status},\"data\":{body}}}")
}

/// Recompute due topics outside connection tasks so slow reads cannot block sockets.
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

/// Serve subscriptions and updates on one `/api/ws` connection.
///
/// # Errors
/// Fails on a socket error or invalid client framing.
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

    // Break instead of using `?` so every exit detaches the connection and its watches.
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

/// Replace the watch list with `{"sub":[{"method":…,"path":…,"body":…}, …]}`.
/// Send cached snapshots and return reads to start; ignore other messages.
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
                body: sub
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
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

    const PANEL_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../adi-webapp/src");

    /// Prevent scanner drift from silently turning this into a test of an empty list.
    const FEWEST_PLAUSIBLE_SUBS: usize = 15;

    /// Scan the panel's subscriptions so additions cannot silently bypass the allowlist test.
    /// Replace formatted path segments with samples for prefix-matched routes.
    fn subscribed_by_the_panel() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && let Ok(source) = std::fs::read_to_string(&path)
                {
                    scan(&source, out);
                }
            }
        }

        // Each subscription's path is its first `/api/…` string literal.
        fn scan(source: &str, out: &mut Vec<(String, String)>) {
            for (call, method) in [
                ("Sub::get(", "GET"),
                ("Sub::get_on(", "GET"),
                ("Sub::post(", "POST"),
                ("Sub::post_on(", "POST"),
            ] {
                for (at, _) in source.match_indices(call) {
                    let rest = &source[at + call.len()..];
                    let Some(open) = rest.find("\"/api/") else {
                        continue;
                    };
                    let literal = &rest[open + 1..];
                    let Some(end) = literal.find('"') else {
                        continue;
                    };
                    let path = literal[..end]
                        .split('/')
                        .map(|seg| if seg.starts_with('{') { "sample" } else { seg })
                        .collect::<Vec<_>>()
                        .join("/");
                    out.push((method.to_string(), path));
                }
            }
        }

        let mut found = Vec::new();
        walk(std::path::Path::new(PANEL_SRC), &mut found);
        found.sort();
        found.dedup();
        found
    }

    #[test]
    fn every_read_the_panel_watches_is_watchable() {
        if !std::path::Path::new(PANEL_SRC).is_dir() {
            return; // built without its sibling crate; there is nothing to compare against
        }
        let subs = subscribed_by_the_panel();
        assert!(
            subs.len() >= FEWEST_PLAUSIBLE_SUBS,
            "only found {} subscriptions in {PANEL_SRC} — the scan has stopped matching how the \
             panel writes them, and is no longer guarding anything",
            subs.len()
        );
        for (method, path) in &subs {
            assert!(
                watchable(method, path).is_some(),
                "the control panel subscribes to {method} {path} and this channel will not watch \
                 it — the page that wants it never gets an answer, and sits on \"Loading…\""
            );
        }
    }

    #[test]
    fn a_refused_subscription_is_answered_rather_than_ignored() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let (snapshots, fresh) = hub.subscribe(a, vec![watch("POST", "/api/agents/run", "")]);
        assert!(
            fresh.is_empty(),
            "nothing is computed for a read off the list"
        );
        assert_eq!(snapshots.len(), 1, "but the client is told");
        assert!(snapshots[0].contains("\"status\":400"), "{}", snapshots[0]);
        assert!(snapshots[0].contains("cannot watch"), "{}", snapshots[0]);
        assert!(hub.lock().topics.is_empty(), "and no topic is left behind");
    }

    #[test]
    fn only_reads_are_watchable() {
        assert_eq!(watchable("GET", "/api/health"), Some(IDLE));
        assert_eq!(watchable("GET", "/api/tasks"), Some(SLOW));
        assert_eq!(watchable("GET", "/api/llm/backends"), Some(SLOW));
        assert_eq!(watchable("POST", "/api/agents/peek"), Some(FAST));
        assert_eq!(watchable("GET", "/api/projects/acme"), Some(SLOW));
        assert_eq!(watchable("GET", "/api/fleet"), Some(SLOW));
        assert_eq!(
            watchable("GET", "/api/agents/runs/all?limit=100"),
            Some(SLOW)
        );
        assert_eq!(watchable("POST", "/api/fleet/unpair"), None);
        assert_eq!(watchable("POST", "/api/projects/remove"), None);
        assert_eq!(watchable("POST", "/api/agents/run"), None);
        assert_eq!(watchable("DELETE", "/api/health"), None);
        assert_eq!(watchable("POST", "/api/agents/run?limit=1"), None);
    }

    #[test]
    fn a_nodes_read_is_watchable_but_never_at_the_local_rate() {
        assert_eq!(
            watchable("GET", "/api/node/laptop-b/api/agents"),
            Some(SLOW)
        );
        assert_eq!(
            watchable("GET", "/api/node/laptop-b/api/agents/runs/all?limit=100"),
            Some(SLOW)
        );
        assert_eq!(
            watchable("POST", "/api/node/laptop-b/api/agents/run/peek"),
            Some(SLOW)
        );
        assert_eq!(watchable("POST", "/api/node/laptop-b/api/agents/run"), None);
        assert_eq!(watchable("GET", "/api/node/laptop-b/api/ws"), None);
        assert_eq!(watchable("GET", "/api/node/laptop-b/index.html"), None);
        assert_eq!(
            watchable("GET", "/api/node/laptop-b/api/node/studio/api/agents"),
            None
        );
    }

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

        hub.publish(&key, message(&key, 200, "{\"up\":1}"));
        assert!(rx_a.try_recv().is_err());
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

        let health = watch("GET", "/api/health", "").key();
        let tasks = watch("GET", "/api/tasks", "").key();
        hub.publish(&health, message(&health, 200, "{}"));
        hub.publish(&tasks, message(&tasks, 200, "{}"));

        hub.subscribe(a, vec![watch("GET", "/api/tasks", "")]);
        let due: Vec<String> = hub
            .take_due(Instant::now() + SLOW)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(due, vec![tasks]);

        hub.detach(a);
        assert!(hub.lock().conns.is_empty());
        assert!(hub.take_due(Instant::now() + STALE + SLOW).is_empty());
        assert_eq!(hub.lock().topics.len(), 2);
        hub.take_due(Instant::now() + LINGER + SLOW);
        assert!(hub.lock().topics.is_empty());
    }

    #[test]
    fn going_back_to_a_page_paints_from_what_was_last_known() {
        let hub = Hub::default();
        let read = watch("GET", "/api/agents/runs/all", "");
        let key = read.key();
        let (a, _rx_a) = hub.attach();
        hub.subscribe(a, vec![read.clone()]);
        hub.publish(&key, message(&key, 200, "{\"runs\":1}"));
        hub.detach(a);

        let (b, _rx_b) = hub.attach();
        let (snapshots, fresh) = hub.subscribe(b, vec![read]);
        assert_eq!(snapshots.len(), 1, "the last answer, straight away");
        assert_eq!(
            fresh.len(),
            1,
            "and a fresh read of it, at once rather than an interval out"
        );
    }

    #[test]
    fn what_is_kept_unwatched_is_bounded() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        let reads: Vec<Watch> = (0..MAX_LINGERING + 5)
            .map(|n| {
                watch(
                    "POST",
                    "/api/agents/run/peek",
                    &format!("{{\"run_id\":\"{n}\"}}"),
                )
            })
            .collect();
        hub.subscribe(a, reads);
        hub.detach(a);
        assert_eq!(hub.lock().topics.len(), MAX_LINGERING);
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
        assert!(hub.take_due(Instant::now()).is_empty());

        hub.publish(&key, message(&key, 200, "{}"));
        assert!(hub.take_due(Instant::now()).is_empty());
        assert_eq!(hub.take_due(Instant::now() + SLOW).len(), 1);
        assert!(hub.take_due(Instant::now() + SLOW).is_empty());
    }

    #[test]
    fn a_read_that_never_came_back_does_not_mute_its_topic_forever() {
        let hub = Hub::default();
        let (a, _rx_a) = hub.attach();
        hub.subscribe(a, vec![watch("GET", "/api/tasks", "")]);
        assert!(hub.take_due(Instant::now() + SLOW).is_empty());
        assert_eq!(hub.take_due(Instant::now() + STALE + SLOW).len(), 1);
    }
}
