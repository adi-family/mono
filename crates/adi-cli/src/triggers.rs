//! The `triggers` command group: the trigger-definition subcommand surface and its
//! dispatch over the shared trigger-definition store.

use adi_core::{Adi, RUNTIME_SH, Trigger, TriggerManifest, trigger_presets};
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
    /// List the presets a trigger can be created from.
    Presets {
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a trigger definition.
    Save {
        name: String,
        /// How it launches: `webhook` (an inbound call to /api/hooks/<name>) or `background`
        /// (a long-lived process the app keeps alive while the trigger is enabled).
        #[arg(long)]
        kind: String,
        /// The language of the code block: `sh` (default) or `ts` (run with bun).
        #[arg(long)]
        runtime: Option<String>,
        /// Start from a preset (see `triggers presets`): fills the kind, runtime, and code
        /// block. Anything given explicitly wins over the preset.
        #[arg(long)]
        preset: Option<String>,
        /// The code block launched when the trigger fires.
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
        /// Repeatable key=value setting, reaching the code block as `ADI_<KEY>` (`secret`,
        /// `token_env`, `chat_id`, …). Which keys matter is the preset's business.
        #[arg(long = "extra")]
        extra: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Enable a trigger (its external source may fire it again).
    Enable { name: String },
    /// Disable a trigger (keeps the definition; the external source refuses to fire it).
    Disable { name: String },
    /// Run a trigger's code block once, by hand — output goes to its log.
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
        TriggersCommand::Presets { json } => {
            if json {
                print_json(
                    &trigger_presets::all()
                        .iter()
                        .map(preset_json)
                        .collect::<Vec<_>>(),
                );
            } else {
                for preset in trigger_presets::all() {
                    println!(
                        "{} — {} [{}/{}]",
                        preset.id, preset.label, preset.kind, preset.runtime
                    );
                    println!("  {}", preset.description);
                    for field in preset.fields {
                        println!("    --extra {}=…  {}", field.key, field.hint);
                    }
                }
            }
        }
        TriggersCommand::Save {
            name,
            kind,
            runtime,
            preset,
            code,
            description,
            project,
            disabled,
            extra,
            json,
        } => {
            let kind = clean_required("kind", kind)?;
            // A preset supplies the code block (and the runtime it is written in) unless the
            // caller spelled them out.
            let preset = match clean(preset) {
                Some(id) => Some(
                    trigger_presets::get(&id)
                        .ok_or_else(|| format!("no such preset: {id} (see `triggers presets`)"))?,
                ),
                None => None,
            };
            let manifest = TriggerManifest {
                kind,
                runtime: clean(runtime)
                    .or_else(|| preset.map(|p| p.runtime.to_string()))
                    .unwrap_or_else(|| RUNTIME_SH.to_string()),
                code: code
                    .or_else(|| preset.map(|p| p.code.to_string()))
                    .unwrap_or_default(),
                preset: preset.map(|p| p.id.to_string()),
                description: clean(description)
                    .or_else(|| preset.map(|p| p.description.to_string()))
                    .unwrap_or_default(),
                enabled: !disabled,
                project: clean(project),
                extra: preset_defaults(preset, parse_extra(extra)?),
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

/// A preset's fields filled in: the caller's `--extra` values win, and any the caller didn't
/// give fall back to the preset's default (an empty default is left out for the user to supply).
fn preset_defaults(
    preset: Option<&adi_core::trigger_presets::Preset>,
    mut extra: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    if let Some(preset) = preset {
        for field in preset.fields {
            if !field.default.is_empty() {
                extra
                    .entry(field.key.to_string())
                    .or_insert_with(|| field.default.to_string());
            }
        }
    }
    extra
}

/// A preset as JSON, for `triggers presets --json`.
fn preset_json(preset: &adi_core::trigger_presets::Preset) -> serde_json::Value {
    serde_json::json!({
        "id": preset.id,
        "label": preset.label,
        "description": preset.description,
        "kind": preset.kind,
        "runtime": preset.runtime,
        "code": preset.code,
        "fields": preset.fields.iter().map(|f| serde_json::json!({
            "key": f.key,
            "label": f.label,
            "hint": f.hint,
            "default": f.default,
        })).collect::<Vec<_>>(),
    })
}

/// Print a trigger definition in the compact human CLI format.
fn print_trigger(store: &adi_core::Triggers, trigger: &Trigger) {
    let state = if trigger.manifest.enabled {
        "enabled"
    } else {
        "disabled"
    };
    println!(
        "{} — {}/{} [{state}]",
        trigger.name, trigger.manifest.kind, trigger.manifest.runtime
    );
    // A background trigger's process is supervised by the app, which publishes its state — so
    // this reads as "up" even though the CLI is a different process entirely.
    if let Some(run) = store.status(&trigger.name) {
        let uptime = run.uptime_secs().unwrap_or_default();
        print!("  running: pid {}, up {uptime}s", run.pid);
        if run.restarts > 0 {
            print!(", {} restart(s)", run.restarts);
        }
        println!();
    } else if trigger.manifest.is_background() && trigger.manifest.enabled {
        println!("  running: no (is the app up?)");
    }
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
