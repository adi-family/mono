//! The `events` command group: publish platform events onto the shared bus and peek at what is
//! spooled but not yet delivered. Publishing here reaches every enabled
//! [event trigger](adi_core::KIND_EVENT) whose patterns match — the same path task and agent
//! mutations take automatically.

use adi_core::Adi;
use clap::Subcommand;

use crate::format::{clean, print_json};

#[derive(Debug, Subcommand)]
pub(crate) enum EventsCommand {
    /// Publish a platform event (e.g. `adi.tasks.created`). Every enabled event trigger whose
    /// patterns match fires with `--payload` as its `ADI_PAYLOAD`.
    Emit {
        /// The dotted event name.
        name: String,
        /// The event body handed to matching triggers (JSON by convention). Defaults to `{}`.
        #[arg(long)]
        payload: Option<String>,
    },
    /// List events that are spooled but not yet delivered. Normally empty while the app is up —
    /// its dispatcher drains the spool within a second.
    List {
        #[arg(long)]
        json: bool,
    },
    /// List the catalog of platform events you can subscribe to: each name, when it fires, and the
    /// structure of the payload it delivers (a concrete example, or the full JSON Schema with
    /// `--schema`). Pass a `name` to show just that event.
    Types {
        /// Show only this exact event (e.g. `adi.tasks.created`); omitted lists them all.
        name: Option<String>,
        /// Emit machine-readable JSON — each event as `{name, summary, schema, example}`.
        #[arg(long)]
        json: bool,
        /// In text mode, also print each payload's full JSON Schema (not just the example).
        #[arg(long)]
        schema: bool,
    },
}

/// Dispatch an `events` subcommand over the shared event bus.
pub(crate) fn run_events(adi: Adi, command: EventsCommand) -> Result<(), String> {
    let bus = adi.events();
    match command {
        EventsCommand::Emit { name, payload } => {
            let payload = clean(payload).unwrap_or_else(|| "{}".to_string());
            bus.emit(&name, payload).map_err(|e| e.to_string())?;
            println!("Emitted {name}.");
        }
        EventsCommand::List { json } => {
            let spooled = bus.drain().map_err(|e| e.to_string())?;
            if json {
                let items: Vec<_> = spooled
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.record.name,
                            "emitted_at": s.record.emitted_at,
                            "payload": s.record.payload,
                        })
                    })
                    .collect();
                print_json(&items);
            } else if spooled.is_empty() {
                println!("No events spooled (delivered ones are gone).");
            } else {
                for s in &spooled {
                    println!("{} — {} (unix)", s.record.name, s.record.emitted_at);
                    if !s.record.payload.trim().is_empty() {
                        println!("  {}", s.record.payload);
                    }
                }
            }
        }
        EventsCommand::Types {
            name,
            json,
            schema,
        } => {
            let mut types = adi_core::event_catalog();
            if let Some(name) = &name {
                types.retain(|e| e.name == name);
                if types.is_empty() {
                    return Err(format!(
                        "Unknown event `{name}`. Run `adi events types` to list them."
                    ));
                }
            }
            if json {
                let items: Vec<_> = types
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "name": e.name,
                            "summary": e.summary,
                            "schema": e.schema,
                            "example": e.example,
                        })
                    })
                    .collect();
                print_json(&items);
            } else {
                for e in &types {
                    println!("{} — {}", e.name, e.summary);
                    println!("  example: {}", e.example);
                    if schema {
                        let pretty = serde_json::to_string_pretty(&e.schema)
                            .unwrap_or_else(|_| e.schema.to_string());
                        // Indent the schema block so it reads under its event.
                        for line in pretty.lines() {
                            println!("    {line}");
                        }
                    }
                }
                println!("\n{}", adi_core::EVENT_ENVELOPE);
            }
        }
    }
    Ok(())
}
