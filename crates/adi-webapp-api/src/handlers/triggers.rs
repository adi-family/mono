use std::collections::BTreeMap;

use adi_triggers::Error as TriggerStoreError;
use adi_triggers::RunState;
use adi_triggers::Supervisor;
use adi_triggers::TriggerManifest;
use adi_triggers::Triggers;

use crate::types::{
    HookAck, SaveTrigger, TriggerDto, TriggerFireResult, TriggerKindOption, TriggerLog,
    TriggerPreset, TriggerPresetField, TriggerRef, TriggerRuntimeOption, TriggersState,
};

use super::response::{Response, clean, error, ok_json};

/// Trim dynamic backend parameters and drop empty or unsafe keys. Which keys are *meaningful*
/// is the code block's business (its preset declares them, and each reaches it as `ADI_<KEY>`),
/// so nothing here filters by kind — only by what can safely become an environment variable.
fn clean_extra(extra: BTreeMap<String, String>) -> BTreeMap<String, String> {
    extra
        .into_iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty() && safe_extra_key(k))
        .collect()
}

fn safe_extra_key(key: &str) -> bool {
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// `GET /api/triggers` — every registered trigger plus the editor's vocabulary. Each mutation
/// endpoint below returns a fresh [`TriggersState`], so the client refreshes from one round-trip.
#[must_use]
pub fn triggers(store: &Triggers) -> Response {
    match triggers_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => Response::from(&e),
    }
}

/// The full [`TriggersState`]: the stored definitions decorated with their last-fired time and
/// live run status, plus the server-owned kinds, runtimes, and presets.
fn triggers_state(store: &Triggers) -> Result<TriggersState, TriggerStoreError> {
    Ok(TriggersState {
        triggers: store
            .list()?
            .into_iter()
            .map(|t| trigger_dto(store, t))
            .collect(),
        kinds: trigger_kinds(),
        runtimes: trigger_runtimes(),
        presets: trigger_presets(),
    })
}

/// `POST /api/triggers/save` — create or update a trigger definition (an upsert keyed by
/// `name`), then report the fresh list. `name` and `kind` are required.
///
/// Saving is also how a background trigger is started and stopped: the supervisor runs exactly
/// the enabled ones, so it is poked here to pick the change up now rather than at its next tick.
#[must_use]
pub fn save_trigger(store: &Triggers, supervisor: &Supervisor, body: &[u8]) -> Response {
    let Some(req) = parse_save_trigger(body) else {
        return bad_save_trigger();
    };
    let name = req.name.trim().to_string();
    let manifest = TriggerManifest {
        // The store normalizes both onto the kinds/runtimes this build understands.
        kind: req.kind.trim().to_string(),
        runtime: req.runtime.trim().to_string(),
        code: req.code,
        preset: clean(req.preset),
        description: req.description.trim().to_string(),
        enabled: req.enabled,
        project: clean(req.project),
        extra: clean_extra(req.extra),
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => {
            supervisor.poke();
            triggers(store)
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/triggers/delete` — delete a trigger definition, then report the fresh list. A
/// deleted background trigger's process is stopped by the supervisor on its next reconcile.
#[must_use]
pub fn delete_trigger(store: &Triggers, supervisor: &Supervisor, body: &[u8]) -> Response {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => {
            supervisor.poke();
            triggers(store)
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/triggers/restart` — replace a supervised background trigger's process with a
/// fresh one, without changing its definition. A no-op for a trigger that isn't running: the
/// supervisor starts whatever should be up anyway.
#[must_use]
pub fn restart_trigger(store: &Triggers, supervisor: &Supervisor, body: &[u8]) -> Response {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    match store.get(name) {
        Ok(Some(trigger)) => {
            supervisor.request_restart(&trigger.name);
            match triggers_state(store) {
                Ok(state) => ok_json(&TriggerFireResult {
                    message: format!("Restarting “{}”.", trigger.name),
                    state,
                }),
                Err(e) => Response::from(&e),
            }
        }
        Ok(None) => Response::from(&TriggerStoreError::NotFound(name.to_string())),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/triggers/fire` — run a trigger's code block once, by hand. An explicit user
/// action, so it works even on a disabled trigger — only the *external* sources are gated by
/// `enabled`. Replies with the spawned pid plus fresh state.
#[must_use]
pub fn fire_trigger(store: &Triggers, body: &[u8]) -> Response {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    let firing = match store.fire(name, None) {
        Ok(firing) => firing,
        Err(e) => return Response::from(&e),
    };
    match triggers_state(store) {
        Ok(state) => ok_json(&TriggerFireResult {
            message: format!("Fired “{name}” (pid {}).", firing.pid),
            state,
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/triggers/log` — the tail of a trigger's most recent run log. A registered
/// trigger that never ran answers `fired: false` (200, not an error); only an unknown name
/// is a 404.
#[must_use]
pub fn trigger_log(store: &Triggers, body: &[u8]) -> Response {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    match store.get(name) {
        Ok(Some(trigger)) => {
            let output = store.read_log(&trigger.name);
            ok_json(&TriggerLog {
                fired: output.is_some(),
                output: output.unwrap_or_default(),
                fired_at: store.last_fired(&trigger.name),
                name: trigger.name,
            })
        }
        Ok(None) => Response::from(&TriggerStoreError::NotFound(name.to_string())),
        Err(e) => Response::from(&e),
    }
}

/// `POST|GET /api/hooks/<name>` — the public webhook endpoint: launch the named trigger with the
/// request body as its payload. Only an **enabled** trigger of the `webhook` kind launches; when
/// its `secret` extra is set, the caller must match it with a `?secret=` query parameter.
/// An unknown name and a background trigger answer the same 404, so the endpoint doesn't
/// reveal which internal names exist.
#[must_use]
pub fn hook_trigger(store: &Triggers, name: &str, query: &str, payload: &[u8]) -> Response {
    let trigger = match store.get(name) {
        Ok(Some(t)) => t,
        // An unregistered and an unsafely-named hook answer identically, revealing nothing.
        Ok(None) | Err(TriggerStoreError::InvalidName(_)) => {
            return error(404, &format!("no such hook: {name}"));
        }
        Err(e) => return Response::from(&e),
    };
    if trigger.manifest.kind != adi_triggers::KIND_WEBHOOK {
        return error(404, &format!("no such hook: {name}"));
    }
    if !trigger.manifest.enabled {
        return error(403, &format!("hook {name} is disabled"));
    }
    if let Some(secret) = trigger.manifest.extra.get("secret").filter(|s| !s.is_empty())
        && query_param(query, "secret") != Some(secret.as_str())
    {
        return error(403, "bad or missing secret");
    }
    match store.fire(&trigger.name, Some(payload)) {
        Ok(_) => ok_json(&HookAck {
            ok: true,
            trigger: trigger.name,
        }),
        Err(e) => Response::from(&e),
    }
}

/// The value of `key` in a raw query string (`a=1&b=2`), undecoded. Webhook secrets are plain
/// tokens (letters/digits/dashes), so URL-decoding is deliberately skipped.
fn query_param<'q>(query: &'q str, key: &str) -> Option<&'q str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// The two trigger kinds — a kind answers only "how does this launch?". What a trigger *does*
/// comes from its code block, prefilled by a [preset](trigger_presets).
fn trigger_kinds() -> Vec<TriggerKindOption> {
    vec![
        TriggerKindOption {
            id: adi_triggers::KIND_WEBHOOK.into(),
            label: "Webhook".into(),
            hint: "runs on a call to /api/hooks/<name>".into(),
        },
        TriggerKindOption {
            id: adi_triggers::KIND_BACKGROUND.into(),
            label: "Background".into(),
            hint: "runs continuously; auto-restarts".into(),
        },
    ]
}

/// The runtimes a code block can be written in.
fn trigger_runtimes() -> Vec<TriggerRuntimeOption> {
    vec![
        TriggerRuntimeOption {
            id: adi_triggers::RUNTIME_SH.into(),
            label: "Shell".into(),
            hint: "run with sh -c".into(),
        },
        TriggerRuntimeOption {
            id: adi_triggers::RUNTIME_TS.into(),
            label: "TypeScript".into(),
            hint: "run with bun".into(),
        },
    ]
}

/// The preset catalog, straight from the trigger library so the CLI and the UI offer the same
/// starting points.
fn trigger_presets() -> Vec<TriggerPreset> {
    adi_triggers::presets::all()
        .iter()
        .map(|p| TriggerPreset {
            id: p.id.into(),
            label: p.label.into(),
            description: p.description.into(),
            kind: p.kind.into(),
            runtime: p.runtime.into(),
            code: p.code.into(),
            fields: p
                .fields
                .iter()
                .map(|f| TriggerPresetField {
                    key: f.key.into(),
                    label: f.label.into(),
                    hint: f.hint.into(),
                    default: f.default.into(),
                })
                .collect(),
        })
        .collect()
}

/// Flatten a stored trigger into its wire [`TriggerDto`], decorated with its last-run time and
/// — for a background trigger — the live process the supervisor publishes.
fn trigger_dto(store: &Triggers, trigger: adi_triggers::Trigger) -> TriggerDto {
    let last_fired_at = store.last_fired(&trigger.name);
    let status = store.status(&trigger.name);
    let m = trigger.manifest;
    TriggerDto {
        name: trigger.name,
        kind: m.kind,
        runtime: m.runtime,
        code: m.code,
        preset: m.preset,
        description: m.description,
        enabled: m.enabled,
        project: m.project,
        extra: m.extra,
        created_at: m.created_at,
        updated_at: m.updated_at,
        last_fired_at,
        running: status.is_some(),
        pid: status.as_ref().map(|s| s.pid),
        uptime_secs: status.as_ref().and_then(RunState::uptime_secs),
        restarts: status.as_ref().map_or(0, |s| s.restarts),
    }
}

// Map a trigger-store error to an HTTP status: bad name / no code → 400, missing → 404, else 500.
impl From<&TriggerStoreError> for Response {
    fn from(e: &TriggerStoreError) -> Self {
        let status = match e {
            TriggerStoreError::InvalidName(_) | TriggerStoreError::NoCode(_) => 400,
            TriggerStoreError::NotFound(_) => 404,
            TriggerStoreError::Config(_)
            | TriggerStoreError::Io(_)
            | TriggerStoreError::Launch(_) => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_save_trigger(body: &[u8]) -> Option<SaveTrigger> {
    let req: SaveTrigger = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.kind.trim().is_empty()).then_some(req)
}

fn bad_save_trigger() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"kind\": \"…\", … } with a non-empty name and kind",
    )
}

fn parse_trigger_ref(body: &[u8]) -> Option<TriggerRef> {
    let req: TriggerRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_trigger_ref() -> Response {
    error(400, "expected JSON body { \"name\": \"…\" }")
}

// MARK: mesh — peer-to-peer port-forwarding config over the adi-mesh library
