use super::response::{Response, error, ok_json};
use adi_events::{
    Events,
    diagnostics::{Diagnostic, Query},
};

/// Shared local process log. A cursor retrieves older records without changing anyone's view.
pub fn diagnostics(store: &Events, body: &[u8]) -> Response {
    let query: Query = if body.is_empty() {
        Query::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(query) => query,
            Err(_) => return error(400, "Invalid log query"),
        }
    };
    match store.diagnostics(&query) {
        Ok(records) => ok_json(&serde_json::json!({ "records": records })),
        Err(e) => error(500, &e.to_string()),
    }
}

/// Any local process can submit structured context without owning a database connection.
pub fn report_diagnostic(store: &Events, body: &[u8]) -> Response {
    if body.len() > 64 * 1024 {
        return error(413, "Log records must be at most 64 KiB");
    }
    let record: Diagnostic = match serde_json::from_slice(body) {
        Ok(record) => record,
        Err(_) => {
            return error(
                400,
                "Expected level, source, area, message and optional context/event_key",
            );
        }
    };
    if record.source.trim().is_empty()
        || record.area.trim().is_empty()
        || record.message.trim().is_empty()
    {
        return error(400, "Log source, area and message must not be empty");
    }
    match store.report(&record) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err(adi_events::Error::InvalidDiagnostic(message)) => error(400, &message),
        Err(e) => error(500, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_reports_are_retrievable_and_invalid_records_are_rejected() {
        let dir = std::env::temp_dir().join(format!("adi-diagnostic-api-{}", std::process::id()));
        let store = Events::with_config(adi_config::Config::with_root(&dir));
        assert_eq!(
            report_diagnostic(
                &store,
                br#"{"source":"worker","area":"backup","message":"disk full","level":"error"}"#
            )
            .status,
            200
        );
        let response = diagnostics(&store, br#"{"source":"worker"}"#);
        assert_eq!(response.status, 200);
        assert!(response.body.contains("disk full"));
        assert_eq!(
            report_diagnostic(&store, br#"{"source":"","area":"x","message":"x"}"#).status,
            400
        );
        assert_eq!(diagnostics(&store, b"not json").status, 400);
        let _ = std::fs::remove_dir_all(dir);
    }
}
