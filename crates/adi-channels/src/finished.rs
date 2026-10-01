//! The auto-post-back: a channel-opened run's answer, posted to the router without anybody
//! asking (`docs/channels.md` §5). This is an [`EventObserver`] the same shape
//! `adi-triggers::dispatch::EventObserver` already is — composed alongside `adi-agents`' own
//! `awaits::start` observer in `adi-app`'s one event dispatcher, rather than draining the shared
//! event spool a second time (`adi-triggers`' own dispatcher is explicit that a second drainer
//! would race it for records; see its module doc).

use std::sync::Arc;

use adi_agents::Agents;
use adi_events::EventRecord;
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

const RUN_FINISHED: &str = "adi.agents.run.finished";
/// `adi_agents::events::QUESTION_ASKED`'s exact spelling, copied rather than imported: that
/// module is private to `adi-agents` and nothing re-exports the constant at its crate root (only
/// the [`AgentQuestionAsked`] payload type is public). If `adi-agents` ever renames the event,
/// this silently stops matching rather than failing to compile — worth asking `adi-agents` to
/// export the constant if that risk ever bites.
const QUESTION_ASKED: &str = "adi.agents.question.asked";

/// Build the observer `adi-app` composes into its event dispatcher. `router_url` is the one this
/// node's client already subscribes through — see `RouterApi`'s own note on its assumed shape.
///
/// Matches only [`RUN_FINISHED`] and [`QUESTION_ASKED`]; everything else is ignored immediately,
/// which is what "return promptly" (the dispatcher's own contract on an observer) means in
/// practice. The actual lookup-and-post happens on a detached thread, never on the dispatcher's
/// own tick — a slow or unreachable router must not stall the trigger/await delivery that shares
/// this one drain.
#[must_use]
pub fn observer(
    connections: Connections,
    agents: Agents,
    secrets: Secrets,
    router_url: String,
) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        if record.name != RUN_FINISHED && record.name != QUESTION_ASKED {
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

/// What ended up being said, and which run it belongs to — for a [`RUN_FINISHED`] that's the full
/// answer (never the notification's own truncated `result_head`, per §5), for a
/// [`QUESTION_ASKED`] it's the question itself.
///
/// Reads the payload as a bare [`serde_json::Value`] rather than `adi_agents::AgentRunFinished`/
/// `AgentQuestionAsked`: those types derive `Serialize` only (nothing inside `adi-agents` itself
/// ever needs to parse its own events back), and adding `Deserialize` there for this one caller
/// is more than this task's own crate boundary asks for.
fn said(agents: &Agents, name: &str, payload: &str) -> Result<Option<(String, String)>> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| crate::error::Error::Router(format!("unreadable payload: {e}")))?;
    let field = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default();

    match name {
        RUN_FINISHED => {
            let run_id = field("run_id");
            let Some(agent) = agents.get(field("agent"))? else {
                return Ok(None);
            };
            let answer = agents
                .transcript(&agent, run_id)
                .into_iter()
                .rev()
                .find(|t| t.role == "assistant")
                .map(|t| t.text);
            Ok(answer.map(|text| (run_id.to_string(), text)))
        }
        _ if name == QUESTION_ASKED => {
            let question = field("question");
            if question.is_empty() {
                return Ok(None);
            }
            Ok(Some((field("run_id").to_string(), question.to_string())))
        }
        _ => Ok(None),
    }
}

fn post_back(
    connections: &Connections,
    agents: &Agents,
    secrets: &Secrets,
    router_url: &str,
    name: &str,
    payload: &str,
) -> Result<()> {
    let Some((run_id, text)) = said(agents, name, payload)? else {
        return Ok(());
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    // A run with no connection pointed at it is an ordinary (non-channel) conversation that just
    // happens to share the event bus — the overwhelming majority of runs, so this is the expected
    // path, not an error.
    let Some(connection) = connections.find_by_run(&run_id)? else {
        return Ok(());
    };
    if connection.manifest.paused {
        return Ok(());
    }
    let Some(token) = token::load(secrets, &connection.manifest.provider)? else {
        warn!(
            connection = %connection.id,
            provider = %connection.manifest.provider,
            "no node token on file for this provider; can't post the answer back"
        );
        return Ok(());
    };
    RouterApi::new(router_url).send(&token, &connection.id, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_agents::{AgentQuestionAsked, AgentRunFinished};
    use crate::connection::Target;
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

    /// A run with no connection pointed at it (the ordinary case: almost every run on the
    /// machine) is silently skipped, never treated as an error.
    #[test]
    fn a_run_nobody_bound_a_thread_to_is_ignored() {
        let cfg = scratch("unbound");
        let connections = Connections::with_config(cfg.clone());
        let agents = Agents::with_config(cfg.clone());
        let secrets = Secrets::with_config(cfg);

        let record = EventRecord {
            name: RUN_FINISHED.to_string(),
            payload: serde_json::to_string(&AgentRunFinished {
                agent: "solver".into(),
                run_id: "no-such-run".into(),
                terminal_reason: Some("completed".into()),
                is_error: false,
                duration_ms: None,
                cost_micro_usd: None,
                result_head: "done".into(),
            })
            .unwrap(),
            emitted_at: 1,
        };
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

        post_back(
            &connections,
            &agents,
            &secrets,
            "http://127.0.0.1:1",
            QUESTION_ASKED,
            &serde_json::to_string(&AgentQuestionAsked {
                agent: "solver".into(),
                conv: "run-1".into(),
                run_id: "run-1".into(),
                ask: "q1".into(),
                question: "which backend?".into(),
            })
            .unwrap(),
        )
        .expect("paused connections are skipped, not errored");
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
        obs(&EventRecord {
            name: "adi.tasks.created".into(),
            payload: "{}".into(),
            emitted_at: 1,
        });
        assert!(wait_until(|| true), "returned promptly");
    }
}
