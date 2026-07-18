//! The `triggers` command group: the trigger-definition subcommand surface and its
//! dispatch over the shared trigger-definition store.

use adi_core::{Adi, Trigger, TriggerManifest};
use clap::Subcommand;

use crate::format::{clean, clean_required, parse_extra, print_json};

#[derive(Debug, Subcommand)]
pub(crate) enum TriggersCommand {
    /// List trigger definitions.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one trigger definition.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a trigger definition.
    Save {
        name: String,
        /// The event source that fires it: webhook, telegram, cron, or manual.
        #[arg(long)]
        kind: String,
        /// The shell code block spawned (detached) when the trigger fires.
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// The project to file the trigger under (its id); omit for a global trigger.
        #[arg(long)]
        project: Option<String>,
        /// Save the trigger disabled (its external source won't fire it).
        #[arg(long)]
        disabled: bool,
        /// Repeatable key=value kind-specific setting (`secret`, `schedule`, `token_env`, `chat_id`, …).
        #[arg(long = "extra")]
        extra: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Enable a trigger (its external source may fire it again).
    Enable { name: String },
    /// Disable a trigger (keeps the definition; the external source refuses to fire it).
    Disable { name: String },
    /// Fire a trigger by hand: spawn its code block detached, output to its log.
    Fire { name: String },
    /// Print the tail of a trigger's most recent fire log.
    Log { name: String },
    /// Delete a trigger definition.
    Rm { name: String },
    /// Delete a trigger definition.
    Delete { name: String },
}

/// Dispatch a `triggers` subcommand over the shared trigger-definition store.
pub(crate) fn run_triggers(adi: Adi, command: TriggersCommand) -> Result<(), String> {
    let store = adi.triggers();
    match command {
        TriggersCommand::List { json } => {
            let triggers = store.list().map_err(|e| e.to_string())?;
            if json {
                print_json(&triggers);
            } else if triggers.is_empty() {
                println!("No triggers registered.");
            } else {
                for trigger in &triggers {
                    print_trigger(&store, trigger);
                }
            }
        }
        TriggersCommand::Show { name, json } => {
            let trigger = store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such trigger: {name}"))?;
            if json {
                print_json(&trigger);
            } else {
                print_trigger(&store, &trigger);
                if !trigger.manifest.code.trim().is_empty() {
                    println!("  code: {}", trigger.manifest.code);
                }
            }
        }
        TriggersCommand::Save {
            name,
            kind,
            code,
            description,
            project,
            disabled,
            extra,
            json,
        } => {
            let kind = clean_required("kind", kind)?;
            let manifest = TriggerManifest {
                kind,
                code: code.unwrap_or_default(),
                description: clean(description).unwrap_or_default(),
                enabled: !disabled,
                project: clean(project),
                extra: parse_extra(extra)?,
                created_at: 0,
                updated_at: 0,
            };
            let trigger = store.save(&name, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&trigger);
            } else {
                println!("Saved trigger {}.", trigger.name);
                print_trigger(&store, &trigger);
            }
        }
        TriggersCommand::Enable { name } => {
            set_trigger_enabled(&store, &name, true)?;
            println!("Enabled trigger {name}.");
        }
        TriggersCommand::Disable { name } => {
            set_trigger_enabled(&store, &name, false)?;
            println!("Disabled trigger {name}.");
        }
        TriggersCommand::Fire { name } => {
            let firing = store.fire(&name, None).map_err(|e| e.to_string())?;
            println!("Fired trigger {name} (pid {}).", firing.pid);
            println!("  log: {}", firing.log.display());
        }
        TriggersCommand::Log { name } => {
            store
                .get(&name)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such trigger: {name}"))?;
            match store.read_log(&name) {
                Some(output) => print!("{output}"),
                None => println!("Trigger {name} has never fired."),
            }
        }
        TriggersCommand::Rm { name } | TriggersCommand::Delete { name } => {
            if store.delete(&name).map_err(|e| e.to_string())? {
                println!("Deleted trigger {name}.");
            } else {
                println!("No such trigger: {name}.");
            }
        }
    }
    Ok(())
}

/// Flip a trigger's enabled flag by re-saving its manifest (the store preserves `created_at`).
fn set_trigger_enabled(
    store: &adi_core::Triggers,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let trigger = store
        .get(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no such trigger: {name}"))?;
    let mut manifest = trigger.manifest;
    manifest.enabled = enabled;
    store.save(name, manifest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Print a trigger definition in the compact human CLI format.
fn print_trigger(store: &adi_core::Triggers, trigger: &Trigger) {
    let state = if trigger.manifest.enabled {
        "enabled"
    } else {
        "disabled"
    };
    println!("{} — {} [{state}]", trigger.name, trigger.manifest.kind);
    if !trigger.manifest.description.trim().is_empty() {
        println!("  {}", trigger.manifest.description);
    }
    if let Some(project) = &trigger.manifest.project {
        println!("  project: {project}");
    }
    if !trigger.manifest.extra.is_empty() {
        let extras: Vec<String> = trigger
            .manifest
            .extra
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        println!("  extra: {}", extras.join(" · "));
    }
    if let Some(fired) = store.last_fired(&trigger.name) {
        println!("  last fired: {fired} (unix)");
    }
}
