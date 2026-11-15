//! The `triggers` command group: the trigger-definition subcommand surface and its
//! dispatch over the shared trigger-definition store.

use adi_core::{Adi, RUNTIME_SH, Trigger, TriggerManifest, clean_trigger_on, trigger_presets};
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
        /// `token_env`, `chat_id`, …). Which keys matter is the preset's business, except
        /// `relay` (`public-eu` | `public-us` | `enterprise-eu` | `enterprise-us` |
        /// `custom:<url>`), which this save itself acts on: it registers a fresh
        /// `relay_endpoint` against the named relay before the save lands (a failed registration
        /// fails this save), prints the resulting public `/h/<id>` URL, and best-effort
        /// deregisters the old endpoint on a later save that changes or clears it.
        #[arg(long = "extra")]
        extra: Vec<String>,
        /// For `--kind event`: an event-name pattern to subscribe to, matched segment-by-segment
        /// (`adi.tasks.*` = one segment, `adi.tasks.**` = the tail). Repeatable. Falls back to the
        /// preset's patterns when omitted.
        #[arg(long = "event")]
        events: Vec<String>,
        /// Restrict which projects may fire this trigger (a project id). Repeatable. Omit to run
        /// for every project. For `event`/`webhook` triggers the project is read from the fire's
        /// payload; a payload naming a project outside this list, or none, is skipped.
        #[arg(long = "on")]
        trigger_on: Vec<String>,
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
    /// Print this node's webhook-relay public signing key — paste
    /// into a self-host relay's `authorized_keys` file. One key for the whole node, shared by
    /// every relay-backed trigger; generated on first use.
    RelayKey,
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
                    for pattern in preset.events {
                        println!("    --event {pattern}");
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
            events,
            trigger_on,
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
            let mut manifest = TriggerManifest {
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
                // Explicit --event patterns win; otherwise inherit the preset's (an event preset
                // ships a sensible default like `adi.tasks.*`).
                events: preset_events(preset, events),
                trigger_on: clean_trigger_on(trigger_on),
                created_at: 0,
                updated_at: 0,
            };
            register_relay_if_needed(&store, &name, &mut manifest.extra)?;
            let trigger = store.save(&name, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&trigger);
            } else {
                println!("Saved trigger {}.", trigger.name);
                print_trigger(&store, &trigger);
                if let Some(url) = relay_public_url(&trigger) {
                    println!("  relay URL: {url}");
                }
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
            let firing = store.fire(&name, None, None).map_err(|e| e.to_string())?;
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
        TriggersCommand::RelayKey => {
            let identity = adi_relay::keys::load_or_create().map_err(|e| e.to_string())?;
            println!("{}", identity.public_key_line());
        }
    }
    Ok(())
}

/// Register (or re-register, or deregister) `manifest_extra`'s relay against the relay it names,
/// exactly as `POST /api/triggers/save` does — the same shared implementation
/// (`adi_relay::register`), so a failed registration fails this save with the relay's own
/// reason, never leaving a trigger half-configured with a `relay` nobody answered for
/// (ADI-MONO-227). A no-op, loading nothing, when `name`'s relay hasn't changed.
fn register_relay_if_needed(
    store: &adi_core::Triggers,
    name: &str,
    manifest_extra: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let previous = store.get(name).map_err(|e| e.to_string())?;
    let previous_extra = previous.as_ref().map(|t| &t.manifest.extra);
    if matches!(adi_relay::register::plan(previous_extra, manifest_extra), adi_relay::register::Plan::Unchanged) {
        // `--extra` rebuilds the manifest from scratch every invocation, with no
        // `relay_endpoint` of its own to carry forward — unlike the panel's edit form.
        adi_relay::register::keep_unchanged_endpoint(previous_extra, manifest_extra);
        return Ok(());
    }
    let identity = std::sync::Arc::new(adi_relay::keys::load_or_create().map_err(|e| e.to_string())?);
    // `Cleanup::Await`, not `::Spawn`: this process exits right after, with nothing left around
    // to let a merely-spawned best-effort delete actually run (see `Cleanup`'s own doc comment).
    block_on(adi_relay::register::register_on_save(
        &identity,
        previous_extra,
        manifest_extra,
        adi_relay::register::Cleanup::Await,
    ))
    .map_err(|e| format!("registering against the relay failed: {e}"))
}

/// The public sender-facing URL for a trigger with a live relay registration — `None` for one
/// with no `relay`, or no `relay_endpoint` yet (the relay/CLI validation that `plan` already
/// deferred to never set one).
fn relay_public_url(trigger: &Trigger) -> Option<String> {
    let relay = trigger.manifest.extra.get("relay")?;
    let endpoint_id = trigger.manifest.extra.get("relay_endpoint")?;
    adi_relay::register::public_url(relay, endpoint_id)
}

/// Every `triggers` entry point that touches the relay does one bounded piece of async work and
/// then returns, so it builds its own runtime rather than `main` holding one open for every other
/// subcommand that never needs it — the same shape `adi-cli/src/indexer.rs`'s own `block_on` is.
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
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

/// The event patterns to save: the caller's `--event` values (trimmed, blanks dropped) if any,
/// else the preset's — an event preset ships defaults like `adi.tasks.*`; every other kind ships
/// none.
fn preset_events(
    preset: Option<&adi_core::trigger_presets::Preset>,
    events: Vec<String>,
) -> Vec<String> {
    let events: Vec<String> = events
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    if events.is_empty() {
        return preset
            .map(|p| p.events.iter().map(|e| (*e).to_string()).collect())
            .unwrap_or_default();
    }
    events
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
        "events": preset.events,
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
    if !trigger.manifest.events.is_empty() {
        println!("  events: {}", trigger.manifest.events.join(" · "));
    }
    if !trigger.manifest.trigger_on.is_empty() {
        println!(
            "  runs only for: {}",
            trigger.manifest.trigger_on.join(" · ")
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_store(tag: &str) -> adi_core::Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-cli-triggers-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_core::Triggers::with_config(adi_config::Config::with_root(root))
    }

    fn manifest_with(extra: &[(&str, &str)]) -> TriggerManifest {
        TriggerManifest {
            kind: adi_core::KIND_WEBHOOK.to_string(),
            extra: extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..TriggerManifest::default()
        }
    }

    /// A bare HTTP/1.1 response with a correct `Content-Length` — the same shape
    /// `adi-relay/src/register.rs`'s own test helper is.
    async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
        use tokio::io::AsyncWriteExt as _;
        stream
            .write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
            .await
            .unwrap();
    }

    /// With no `relay` named at all, `register_relay_if_needed` must not even try to load this
    /// node's keypair — the common case (most triggers never touch a relay) pays nothing for it.
    #[test]
    fn a_trigger_with_no_relay_is_left_alone() {
        let store = scratch_store("no-relay");
        let mut extra = std::collections::BTreeMap::new();
        register_relay_if_needed(&store, "hook", &mut extra).expect("no relay named, nothing to do");
        assert!(extra.is_empty());
    }

    /// Spawn a fake relay on its own thread (its own runtime, never the test's — a tokio socket
    /// crossing runtimes is a bug waiting to happen), bound before this returns so the caller
    /// never races the first request against the bind.
    fn fake_relay(respond_with: impl FnOnce(&str) -> (&'static str, String) + Send + 'static) -> std::net::SocketAddr {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            block_on(async move {
                use tokio::io::AsyncReadExt as _;
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap();
                let (status, body) = respond_with(&String::from_utf8_lossy(&buf[..n]));
                respond(&mut stream, status, &body).await;
            });
        });
        rx.recv().expect("the fake relay binds before answering")
    }

    /// A save naming a relay registers against it, and the fresh `relay_endpoint` lands in the
    /// saved trigger — end to end through `run_triggers`'s own `Save` arm, against a fake relay.
    #[test]
    fn saving_with_a_relay_registers_and_the_cli_prints_the_public_url() {
        let store = scratch_store("save-registers");
        let addr = fake_relay(|request| {
            assert!(request.starts_with("POST /v1/endpoints"));
            ("200 OK", "{\"endpoint_id\":\"wh_cli\"}".to_string())
        });

        let mut extra = manifest_with(&[("relay", &format!("custom:http://{addr}"))]).extra;
        register_relay_if_needed(&store, "hook", &mut extra).expect("registration succeeds");
        assert_eq!(extra.get("relay_endpoint"), Some(&"wh_cli".to_string()));

        let trigger = Trigger {
            name: "hook".to_string(),
            manifest: TriggerManifest { extra, ..manifest_with(&[]) },
        };
        assert_eq!(
            relay_public_url(&trigger),
            Some(format!("http://{addr}/h/wh_cli"))
        );
    }

    /// A registration failure must fail the save with the relay's own reason and leave nothing
    /// behind — no `relay_endpoint`, so a retry sees the same "needs to register" plan next time.
    #[test]
    fn a_failed_registration_fails_the_save_with_the_relays_reason() {
        let store = scratch_store("save-fails");
        let addr = fake_relay(|_request| ("500 Internal Server Error", "{\"message\":\"relay is down\"}".to_string()));

        let mut extra = manifest_with(&[("relay", &format!("custom:http://{addr}"))]).extra;
        let error = register_relay_if_needed(&store, "hook", &mut extra).unwrap_err();
        assert_eq!(error, "registering against the relay failed: relay is down");
        assert!(!extra.contains_key("relay_endpoint"));
    }

    /// Re-saving with the same relay must not register again, and must not lose the endpoint
    /// either — the exact bug ADI-MONO-227 fixed. Unlike the webapp panel's edit form (which
    /// loads the existing trigger's `extra` into its own state and round-trips every relay key
    /// on every submit), `adi-mono triggers save` rebuilds its manifest from scratch out of this
    /// invocation's own `--extra` flags, which never include `relay_endpoint` — so the saved
    /// extra here deliberately does *not* carry it, the way a real CLI re-save never would.
    #[test]
    fn resaving_the_same_relay_keeps_the_existing_endpoint_without_a_second_call() {
        let store = scratch_store("resave-unchanged");
        let seeded = manifest_with(&[("relay", "public-eu"), ("relay_endpoint", "wh_kept")]);
        store.save("hook", seeded).expect("seed the previous save");

        let mut extra = manifest_with(&[("relay", "public-eu")]).extra;
        register_relay_if_needed(&store, "hook", &mut extra).expect("unchanged, nothing to call");
        assert_eq!(extra.get("relay_endpoint"), Some(&"wh_kept".to_string()));
    }

    /// `relay_public_url` is `None` without both a `relay` and a live `relay_endpoint`.
    #[test]
    fn relay_public_url_needs_both_relay_and_endpoint() {
        let no_relay = Trigger {
            name: "hook".to_string(),
            manifest: manifest_with(&[]),
        };
        let no_endpoint = Trigger {
            name: "hook".to_string(),
            manifest: manifest_with(&[("relay", "public-eu")]),
        };
        assert_eq!(relay_public_url(&no_relay), None);
        assert_eq!(relay_public_url(&no_endpoint), None);
    }
}
