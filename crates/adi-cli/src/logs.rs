use adi_events::{
    Events,
    diagnostics::{Diagnostic, Level, Query},
};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum LogsCommand {
    /// Newest first; use --before with the last id for the next page.
    List {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        before: Option<u64>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, value_parser = ["error", "warning", "info"])]
        level: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Report a process failure or informational event. Do not include credentials in context.
    Report {
        #[arg(long)]
        source: String,
        #[arg(long)]
        area: String,
        #[arg(long, default_value = "error", value_parser = ["error", "warning", "info"])]
        level: String,
        #[arg(long, default_value = "{}")]
        context: String,
        #[arg(long)]
        event_key: Option<String>,
        message: String,
    },
    /// Help written for a language model rather than for a person. Bare `llm` prints it too.
    Llm(crate::llm_help::LlmHelp),
}

fn level(value: &str) -> Level {
    match value {
        "warning" => Level::Warning,
        "info" => Level::Info,
        _ => Level::Error,
    }
}

pub(crate) fn run(store: Events, command: LogsCommand) -> Result<(), String> {
    match command {
        LogsCommand::Llm(_) => unreachable!("`llm` is answered in main"),
        LogsCommand::List {
            limit,
            before,
            source,
            level: severity,
            search,
            json,
        } => {
            let rows = store
                .diagnostics(&Query {
                    limit: Some(limit),
                    before,
                    source,
                    level: severity.as_deref().map(level),
                    search,
                })
                .map_err(|e| e.to_string())?;
            if json {
                crate::format::print_json(&rows);
            } else {
                for row in rows {
                    println!(
                        "{} {} {} [{} / {}] {}",
                        row.id,
                        row.recorded_at,
                        row.diagnostic.level.as_str(),
                        row.diagnostic.source,
                        row.diagnostic.area,
                        row.diagnostic.message
                    );
                }
            }
        }
        LogsCommand::Report {
            source,
            area,
            level: severity,
            context,
            event_key,
            message,
        } => {
            if source.trim().is_empty() || area.trim().is_empty() || message.trim().is_empty() {
                return Err("source, area and message must not be empty".into());
            }
            let diagnostic = Diagnostic {
                level: level(&severity),
                source,
                area,
                message,
                context: serde_json::from_str(&context).map_err(|e| e.to_string())?,
                event_key,
            };
            store.report(&diagnostic).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Keep command failures visible after the process exits, without writing to protocol stdout.
pub(crate) fn report_error(message: &str) {
    eprintln!("error: {message}");
    let record = Diagnostic {
        level: Level::Error,
        source: "adi-mono".into(),
        area: "CLI".into(),
        message: message.chars().take(16000).collect(),
        context: serde_json::json!({"pid": std::process::id()}),
        event_key: None,
    };
    if let Err(error) = Events::open().report(&record) {
        eprintln!("could not store CLI error: {error}");
    }
}
