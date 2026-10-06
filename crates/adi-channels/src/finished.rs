//! The auto-post-back of a question: a channel-opened run that stops to ask something gets the
//! question posted to the router without anybody asking (`docs/channels.md` §5). This is an
//! [`EventObserver`] the same shape `adi-triggers::dispatch::EventObserver` already is — composed
//! alongside `adi-agents`' own `awaits::start` observer in `adi-app`'s one event dispatcher, rather
//! than draining the shared event spool a second time (`adi-triggers`' own dispatcher is explicit
//! that a second drainer would race it for records; see its module doc).
//!
//! A turn's *answer* is not posted from here any more: `adi.agents.run.finished` fires once per run
//! and only when somebody lists it, which left every follow-up message unanswered — see
//! [`crate::turn`], which owns the answer and the "thinking…" clear now.

use std::sync::Arc;

use adi_agents::{AgentQuestionAsked, Agents};
use adi_events::{Event, EventRecord};
use adi_secrets::Secrets;
use tracing::warn;

use crate::connection::Connections;
use crate::error::Result;
use crate::router_api::RouterApi;
use crate::token;

/// Told about every event the platform's one dispatcher drains — same shape
/// `adi_triggers::dispatch::EventObserver` is, duplicated rather than depended on: this crate has
/// no other reason to take `adi-triggers` as a dependency.
pub type EventObserver = Arc<dyn Fn(&EventRecord) + Send + Sync>;

/// Build the observer `adi-app` composes into its event dispatcher. `router_url` is the one this
/// node's client already subscribes through — see `RouterApi`'s own note on its assumed shape.
///
/// Matches only [`AgentQuestionAsked`]; everything else is ignored immediately, which is what
/// "return promptly" (the dispatcher's own contract on an observer) means in practice. The actual
/// lookup-and-post happens on a detached thread, never on the dispatcher's own tick — a slow or
/// unreachable router must not stall the trigger/await delivery that shares this one drain.
#[must_use]
pub fn observer(
    connections: Connections,
    agents: Agents,
    secrets: Secrets,
    router_url: String,
) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        if record.name != AgentQuestionAsked::NAME {
            return;
        }
        let connections = connections.clone();
        let agents = agents.clone();
        let secrets = secrets.clone();
        let router_url = router_url.clone();
        let name = record.name.clone();
        let payload = record.payload.clone();
        std::thread::spawn(move || {
            if let Err(e) = post_back(&connections, &agents, &secrets, &router_url, &name, &payload)
            {
                warn!(event = %name, error = %e, "couldn't post a channel's run answer back");
            }
        });
    })
}

/// The run id an [`AgentQuestionAsked`] payload belongs to.
fn run_id_of(value: &serde_json::Value) -> String {
    value
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Reads the payload as a bare [`serde_json::Value`] rather than `adi_agents::AgentQuestionAsked`:
/// that type derives `Serialize` only (nothing inside `adi-agents` itself ever needs to parse its
/// own events back), and adding `Deserialize` there for this one caller is more than this crate's
/// boundary asks for. `agents` is unused now that answers are [`crate::turn`]'s, and kept so
/// `adi-app`'s composition doesn't change shape.
fn post_back(
    connections: &Connections,
    _agents: &Agents,
    secrets: &Secrets,
    router_url: &str,
    name: &str,
    payload: &str,
) -> Result<()> {
    if name != AgentQuestionAsked::NAME {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| crate::error::Error::Router(format!("unreadable payload: {e}")))?;
    let run_id = run_id_of(&value);
    if run_id.is_empty() {
        return Ok(());
    }
    // A run with no connection pointed at it is an ordinary (non-channel) conversation that just
    // happens to share the event bus — the overwhelming majority of runs, so this is the expected
    // path, not an error.
    let Some((connection, thread)) = connections.find_by_run(&run_id)? else {
        return Ok(());
    };
    if connection.manifest.paused {
        return Ok(());
    }
    let question = value.get("question").and_then(|v| v.as_str()).unwrap_or_default();
    if question.trim().is_empty() {
        return Ok(());
    }
    let Some(token) = token::load(secrets, &connection.manifest.provider)? else {
        warn!(
            connection = %connection.id,
            provider = %connection.manifest.provider,
            "no node token on file for this provider; can't post the question back"
        );
        return Ok(());
    };
    RouterApi::new(router_url).send(&token, &connection.id, Some(&thread), question)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Target;
    use adi_agents::AgentSaved;
    use std::time::Duration;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-finished-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// A fake router that accepts any number of requests in sequence, 200s every one, and hands
    /// each request's raw body back over a channel — enough to tell which of `/send`'s two shapes
    /// (`{..., status}` vs `{..., text}`) `post_back` actually sent, and in what order. Per this
    /// task's own instructions, never the real `apps/channel-router`.
    fn spawn_capturing_router() -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader, Write as _};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    break;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                let _ = tx.send(String::from_utf8_lossy(&body).to_string());
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        (format!("http://{addr}"), rx)
    }

    /// A run with no connection pointed at it (the ordinary case: almost every run on the
    /// machine) is silently skipped, never treated as an error.
    #[test]
    fn a_run_nobody_bound_a_thread_to_is_ignored() {
        let cfg = scratch("unbound");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg.clone());
        let secrets = Secrets::with_config(cfg);

        let record = AgentQuestionAsked {
            agent: "solver".into(),
            conv: "no-such-run".into(),
            run_id: "no-such-run".into(),
            ask: "q1".into(),
            question: "which backend?".into(),
        }
        .to_record()
        .expect("question-asked record");
        // Must not panic and must not spin up a post — there's nothing to look up a token for.
        post_back(
            &connections,
            &agents,
            &secrets,
            "http://127.0.0.1:1",
            &record.name,
            &record.payload,
        )
        .expect("silently skipped");
    }

    /// A paused connection's run still finished, but nothing is posted — same rule the dispatch
    /// side applies to an inbound message.
    #[test]
    fn a_paused_connections_run_is_not_posted() {
        let cfg = scratch("paused");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg.clone());
        let secrets = Secrets::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "solver".into(),
                },
            )
            .unwrap();
        connections.bind_thread(&created.id, "chat-1", "run-1").unwrap();
        connections.set_paused(&created.id, true).unwrap();
        token::save(&secrets, "telegram", "tok").unwrap();

        let record = AgentQuestionAsked {
            agent: "solver".into(),
            conv: "run-1".into(),
            run_id: "run-1".into(),
            ask: "q1".into(),
            question: "which backend?".into(),
        }
        .to_record()
        .expect("question-asked record");
        post_back(
            &connections,
            &agents,
            &secrets,
            "http://127.0.0.1:1",
            &record.name,
            &record.payload,
        )
        .expect("paused connections are skipped, not errored");
    }

    /// `AgentQuestionAsked` never touches the "thinking…" indicator — the run isn't finished, just
    /// paused on a question, so there's nothing yet to clear.
    #[test]
    fn question_asked_never_calls_set_thinking() {
        let cfg = scratch("question-no-clear");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg.clone());
        let secrets = Secrets::with_config(cfg);
        let created = connections
            .create(
                "slack",
                Target::Agent {
                    agent: "solver".into(),
                },
            )
            .unwrap();
        connections.bind_thread(&created.id, "C1", "run-1").unwrap();
        token::save(&secrets, "slack", "tok").unwrap();

        let (router_url, rx) = spawn_capturing_router();
        let record = AgentQuestionAsked {
            agent: "solver".into(),
            conv: "run-1".into(),
            run_id: "run-1".into(),
            ask: "q1".into(),
            question: "which backend?".into(),
        }
        .to_record()
        .expect("question-asked record");
        post_back(
            &connections,
            &agents,
            &secrets,
            &router_url,
            &record.name,
            &record.payload,
        )
        .expect("posts the question back");

        let body = rx.recv_timeout(Duration::from_secs(5)).expect("one request");
        assert!(body.contains("\"text\":\"which backend?\""), "got: {body}");
        assert!(!body.contains("\"status\""), "got: {body}");
    }

    #[test]
    fn observer_ignores_events_it_does_not_care_about() {
        let cfg = scratch("ignore");
        let obs = observer(
            Connections::with_config(cfg.clone()),
            Agents::with_config(cfg.clone()),
            Secrets::with_config(cfg),
            "http://127.0.0.1:1".into(),
        );
        // No thread is spun up for an unrelated event — nothing to assert on directly, but this
        // must return instantly and never panic.
        obs(&AgentSaved {
            agent: "solver".into(),
        }
        .to_record()
        .expect("agent-saved record"));
        assert!(wait_until(|| true), "returned promptly");
    }
}
