use std::collections::BTreeMap;

use adi_triggers::Error as TriggerStoreError;
use adi_triggers::TriggerManifest;
use adi_triggers::Triggers;

use crate::types::{HookAck, SaveTrigger, TriggerDto, TriggerFireResult, TriggerKindOption, TriggerLog, TriggerRef, TriggersState};

use super::response::{error, ok_json, clean};

/// Trim dynamic backend parameters and drop empty or unsafe keys.
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

/// `GET /api/triggers` — every registered trigger plus the selectable kinds. Each mutation
/// endpoint below returns a fresh [`TriggersState`], so the client refreshes from one round-trip.
#[must_use]
pub fn triggers(store: &Triggers) -> (u16, String) {
    match triggers_state(store) {
        Ok(state) => ok_json(&state),
        Err(e) => trigger_error(&e),
    }
}

/// The full [`TriggersState`]: the stored definitions decorated with their last-fired time,
/// plus the server-owned kind options.
fn triggers_state(store: &Triggers) -> Result<TriggersState, TriggerStoreError> {
    Ok(TriggersState {
        triggers: store
            .list()?
            .into_iter()
            .map(|t| trigger_dto(store, t))
            .collect(),
        kinds: trigger_kinds(),
    })
}

/// `POST /api/triggers/save` — create or update a trigger definition (an upsert keyed by
/// `name`), then report the fresh list. `name` and `kind` are required.
#[must_use]
pub fn save_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_save_trigger(body) else {
        return bad_save_trigger();
    };
    let name = req.name.trim().to_string();
    let manifest = TriggerManifest {
        kind: req.kind.trim().to_string(),
        code: req.code,
        description: req.description.trim().to_string(),
        enabled: req.enabled,
        project: clean(req.project),
        extra: clean_extra(req.extra),
        // The store owns the timestamps.
        created_at: 0,
        updated_at: 0,
    };
    match store.save(&name, manifest) {
        Ok(_) => triggers(store),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/delete` — delete a trigger definition, then report the fresh list.
#[must_use]
pub fn delete_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    match store.delete(req.name.trim()) {
        Ok(_) => triggers(store),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/fire` — fire a trigger by hand (no payload). An explicit user action, so
/// it works even on a disabled trigger — only the *external* sources are gated by `enabled`.
/// Replies with the spawned pid plus fresh state.
#[must_use]
pub fn fire_trigger(store: &Triggers, body: &[u8]) -> (u16, String) {
    let Some(req) = parse_trigger_ref(body) else {
        return bad_trigger_ref();
    };
    let name = req.name.trim();
    let firing = match store.fire(name, None) {
        Ok(firing) => firing,
        Err(e) => return trigger_error(&e),
    };
    match triggers_state(store) {
        Ok(state) => ok_json(&TriggerFireResult {
            message: format!("Fired “{name}” (pid {}).", firing.pid),
            state,
        }),
        Err(e) => trigger_error(&e),
    }
}

/// `POST /api/triggers/log` — the tail of a trigger's most recent fire log. A registered
/// trigger that never fired answers `fired: false` (200, not an error); only an unknown name
/// is a 404.
#[must_use]
pub fn trigger_log(store: &Triggers, body: &[u8]) -> (u16, String) {
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
        Ok(None) => trigger_error(&TriggerStoreError::NotFound(name.to_string())),
        Err(e) => trigger_error(&e),
    }
}

/// `POST|GET /api/hooks/<name>` — the public webhook endpoint: fire the named trigger with the
/// request body as its payload. Only an **enabled** trigger of the `webhook` kind fires; when
/// its `secret` extra is set, the caller must match it with a `?secret=` query parameter.
/// An unknown name and a non-webhook trigger answer the same 404, so the endpoint doesn't
/// reveal which internal names exist.
#[must_use]
pub fn hook_trigger(store: &Triggers, name: &str, query: &str, payload: &[u8]) -> (u16, String) {
    let trigger = match store.get(name) {
        Ok(Some(t)) => t,
        // An unregistered and an unsafely-named hook answer identically, revealing nothing.
        Ok(None) | Err(TriggerStoreError::InvalidName(_)) => {
            return error(404, &format!("no such hook: {name}"));
        }
        Err(e) => return trigger_error(&e),
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
        Err(e) => trigger_error(&e),
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

/// The selectable trigger kinds — server-owned so adding one doesn't require a webapp rebuild.
fn trigger_kinds() -> Vec<TriggerKindOption> {
    let kind = |id: &str, label: &str, hint: &str| TriggerKindOption {
        id: id.into(),
        label: label.into(),
        hint: hint.into(),
    };
    vec![
        kind(
            adi_triggers::KIND_WEBHOOK,
            "Webhook",
            "fires on POST/GET to /api/hooks/<name>; optional shared secret",
        ),
        kind(
            adi_triggers::KIND_TELEGRAM,
            "Telegram",
            "bot-update listener is future work — define now, fire manually to test",
        ),
        kind(
            adi_triggers::KIND_CRON,
            "Cron",
            "scheduler runtime is future work — define now, fire manually to test",
        ),
        kind(adi_triggers::KIND_MANUAL, "Manual", "fired only by hand (UI / CLI / API)"),
    ]
}

/// Flatten a stored trigger into its wire [`TriggerDto`], decorated with its last-fired time.
fn trigger_dto(store: &Triggers, trigger: adi_triggers::Trigger) -> TriggerDto {
    let last_fired_at = store.last_fired(&trigger.name);
    let m = trigger.manifest;
    TriggerDto {
        name: trigger.name,
        kind: m.kind,
        code: m.code,
        description: m.description,
        enabled: m.enabled,
        project: m.project,
        extra: m.extra,
        created_at: m.created_at,
        updated_at: m.updated_at,
        last_fired_at,
    }
}

/// Map a trigger-store error to an HTTP status: bad name / no code → 400, missing → 404, else 500.
fn trigger_error(e: &TriggerStoreError) -> (u16, String) {
    let status = match e {
        TriggerStoreError::InvalidName(_) | TriggerStoreError::NoCode(_) => 400,
        TriggerStoreError::NotFound(_) => 404,
        TriggerStoreError::Config(_) | TriggerStoreError::Io(_) | TriggerStoreError::Launch(_) => {
            500
        }
    };
    error(status, &e.to_string())
}

fn parse_save_trigger(body: &[u8]) -> Option<SaveTrigger> {
    let req: SaveTrigger = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.kind.trim().is_empty()).then_some(req)
}

fn bad_save_trigger() -> (u16, String) {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"kind\": \"…\", … } with a non-empty name and kind",
    )
}

fn parse_trigger_ref(body: &[u8]) -> Option<TriggerRef> {
    let req: TriggerRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_trigger_ref() -> (u16, String) {
    error(400, "expected JSON body { \"name\": \"…\" }")
}

// MARK: mesh — peer-to-peer port-forwarding config over the adi-mesh library
