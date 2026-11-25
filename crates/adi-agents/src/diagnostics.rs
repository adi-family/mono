//! Import engine diagnostics when a live or completed turn is folded. Keys make repeated reads
//! and concurrent readers safe; historical logs are imported when they are next opened too.
use crate::runner::Session;
use std::io::{BufRead, BufReader};

pub(crate) fn capture(config: &adi_config::Config, session: &crate::store::SessionRef<'_>) {
    let Ok(file) = std::fs::File::open(session.log_path()) else {
        return;
    };
    let store = adi_events::Events::with_config(config.clone());
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            break;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        for mut diagnostic in crate::backends::claude_stream::diagnostics(&event) {
            diagnostic.context["agent"] = session.agent().into();
            diagnostic.context["session"] = session.id().into();
            diagnostic.context["log_path"] = session.log_path().to_string_lossy().as_ref().into();
            if diagnostic.event_key.is_none() {
                diagnostic.event_key = Some(format!(
                    "engine:{}:{}:{index}:{}",
                    session.agent(),
                    session.id(),
                    diagnostic.message
                ));
            }
            if let Err(error) = store.report(&diagnostic) {
                adi_events::diagnostics::complain(&format!(
                    "could not store agent diagnostic: {error}"
                ));
                return;
            }
        }
    }
}
