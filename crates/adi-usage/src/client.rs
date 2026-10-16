//! One HTTPS `POST <relay>/v1/app` per flush — the collector's app route
//! (`collector:src/surface.ts`'s `ROUTES.app`, `collector:src/collect.rs`'s `RECORD_WIRE` and
//! `BATCH_LIMITS.app`). No key, no `Origin` header (plain `reqwest`, never a browser call), a 5 s
//! timeout, and nothing here ever blocks a request this process is answering — the caller runs
//! this off the dispatch loop's own timer, never inline with a handler.

use std::collections::HashSet;
use std::time::Duration;

use adi_config::Module;
use serde_json::{Value, json};

use crate::queue::{self, QueuedEvent};

/// `relay.withadi.dev` in production. Overridable so a test (or a local run against the mock
/// collector in `adi-analytics`) never reaches the real one — see [`RELAY_URL_ENV`].
pub const DEFAULT_RELAY_URL: &str = "https://relay.withadi.dev";

/// Overrides [`DEFAULT_RELAY_URL`]. Set this to a local mock endpoint for anything that would
/// otherwise send a real event — never point it at the real relay from a test.
pub const RELAY_URL_ENV: &str = "ADI_ANALYTICS_RELAY_URL";

/// `collector:src/collect.rs`'s `BATCH_LIMITS.app`.
const MAX_BATCH_EVENTS: usize = 100;
const MAX_BATCH_BYTES: usize = 64 * 1024;

const TIMEOUT: Duration = Duration::from_secs(5);

/// What one [`flush`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Nothing queued (or everything queued had already aged past 7 days).
    Empty,
    /// This many events were sent and accepted; they are no longer on disk.
    Sent(usize),
    /// A POST was attempted and failed (network, timeout, or a non-2xx answer). Nothing was
    /// removed from the queue — the next flush retries the same items.
    Failed,
}

/// [`DEFAULT_RELAY_URL`], or [`RELAY_URL_ENV`] when it is set to something non-blank.
#[must_use]
pub fn relay_url() -> String {
    std::env::var(RELAY_URL_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_RELAY_URL.to_string())
}

/// `rustls-no-provider` needs an explicit provider before a client can be built; idempotent, and
/// there is no startup hook in this crate to install it from instead — the dispatch loop's first
/// tick may be the first thing in the process to build a `reqwest::Client` at all.
fn ensure_tls_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

/// Age out anything past 7 days, take up to one batch, and POST it. Removes exactly the items the
/// collector accepted — anything queued after the batch was built (or left over because the queue
/// was bigger than one batch) stays for the next flush.
pub async fn flush(module: &Module, install_id: &str, now_ms: u64) -> FlushOutcome {
    let items = queue::drop_stale(module, now_ms);
    let batch = take_batch(&items);
    if batch.is_empty() {
        return FlushOutcome::Empty;
    }

    ensure_tls_provider();
    let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(error = %e, "could not build the usage-events HTTP client");
            return FlushOutcome::Failed;
        }
    };
    let url = format!("{}/v1/app", relay_url().trim_end_matches('/'));
    let body = build_body(install_id, &batch);
    let sent = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    match sent {
        Ok(resp) if resp.status().is_success() => {
            let ids: HashSet<String> = batch.iter().map(|i| i.id.clone()).collect();
            queue::remove(module, &ids);
            FlushOutcome::Sent(batch.len())
        }
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "usage-events flush refused");
            FlushOutcome::Failed
        }
        Err(e) => {
            tracing::debug!(error = %e, "usage-events flush failed");
            FlushOutcome::Failed
        }
    }
}

/// The largest prefix of `items` that fits [`MAX_BATCH_EVENTS`] and (approximately —
/// per-item size, not the exact serialized batch) [`MAX_BATCH_BYTES`].
fn take_batch(items: &[QueuedEvent]) -> Vec<QueuedEvent> {
    let mut batch = Vec::new();
    let mut bytes = 0usize;
    for item in items {
        if batch.len() >= MAX_BATCH_EVENTS {
            break;
        }
        let size = serde_json::to_vec(item).map_or(0, |b| b.len());
        if bytes + size > MAX_BATCH_BYTES && !batch.is_empty() {
            break;
        }
        bytes += size;
        batch.push(item.clone());
    }
    batch
}

/// `RECORD_WIRE`'s body shape: `distinct_id` once at the top (every item here is this one
/// install), then `events: [{ event, properties, ts, id }]`.
fn build_body(install_id: &str, items: &[QueuedEvent]) -> String {
    let events: Vec<Value> = items
        .iter()
        .map(|item| {
            json!({
                "event": item.event,
                "properties": item.properties,
                "ts": to_rfc3339(item.ts_ms),
                "id": item.id,
            })
        })
        .collect();
    json!({ "distinct_id": install_id, "events": events }).to_string()
}

fn to_rfc3339(ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(i64::try_from(ms).unwrap_or(0))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::enqueue;

    /// Every test below sets [`RELAY_URL_ENV`] (process-global), and `cargo test` runs a crate's
    /// tests on multiple threads by default — this serializes them against each other the same
    /// way `settings::ENV_TEST_LOCK` does for its own two vars.
    static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch(tag: &str) -> Module {
        let root = std::env::temp_dir().join(format!(
            "adi-usage-client-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root).module("analytics")
    }

    fn item(id: &str) -> QueuedEvent {
        QueuedEvent {
            id: id.to_string(),
            event: "fleet_invite_created".to_string(),
            ts_ms: 1_760_000_000_000,
            properties: json!({ "version": "1.30.0", "os": "linux", "arch": "x86_64" }),
        }
    }

    #[test]
    fn relay_url_falls_back_to_the_default_when_the_override_is_blank() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe { std::env::remove_var(RELAY_URL_ENV) };
        assert_eq!(relay_url(), DEFAULT_RELAY_URL);
        unsafe { std::env::set_var(RELAY_URL_ENV, "  ") };
        assert_eq!(relay_url(), DEFAULT_RELAY_URL);
        unsafe { std::env::set_var(RELAY_URL_ENV, "http://127.0.0.1:9") };
        assert_eq!(relay_url(), "http://127.0.0.1:9");
        unsafe { std::env::remove_var(RELAY_URL_ENV) };
    }

    #[test]
    fn take_batch_caps_the_event_count() {
        let items: Vec<QueuedEvent> = (0..150).map(|n| item(&n.to_string())).collect();
        assert_eq!(take_batch(&items).len(), MAX_BATCH_EVENTS);
    }

    #[test]
    fn the_wire_body_carries_distinct_id_once_and_every_item() {
        let batch = vec![item("a"), item("b")];
        let body: Value = serde_json::from_str(&build_body("install-123", &batch)).expect("json");
        assert_eq!(body["distinct_id"], "install-123");
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
        assert_eq!(body["events"][0]["event"], "fleet_invite_created");
        assert_eq!(body["events"][0]["id"], "a");
        assert!(body["events"][0]["ts"].as_str().unwrap().starts_with("2025-"));
    }

    #[tokio::test]
    async fn flushing_an_empty_queue_sends_nothing() {
        let module = scratch("empty");
        assert_eq!(flush(&module, "install-1", 0).await, FlushOutcome::Empty);
    }

    // The lock below is a `std::sync::Mutex` held across an `.await`, which clippy flags on
    // principle — safe here because it only ever serializes this crate's own single-threaded
    // tests against each other (never contended across a real async boundary) and is dropped
    // well within the test's own current-thread runtime.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_batch_that_fails_to_send_is_left_queued_for_the_next_attempt() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let module = scratch("unreachable");
        enqueue(&module, item("a"));
        // Nothing listens on this port; the POST itself fails rather than getting a response.
        unsafe { std::env::set_var(RELAY_URL_ENV, "http://127.0.0.1:1") };
        let outcome = flush(&module, "install-1", 1_760_000_000_000).await;
        unsafe { std::env::remove_var(RELAY_URL_ENV) };
        assert_eq!(outcome, FlushOutcome::Failed);
        assert_eq!(queue::read(&module).len(), 1, "the failed item stays queued");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn a_batch_posts_to_v1_app_with_no_key_and_no_origin_and_is_then_cleared() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let module = scratch("posts");
        enqueue(&module, item("a"));
        enqueue(&module, item("b"));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .await
                .unwrap();
            request
        });

        unsafe { std::env::set_var(RELAY_URL_ENV, format!("http://{addr}")) };
        let outcome = flush(&module, "install-1", 1_760_000_000_000).await;
        unsafe { std::env::remove_var(RELAY_URL_ENV) };

        assert_eq!(outcome, FlushOutcome::Sent(2));
        assert!(queue::read(&module).is_empty(), "the sent batch is cleared");

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/app"), "{request}");
        assert!(!request.to_lowercase().contains("authorization:"), "{request}");
        assert!(!request.to_lowercase().contains("origin:"), "{request}");
        assert!(request.contains("\"distinct_id\":\"install-1\""), "{request}");
    }
}
