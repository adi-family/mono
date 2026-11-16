//! Local, durable diagnostics shared by processes. Separate from the event bus: reading an error
//! never acknowledges a workflow event. Records expire after 30 days; at most 100,000 are kept.
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing_subscriber::{Layer, layer::Context};

use crate::{Events, Result};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS diagnostics (
 id INTEGER PRIMARY KEY AUTOINCREMENT, recorded_at INTEGER NOT NULL,
 level TEXT NOT NULL, source TEXT NOT NULL, area TEXT NOT NULL, message TEXT NOT NULL,
 context TEXT NOT NULL, event_key TEXT UNIQUE);
 CREATE INDEX IF NOT EXISTS diagnostics_source ON diagnostics(source, id);
 CREATE INDEX IF NOT EXISTS diagnostics_time ON diagnostics(recorded_at);";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    #[default]
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    #[serde(default)]
    pub level: Level,
    /// Process or producer name, e.g. adi-app, browser, backup-job.
    pub source: String,
    pub area: String,
    pub message: String,
    #[serde(default)]
    pub context: Value,
    /// Stable identity for replayed events. A repeated write is a no-op, including across processes.
    #[serde(default)]
    pub event_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: u64,
    pub recorded_at: u64,
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Default, Deserialize)]
pub struct Query {
    pub before: Option<u64>,
    pub limit: Option<usize>,
    pub source: Option<String>,
    pub level: Option<Level>,
    pub search: Option<String>,
}

impl Events {
    /// Append a diagnostic. The same event key can safely be submitted more than once.
    pub fn report(&self, diagnostic: &Diagnostic) -> Result<()> {
        if diagnostic.source.trim().is_empty()
            || diagnostic.area.trim().is_empty()
            || diagnostic.message.trim().is_empty()
        {
            return Err(crate::Error::InvalidDiagnostic(
                "source, area and message must not be empty".into(),
            ));
        }
        if serde_json::to_vec(diagnostic)
            .map_err(crate::Error::Encode)?
            .len()
            > 64 * 1024
        {
            return Err(crate::Error::InvalidDiagnostic(
                "records must be at most 64 KiB".into(),
            ));
        }
        let conn = self.connect()?;
        conn.execute_batch(SCHEMA)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as u64;
        let context = serde_json::to_string(&diagnostic.context).map_err(crate::Error::Encode)?;
        conn.execute("INSERT OR IGNORE INTO diagnostics
            (recorded_at, level, source, area, message, context, event_key) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![now, diagnostic.level.as_str(), diagnostic.source, diagnostic.area,
                diagnostic.message, context, diagnostic.event_key])?;
        // Retention runs on writes as well as on daemon sweeps, so standalone producers stay bounded.
        conn.execute(
            "DELETE FROM diagnostics WHERE recorded_at < ?1 OR id <=
            (SELECT id FROM diagnostics ORDER BY id DESC LIMIT 1 OFFSET 100000)",
            [now.saturating_sub(30 * 24 * 60 * 60 * 1000)],
        )?;
        Ok(())
    }

    /// Newest first, with a stable id cursor. Search is literal, never SQL or a wildcard expression.
    pub fn diagnostics(&self, query: &Query) -> Result<Vec<Record>> {
        let conn = self.connect()?;
        conn.execute_batch(SCHEMA)?;
        let mut stmt = conn.prepare("SELECT id,recorded_at,level,source,area,message,context,event_key
            FROM diagnostics WHERE (?1 IS NULL OR id < ?1)
            AND (?2 IS NULL OR source = ?2) AND (?3 IS NULL OR level = ?3)
            AND (?4 IS NULL OR instr(lower(source || ' ' || message || ' ' || area || ' ' || context), lower(?4)) > 0)
            ORDER BY id DESC LIMIT ?5")?;
        let rows = stmt.query_map(
            params![
                query.before,
                query.source,
                query.level.map(Level::as_str),
                query.search,
                query.limit.unwrap_or(100).clamp(1, 500)
            ],
            |row| {
                let level: String = row.get(2)?;
                let context: String = row.get(6)?;
                Ok(Record {
                    id: row.get(0)?,
                    recorded_at: row.get(1)?,
                    diagnostic: Diagnostic {
                        level: match level.as_str() {
                            "info" => Level::Info,
                            "warning" => Level::Warning,
                            _ => Level::Error,
                        },
                        source: row.get(3)?,
                        area: row.get(4)?,
                        message: row.get(5)?,
                        context: serde_json::from_str(&context).unwrap_or(Value::Null),
                        event_key: row.get(7)?,
                    },
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

/// Add to a tracing registry to persist warnings and errors from a service and its libraries.
/// Formatting still goes to the normal process log. Store failures go to stderr, never back
/// through tracing (which would recurse). No tracing output is ever written to MCP stdout.
#[derive(Debug)]
pub struct DiagnosticLayer {
    store: Events,
    source: String,
}

impl DiagnosticLayer {
    pub fn new(store: Events, source: impl Into<String>) -> Self {
        Self {
            store,
            source: source.into(),
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for DiagnosticLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing::Level::ERROR => Level::Error,
            tracing::Level::WARN => Level::Warning,
            _ => return,
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let diagnostic = Diagnostic {
            level,
            source: self.source.clone(),
            area: metadata.target().into(),
            message: if fields.message.is_empty() {
                metadata.name().into()
            } else {
                fields.message.chars().take(16000).collect()
            },
            context: json!({"error": fields.error.chars().take(16000).collect::<String>(), "pid": std::process::id(),
                "file": metadata.file(), "line": metadata.line()}),
            event_key: None,
        };
        if let Err(error) = self.store.report(&diagnostic) {
            eprintln!("diagnostic storage failed: {error}");
        }
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    error: String,
}
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "error" => self.error = format!("{value:?}"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_writers_replay_filter_and_page() {
        let dir = std::env::temp_dir().join(format!("adi-diagnostics-{}", std::process::id()));
        let config = adi_config::Config::with_root(&dir);
        let a = Events::with_config(config.clone());
        let b = Events::with_config(config);
        let mut error = Diagnostic {
            level: Level::Error,
            source: "worker".into(),
            area: "MCP".into(),
            message: "adi failed to connect".into(),
            context: json!({"session":"s1"}),
            event_key: Some("once".into()),
        };
        a.report(&error).unwrap();
        b.report(&error).unwrap();
        error.source = "browser".into();
        error.event_key = Some("twice".into());
        b.report(&error).unwrap();
        let first = a
            .diagnostics(&Query {
                limit: Some(1),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(first[0].diagnostic.source, "browser");
        let rest = b
            .diagnostics(&Query {
                before: Some(first[0].id),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].diagnostic.source, "worker");
        assert_eq!(
            a.diagnostics(&Query {
                source: Some("worker".into()),
                search: Some("s1".into()),
                ..Query::default()
            })
            .unwrap()
            .len(),
            1
        );
        assert!(
            a.diagnostics(&Query {
                search: Some("%".into()),
                ..Query::default()
            })
            .unwrap()
            .is_empty()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn warnings_are_persisted_by_tracing_and_info_is_not() {
        use tracing_subscriber::prelude::*;
        let dir =
            std::env::temp_dir().join(format!("adi-diagnostics-tracing-{}", std::process::id()));
        let store = Events::with_config(adi_config::Config::with_root(&dir));
        let subscriber = tracing_subscriber::registry()
            .with(DiagnosticLayer::new(store.clone(), "test-service"));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("ordinary progress");
            tracing::warn!(error = "connection refused", "upstream unavailable");
        });
        let rows = store.diagnostics(&Query::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].diagnostic.level, Level::Warning);
        assert!(rows[0].diagnostic.message.contains("upstream unavailable"));
        assert!(
            rows[0].diagnostic.context["error"]
                .as_str()
                .unwrap()
                .contains("connection refused")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_producers_and_retention() {
        let dir =
            std::env::temp_dir().join(format!("adi-diagnostics-concurrent-{}", std::process::id()));
        let store = Events::with_config(adi_config::Config::with_root(&dir));
        let workers: Vec<_> = (0..4)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for n in 0..10 {
                        store
                            .report(&Diagnostic {
                                level: Level::Error,
                                source: format!("worker-{i}"),
                                area: "test".into(),
                                message: format!("failure {n}"),
                                context: Value::Null,
                                event_key: None,
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(store.diagnostics(&Query::default()).unwrap().len(), 40);
        store
            .connect()
            .unwrap()
            .execute("UPDATE diagnostics SET recorded_at = 1", [])
            .unwrap();
        store
            .report(&Diagnostic {
                level: Level::Info,
                source: "test".into(),
                area: "test".into(),
                message: "new".into(),
                context: Value::Null,
                event_key: None,
            })
            .unwrap();
        assert_eq!(store.diagnostics(&Query::default()).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
