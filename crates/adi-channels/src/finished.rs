//! Forward pending agent questions from the Hive-managed channels daemon.
//!
//! Questions already live in the session store. Polling the conversations bound to channel
//! threads keeps delivery independent of the control panel and leaves the platform's single
//! event-spool consumer alone. Successful sends are checkpointed so normal daemon restarts do
//! not repeat a pending question. A crash after the remote send but before its checkpoint can
//! still repeat it: the router has no idempotency key with which to make that boundary atomic.
//!
//! Final answers and the "thinking…" indicator remain owned by [`crate::turn`].

use std::collections::BTreeMap;

use adi_agents::Agents;
use adi_config::ConfigFile;
use adi_secrets::Secrets;
use tracing::warn;

use crate::connection::{Connection, Connections, Target};
use crate::error::Result;
use crate::router_api::RouterApi;
use crate::token;

type Deliveries = BTreeMap<String, String>;

/// One sequential question-delivery worker. Call [`Self::tick`] on a blocking thread about once
/// a second; a slow router must not block the daemon's HTTP or WebSocket runtime.
#[derive(Debug)]
pub struct QuestionForwarder {
    connections: Connections,
    agents: Agents,
    secrets: Secrets,
    router_url: String,
    delivered: Option<Deliveries>,
    dirty: bool,
}

impl QuestionForwarder {
    #[must_use]
    pub fn new(
        connections: Connections,
        agents: Agents,
        secrets: Secrets,
        router_url: String,
    ) -> Self {
        Self {
            connections,
            agents,
            secrets,
            router_url,
            delivered: None,
            dirty: false,
        }
    }

    fn checkpoint(&self) -> ConfigFile<Deliveries> {
        // Nested below the manifests: Connections::list only reads top-level TOML files.
        self.connections
            .config()
            .module("channels")
            .file("state/question-deliveries.toml")
    }

    fn flush(&mut self) -> Result<()> {
        if self.dirty {
            if let Some(delivered) = &self.delivered {
                self.checkpoint().save(delivered)?;
                self.dirty = false;
            }
        }
        Ok(())
    }

    /// Send each newly pending question from a linked, unpaused channel conversation.
    /// A missing token or a failed router request leaves it pending for the next tick.
    /// Returns the number of successful sends, without changing the question's answer state.
    ///
    /// # Errors
    /// Connection-store or checkpoint errors. An unreadable checkpoint is never discarded: that
    /// would resend questions after a restart. Router/token errors are logged per connection so
    /// one unavailable provider does not prevent another from receiving its questions.
    pub fn tick(&mut self) -> Result<usize> {
        if self.delivered.is_none() {
            self.delivered = Some(self.checkpoint().load_or_default()?);
        }
        // If a previous send succeeded but saving failed, persist its in-memory record before
        // trying another send. This also avoids repeats on every tick while the disk is full.
        self.flush()?;
        let mut sent = 0;
        for connection in self.connections.list()? {
            if !connection.manifest.linked || connection.manifest.paused {
                continue;
            }
            let Target::Agent { agent } = &connection.manifest.target else {
                continue;
            };
            for (thread, run_id) in &connection.manifest.threads {
                let Some(ask) = self.agents.pending_question(agent, run_id) else {
                    continue;
                };
                let key = serde_json::json!([connection.id, thread, agent, run_id]).to_string();
                if self.delivered.as_ref().and_then(|d| d.get(&key)) == Some(&ask.id) {
                    continue;
                }
                match self.send(&connection, thread, &ask.headline()) {
                    Ok(true) => {
                        self.delivered
                            .get_or_insert_with(Deliveries::new)
                            .insert(key, ask.id);
                        self.dirty = true;
                        self.flush()?;
                        sent += 1;
                    }
                    Ok(false) => {}
                    Err(error) => warn!(
                        connection = %connection.id,
                        run = %run_id,
                        error = %error,
                        "couldn't forward a channel question; will retry"
                    ),
                }
            }
        }
        Ok(sent)
    }

    fn send(&self, connection: &Connection, thread: &str, question: &str) -> Result<bool> {
        let Some(token) = token::load(&self.secrets, &connection.manifest.provider)? else {
            return Ok(false);
        };
        RouterApi::new(&self.router_url).send(&token, &connection.id, Some(thread), question)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_agents::store::{Answer, AnsweredBy, AskRequest, Question, SessionStore};
    use std::io::{BufRead, BufReader, Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    struct Fixture {
        config: adi_config::Config,
        connections: Connections,
        sessions: SessionStore,
        connection: String,
        run_id: String,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "adi-channels-questions-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&root);
            let config = adi_config::Config::with_root(root);
            let connections = Connections::with_config(config.clone());
            let sessions = SessionStore::new(config.module("sessions").dir());
            let run_id = sessions
                .create("solver", adi_agents::Backend::HarnessAdi, "/tmp", "go")
                .unwrap()
                .id;
            let connection = connections
                .create(
                    "slack",
                    Target::Agent {
                        agent: "solver".into(),
                    },
                )
                .unwrap()
                .id;
            connections
                .mark_linked(&connection, "workspace", "owner")
                .unwrap();
            connections.bind_thread(&connection, "C1", &run_id).unwrap();
            token::save(&Secrets::with_config(config.clone()), "slack", "test-token").unwrap();
            Self {
                config,
                connections,
                sessions,
                connection,
                run_id,
            }
        }

        fn ask(&self, text: &str) {
            self.sessions
                .ask(
                    "solver",
                    &self.run_id,
                    &AskRequest {
                        questions: vec![Question {
                            header: String::new(),
                            question: text.into(),
                            options: Vec::new(),
                            multi_select: false,
                        }],
                        ..AskRequest::default()
                    },
                )
                .unwrap();
        }

        fn worker(&self, router_url: &str) -> QuestionForwarder {
            QuestionForwarder::new(
                self.connections.clone(),
                Agents::with_config(self.config.clone()),
                Secrets::with_config(self.config.clone()),
                router_url.to_string(),
            )
        }
    }

    /// Serve a fixed sequence, then exit. Tests never use real router credentials or services.
    fn router(statuses: Vec<u16>) -> (String, Receiver<serde_json::Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with("POST /send "));
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                tx.send(serde_json::from_slice(&body).unwrap()).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), rx)
    }

    #[test]
    fn forwards_once_across_ticks_and_restart_then_forwards_the_next_question() {
        let fixture = Fixture::new("restart");
        fixture.ask("which backend?");
        let (url, received) = router(vec![200, 200]);
        let mut first = fixture.worker(&url);
        assert_eq!(first.tick().unwrap(), 1);
        assert_eq!(first.tick().unwrap(), 0);
        let mut restarted = fixture.worker(&url);
        assert_eq!(restarted.tick().unwrap(), 0);
        let body = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(body["text"], "which backend?");
        assert_eq!(body["thread"], "C1");
        assert!(
            body.get("status").is_none(),
            "a question does not clear thinking"
        );
        fixture
            .sessions
            .resolve_question(
                "solver",
                &fixture.run_id,
                None,
                &Answer {
                    at: 1,
                    by: AnsweredBy::Human,
                    replies: vec!["Rust".into()],
                },
            )
            .unwrap();
        fixture.ask("which database?");
        assert_eq!(restarted.tick().unwrap(), 1);
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()["text"],
            "which database?"
        );
        assert_eq!(
            fixture.connections.list().unwrap().len(),
            1,
            "checkpoint is not a manifest"
        );
    }

    #[test]
    fn failed_send_is_retried_without_marking_the_question_delivered() {
        let fixture = Fixture::new("retry");
        fixture.ask("ship it?");
        let (url, received) = router(vec![503, 200]);
        let mut worker = fixture.worker(&url);
        assert_eq!(worker.tick().unwrap(), 0);
        assert_eq!(worker.tick().unwrap(), 1);
        assert_eq!(worker.tick().unwrap(), 0);
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()["text"],
            "ship it?"
        );
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()["text"],
            "ship it?"
        );
    }

    #[test]
    fn paused_questions_wait_for_resume_and_unbound_questions_are_ignored() {
        let fixture = Fixture::new("paused");
        fixture.ask("ship it?");
        fixture
            .connections
            .set_paused(&fixture.connection, true)
            .unwrap();
        let unbound = fixture
            .sessions
            .create("solver", adi_agents::Backend::HarnessAdi, "/tmp", "go")
            .unwrap();
        fixture
            .sessions
            .ask(
                "solver",
                &unbound.id,
                &AskRequest {
                    questions: vec![Question {
                        header: String::new(),
                        question: "not for this channel".into(),
                        options: Vec::new(),
                        multi_select: false,
                    }],
                    ..AskRequest::default()
                },
            )
            .unwrap();
        let (url, received) = router(vec![200]);
        let mut worker = fixture.worker(&url);
        assert_eq!(worker.tick().unwrap(), 0);
        fixture
            .connections
            .set_paused(&fixture.connection, false)
            .unwrap();
        assert_eq!(worker.tick().unwrap(), 1);
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2)).unwrap()["text"],
            "ship it?"
        );
        assert_eq!(worker.tick().unwrap(), 0);
    }
}
