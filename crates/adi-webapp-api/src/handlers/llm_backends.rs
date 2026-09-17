//! `/api/llm/backends/*` — the registry of LLM backends, and the shared record of which ones are
//! spent right now.
//!
//! A **backend** is one complete way to answer a turn: a login, a model, the dials that model runs
//! with, how it says "I am out", and how to ask whether it is back. It is set up once and reused by
//! every agent, and it is never built on another backend — reuse happens in an agent's ordered
//! list, not in an inheritance chain.
//!
//! The endpoints here are deliberately whole-object: a backend is a handful of fields on one page,
//! so a save states all of them. That is unlike `/api/agents/save`, which is omit-to-keep because
//! four different forms save an agent and none of them shows every field.
//!
//! Two things are computed rather than stored and travel with each backend, because they are what
//! the page is actually for: its **credential** (two backends showing the same one share a hold, so
//! the operator can see that moving from one to the other buys nothing), and its **live hold**.

use adi_agents::llm::{
    Holds, LimitRule, LlmBackendManifest, LlmBackends, LlmSettings, Probe, ResolvedChain, catalog,
};
use adi_agents::{Agents, Backend, Error as AgentStoreError};

use crate::types::{
    ContextWarningDto, DanglingRowDto, HoldDto, LimitRuleDto, LlmBackendDto, LlmBackendRef,
    LlmBackendsDto, LlmSettingsDto, ProbeDto, SaveLlmBackend, SaveLlmSettings, TestLlmBackend,
    TestResultDto,
};

use super::response::{FromBody, Response, error, ok_json};

/// `GET /api/llm/backends` — every backend, with its live hold, who uses it, and the warnings the
/// registry can see from here.
#[must_use]
pub fn llm_backends(store: &Agents) -> Response {
    let registry = LlmBackends::with_config(store.config().clone());
    let backends = match registry.list() {
        Ok(backends) => backends,
        Err(e) => return Response::from(&e),
    };
    let agents = store.list().unwrap_or_default();
    let holds = Holds::with_config(store.config());
    let now = now_unix();

    let mut out = Vec::with_capacity(backends.len());
    for backend in &backends {
        let key = adi_agents::llm::HoldKey::new(
            backend.manifest.credential(),
            backend.manifest.model.clone(),
        );
        // A store failure here is not a reason to fail the page: an unreadable hold database means
        // "nothing known to be held", which is what an empty one means too, and the registry is
        // still worth showing.
        let hold = holds
            .blocking(&key)
            .ok()
            .flatten()
            .filter(|hold| hold.blocks_at(now));
        let used_by = agents
            .iter()
            .filter(|agent| {
                agent
                    .manifest
                    .backends
                    .iter()
                    .any(|row| row.backend == backend.id)
            })
            .map(|agent| agent.name.clone())
            .collect();
        out.push(backend_dto(backend, hold.as_ref(), used_by));
    }

    // The two things only a *pair* of records can say: a row naming nothing, and a chain that
    // steps down in context. Both are computed against the same catalog a run would resolve
    // against, so the page and the launch agree on what the list means.
    let known = catalog(backends);
    let mut context_warnings = Vec::new();
    let mut dangling = Vec::new();
    for agent in &agents {
        if agent.manifest.backends.is_empty() {
            continue;
        }
        for row in &agent.manifest.backends {
            if !known.contains_key(&row.backend) {
                dangling.push(DanglingRowDto {
                    agent: agent.name.clone(),
                    backend: row.backend.clone(),
                });
            }
        }
        let Ok(chain) = ResolvedChain::resolve(&agent.manifest.backends, &known, None, None) else {
            continue;
        };
        for (backend, message) in chain.context_shrink_warnings() {
            context_warnings.push(ContextWarningDto {
                agent: agent.name.clone(),
                backend,
                message,
            });
        }
    }

    ok_json(&LlmBackendsDto {
        backends: out,
        settings: settings_dto(LlmSettings::open(store.config())),
        context_warnings,
        dangling,
    })
}

/// `POST /api/llm/backends/save` — create or update one backend, then report the fresh registry.
///
/// Passing `rename_from` renames: the definition is moved first, and **every agent row naming the
/// old id is re-pointed at the new one**. Leaving those behind would turn a rename into a silent
/// un-configuring of every agent that used it — a chain skips a row it cannot resolve, so nothing
/// would fail until a run quietly answered on the wrong model.
#[must_use]
pub fn save_llm_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, SaveLlmBackend);
    let registry = LlmBackends::with_config(store.config().clone());
    let id = req.id.trim().to_string();

    let from = req
        .rename_from
        .as_deref()
        .map(str::trim)
        .filter(|from| !from.is_empty() && *from != id)
        .map(ToString::to_string);
    if let Some(from) = &from {
        match registry.get(from) {
            Ok(Some(_)) => {}
            Ok(None) => return error(404, &format!("no backend named {from}")),
            Err(e) => return Response::from(&e),
        }
    }

    let manifest = manifest_from(req);
    if let Err(e) = registry.save(&id, manifest) {
        return Response::from(&e);
    }
    if let Some(from) = from {
        if let Err(e) = repoint_rows(store, &from, &id) {
            return Response::from(&e);
        }
        if let Err(e) = registry.delete(&from) {
            return Response::from(&e);
        }
    }
    llm_backends(store)
}

/// `POST /api/llm/backends/delete` — remove a backend definition, then report the fresh registry.
///
/// Refused while an agent still names it. A dangling row is not an error a run reports — the chain
/// simply skips it — so deleting out from under one would take an agent's model away and say
/// nothing until it next tried to answer.
#[must_use]
pub fn delete_llm_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, LlmBackendRef);
    let id = req.id.trim();
    let users: Vec<String> = store
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|agent| agent.manifest.backends.iter().any(|row| row.backend == id))
        .map(|agent| agent.name)
        .collect();
    if !users.is_empty() {
        return error(
            409,
            &format!(
                "{id} is still listed by {} — take it off {} first",
                users.join(", "),
                if users.len() == 1 { "that agent" } else { "those agents" }
            ),
        );
    }
    let registry = LlmBackends::with_config(store.config().clone());
    match registry.delete(id) {
        Ok(true) => llm_backends(store),
        Ok(false) => error(404, &format!("no backend named {id}")),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/llm/settings` — save the global switches, then report the fresh registry.
#[must_use]
pub fn save_llm_settings(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, SaveLlmSettings);
    let settings = LlmSettings {
        ask_on_switch: req.ask_on_switch,
        probe_every: req.probe_every,
    };
    match settings.save(&store.config().module(adi_agents::llm::LLM_MODULE)) {
        Ok(()) => llm_backends(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/llm/holds/release` — lift a backend's hold by hand, then report the fresh registry.
///
/// The manual counterpart of the prober: an operator who knows a limit has lifted (they paid, or
/// the provider's own dashboard says so) should not have to wait out a four-hour default guess.
#[must_use]
pub fn release_llm_hold(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, LlmBackendRef);
    let registry = LlmBackends::with_config(store.config().clone());
    let backend = match registry.get(req.id.trim()) {
        Ok(Some(backend)) => backend,
        Ok(None) => return error(404, &format!("no backend named {}", req.id.trim())),
        Err(e) => return Response::from(&e),
    };
    let credential = backend.manifest.credential();
    let holds = Holds::with_config(store.config());
    // Both keys: an `auth` failure was recorded login-wide, and releasing only the model-scoped one
    // would leave the backend just as blocked while the page showed it clear.
    for key in [
        adi_agents::llm::HoldKey::new(credential.clone(), backend.manifest.model.clone()),
        adi_agents::llm::HoldKey::login(credential),
    ] {
        if let Err(e) = holds.release(&key) {
            return Response::from(&e);
        }
    }
    llm_backends(store)
}

/// `POST /api/llm/backends/test` — a real, billed request through this backend, right now. Either
/// a saved backend's `id`, tested as it stands in the store, or `draft`: the form as currently
/// edited, tested whether or not it has ever been saved — the whole reason
/// [`adi_agents::llm::test_manifest`] takes a manifest rather than an id. Writes nothing: no hold
/// touched, released or created, whatever the verdict.
///
/// Always a `200` with the verdict inside — a failed test is a successful answer to "does this
/// work", not a broken request.
#[must_use]
pub fn test_llm_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, TestLlmBackend);
    let manifest = if let Some(draft) = req.draft {
        manifest_from(draft)
    } else {
        let id = req.id.trim();
        match LlmBackends::with_config(store.config().clone()).get(id) {
            Ok(Some(backend)) => backend.manifest,
            Ok(None) => return error(404, &format!("no backend named {id}")),
            Err(e) => return Response::from(&e),
        }
    };
    ok_json(&test_result_dto(&adi_agents::llm::test_manifest(store.config(), &manifest)))
}

// ------------------------------------------------------------------ mapping

/// A backend definition off the wire — shared by [`save_llm_backend`] (which writes it) and
/// [`test_llm_backend`] (which never does).
fn manifest_from(req: SaveLlmBackend) -> LlmBackendManifest {
    LlmBackendManifest {
        label: req.label.trim().to_string(),
        runtime: Backend::from(req.runtime.trim()),
        model: req.model.trim().to_string(),
        context_tokens: req.context_tokens,
        settings: blank_to_none(&req.settings),
        provider: blank_to_none(&req.provider),
        base_url: blank_to_none(&req.base_url),
        api_key_env: blank_to_none(&req.api_key_env),
        params: req.params,
        limit_rules: req.limit_rules.into_iter().map(limit_rule).collect(),
        probe: req.probe.map(probe),
        // The store owns the timestamps; a draft under test never reaches it at all.
        created_at: 0,
        updated_at: 0,
    }
}

fn test_result_dto(result: &adi_agents::llm::TestResult) -> TestResultDto {
    TestResultDto {
        verdict: result.verdict.tag().to_string(),
        message: result.message(),
        elapsed_ms: result.elapsed_ms,
        dimensions: None,
    }
}

/// Re-point every agent row naming `from` at `to`. Written back through the ordinary save, so the
/// rows are validated and `adi.agents.saved` fires — a rename is an edit of those agents.
fn repoint_rows(store: &Agents, from: &str, to: &str) -> Result<(), AgentStoreError> {
    for agent in store.list()? {
        if !agent.manifest.backends.iter().any(|row| row.backend == from) {
            continue;
        }
        let mut manifest = agent.manifest;
        for row in &mut manifest.backends {
            if row.backend == from {
                row.backend = to.to_string();
            }
        }
        store.save(&agent.name, manifest)?;
    }
    Ok(())
}

/// One stored backend as its wire shape, with the live state the page reads.
fn backend_dto(
    backend: &adi_agents::llm::LlmBackend,
    hold: Option<&adi_agents::llm::Hold>,
    used_by: Vec<String>,
) -> LlmBackendDto {
    let m = &backend.manifest;
    LlmBackendDto {
        id: backend.id.clone(),
        label: m.label.clone(),
        runtime: m.runtime.to_string(),
        model: m.model.clone(),
        context_tokens: m.context_tokens,
        settings: m.settings.clone().unwrap_or_default(),
        provider: m.provider.clone().unwrap_or_default(),
        base_url: m.base_url.clone().unwrap_or_default(),
        api_key_env: m.api_key_env.clone().unwrap_or_default(),
        params: m.params.clone(),
        limit_rules: m.limit_rules.iter().map(limit_rule_dto).collect(),
        probe: m.probe.as_ref().map(probe_dto),
        created_at: m.created_at,
        updated_at: m.updated_at,
        credential: m.credential(),
        hold: hold.map(hold_dto),
        used_by,
        replayable: m.is_replayable(),
    }
}

fn limit_rule_dto(rule: &LimitRule) -> LimitRuleDto {
    LimitRuleDto {
        pattern: rule.pattern.clone(),
        class: wire(&rule.class),
        scope: wire(&rule.scope),
        resume: wire(&rule.resume),
        fixed: rule.fixed.clone(),
    }
}

fn limit_rule(dto: LimitRuleDto) -> LimitRule {
    LimitRule {
        pattern: dto.pattern.trim().to_string(),
        class: from_wire(&dto.class),
        scope: from_wire(&dto.scope),
        resume: from_wire(&dto.resume),
        fixed: blank_to_none(dto.fixed.as_deref().unwrap_or_default()),
    }
}

fn probe_dto(probe: &Probe) -> ProbeDto {
    ProbeDto {
        model: probe.model.clone(),
        prompt: probe.prompt.clone(),
    }
}

fn probe(dto: ProbeDto) -> Probe {
    Probe {
        model: dto.model.and_then(|m| blank_to_none(&m)),
        prompt: dto.prompt,
    }
}

fn hold_dto(hold: &adi_agents::llm::Hold) -> HoldDto {
    HoldDto {
        credential: hold.key.credential.clone(),
        model: hold.key.model.clone(),
        class: wire(&hold.class),
        until: hold.until,
        reason: hold.reason.clone(),
        set_by: hold.set_by.clone(),
        attempts: hold.attempts,
        describe: hold.describe(),
    }
}

fn settings_dto(settings: LlmSettings) -> LlmSettingsDto {
    LlmSettingsDto {
        ask_on_switch: settings.ask_on_switch,
        probe_every: settings.probe_every,
    }
}

/// A snake_case enum as the string it serializes to. Round-tripping through serde rather than
/// hand-writing a `match` per enum is what keeps the wire spelling and the TOML spelling one thing:
/// adding a class only has to be done once, on the enum.
fn wire<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(ToString::to_string))
        .unwrap_or_default()
}

/// The inverse of [`wire`]. An unrecognised (or blank) string reads as the enum's default, which is
/// the cautious direction everywhere it is used: an unknown class stops and asks, and an unknown
/// scope holds only the model.
fn from_wire<T: serde::de::DeserializeOwned + Default>(value: &str) -> T {
    serde_json::from_value(serde_json::Value::String(value.trim().to_string()))
        .unwrap_or_else(|_| T::default())
}

fn blank_to_none(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl FromBody for SaveLlmBackend {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\", \"runtime\": \"pty:claude|harness:adi|…\", \"model\"?: \"…\", … } with a non-empty id and runtime";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty() && !self.runtime.trim().is_empty()
    }
}

impl FromBody for LlmBackendRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for SaveLlmSettings {
    const EXPECTED: &'static str =
        "expected JSON body { \"ask_on_switch\": false, \"probe_every\": 300 }";
}

impl FromBody for TestLlmBackend {
    const EXPECTED: &'static str =
        "expected JSON body { \"id\": \"…\" } or { \"draft\": { \"runtime\": \"…\", … } }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
            || self.draft.as_ref().is_some_and(|d| !d.runtime.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_agents::{AgentManifest, llm::AgentBackendEntry};
    use serde_json::{Value, json};

    fn scratch() -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-llm-backends-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(adi_config::Config::with_root(root))
    }

    fn save(store: &Agents, id: &str, model: &str, settings: &str) -> Response {
        save_llm_backend(
            store,
            json!({
                "id": id,
                "runtime": "harness:adi",
                "model": model,
                "settings": settings,
                "context_tokens": 200_000,
                "limit_rules": [{"match": "usage limit", "class": "quota", "resume": "from_message", "fixed": "4h"}],
            })
            .to_string()
            .as_bytes(),
        )
    }

    fn agent_with_rows(store: &Agents, name: &str, rows: &[&str]) {
        let manifest: AgentManifest<std::collections::BTreeMap<String, Value>> = AgentManifest {
            backend: Some(adi_agents::Backend::HarnessAdi),
            backends: rows.iter().map(|id| AgentBackendEntry::new(*id)).collect(),
            ..Default::default()
        };
        store.save(name, manifest).unwrap();
    }

    #[test]
    fn a_backend_round_trips_through_the_registry() {
        let store = scratch();
        let Response { status, body } = save(&store, "anthropic", "claude-opus-5", "~/.claude/a.json");
        assert_eq!(status, 200, "{body}");

        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        let backend = &v["backends"][0];
        assert_eq!(backend["id"], "anthropic");
        assert_eq!(backend["model"], "claude-opus-5");
        assert_eq!(backend["limit_rules"][0]["class"], "quota");
        // The rule's field is `match` on the wire, exactly as it reads in the TOML.
        assert_eq!(backend["limit_rules"][0]["match"], "usage limit");
        // Nothing is held on a fresh store, and the settings come along for the page.
        assert!(backend["hold"].is_null(), "{body}");
        assert_eq!(v["settings"]["ask_on_switch"], false);
    }

    #[test]
    fn two_backends_naming_one_settings_file_report_the_same_credential() {
        // The whole point of showing the credential: an operator can see that falling from one of
        // these to the other buys nothing, because one limit takes both out.
        let store = scratch();
        save(&store, "opus", "claude-opus-5", "~/.claude/a.json");
        save(&store, "sonnet", "claude-sonnet-5", "~/.claude/a.json");
        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        assert_eq!(v["backends"][0]["credential"], v["backends"][1]["credential"]);
    }

    #[test]
    fn the_registry_reports_who_lists_each_backend() {
        let store = scratch();
        save(&store, "anthropic", "claude-opus-5", "~/.claude/a.json");
        save(&store, "glm", "glm-4.6", "~/.claude/glm.json");
        agent_with_rows(&store, "reviewer", &["anthropic", "glm"]);
        agent_with_rows(&store, "scout", &["glm"]);

        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        let by_id = |id: &str| {
            v["backends"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by_id("anthropic")["used_by"], json!(["reviewer"]));
        assert_eq!(by_id("glm")["used_by"], json!(["reviewer", "scout"]));
    }

    #[test]
    fn a_rename_carries_every_row_that_named_it() {
        // A chain silently skips a row it cannot resolve, so a rename that left the rows behind
        // would un-configure an agent without anything failing.
        let store = scratch();
        save(&store, "glm", "glm-4.6", "~/.claude/glm.json");
        agent_with_rows(&store, "reviewer", &["glm"]);

        let Response { status, body } = save_llm_backend(
            &store,
            json!({"id": "zai", "runtime": "harness:adi", "model": "glm-4.6",
                   "settings": "~/.claude/glm.json", "rename_from": "glm"})
            .to_string()
            .as_bytes(),
        );
        assert_eq!(status, 200, "{body}");

        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["backends"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(v["backends"][0]["id"], "zai");
        assert_eq!(v["backends"][0]["used_by"], json!(["reviewer"]));
        assert!(v["dangling"].as_array().unwrap().is_empty(), "{body}");
        assert_eq!(
            store.get("reviewer").unwrap().unwrap().manifest.backends[0].backend,
            "zai"
        );
    }

    #[test]
    fn a_backend_an_agent_still_lists_is_not_deleted() {
        let store = scratch();
        save(&store, "glm", "glm-4.6", "~/.claude/glm.json");
        agent_with_rows(&store, "reviewer", &["glm"]);

        let Response { status, body } = delete_llm_backend(&store, br#"{"id":"glm"}"#);
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("reviewer"), "{body}");
        assert!(store.get("reviewer").unwrap().is_some());

        // Off the agent, it deletes.
        agent_with_rows(&store, "reviewer", &[]);
        assert_eq!(delete_llm_backend(&store, br#"{"id":"glm"}"#).status, 200);
        assert_eq!(delete_llm_backend(&store, br#"{"id":"glm"}"#).status, 404);
    }

    #[test]
    fn a_row_naming_nothing_is_reported_as_dangling() {
        let store = scratch();
        save(&store, "anthropic", "claude-opus-5", "~/.claude/a.json");
        agent_with_rows(&store, "reviewer", &["anthropic", "gone"]);
        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        assert_eq!(v["dangling"][0]["agent"], "reviewer");
        assert_eq!(v["dangling"][0]["backend"], "gone");
    }

    #[test]
    fn a_chain_that_steps_down_in_context_warns_and_does_not_fail() {
        let store = scratch();
        save(&store, "big", "claude-opus-5", "~/.claude/a.json");
        let saved = save_llm_backend(
            &store,
            json!({"id": "small", "runtime": "harness:adi", "model": "glm-4.6",
                   "settings": "~/.claude/glm.json", "context_tokens": 64_000})
            .to_string()
            .as_bytes(),
        );
        assert_eq!(saved.status, 200, "{}", saved.body);
        agent_with_rows(&store, "reviewer", &["big", "small"]);

        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        let warnings = v["context_warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{v}");
        assert_eq!(warnings[0]["agent"], "reviewer");
        assert_eq!(warnings[0]["backend"], "small");
    }

    #[test]
    fn a_live_hold_shows_on_the_backend_it_blocks_and_can_be_released() {
        let store = scratch();
        save(&store, "anthropic", "claude-opus-5", "~/.claude/a.json");
        let holds = Holds::with_config(store.config());
        let now = now_unix();
        holds
            .hold(&adi_agents::llm::Hold {
                key: adi_agents::llm::HoldKey::new(
                    "settings:".to_string()
                        + &adi_config::home().join(".claude/a.json").display().to_string(),
                    "claude-opus-5",
                ),
                class: adi_agents::llm::LimitClass::Quota,
                until: now + 3600,
                reason: "usage limit reached".to_string(),
                set_by: "reviewer/r-1".to_string(),
                attempts: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();

        let v: Value = serde_json::from_str(&llm_backends(&store).body).unwrap();
        assert_eq!(v["backends"][0]["hold"]["class"], "quota");
        assert!(
            v["backends"][0]["hold"]["describe"]
                .as_str()
                .unwrap()
                .starts_with("limited until "),
            "{v}"
        );

        let Response { status, body } = release_llm_hold(&store, br#"{"id":"anthropic"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["backends"][0]["hold"].is_null(), "{body}");
    }

    #[test]
    fn the_global_switches_round_trip() {
        let store = scratch();
        let Response { status, body } = save_llm_settings(
            &store,
            br#"{"ask_on_switch":true,"probe_every":900}"#,
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["settings"]["ask_on_switch"], true);
        assert_eq!(v["settings"]["probe_every"], 900);
    }

    #[test]
    fn a_body_naming_no_runtime_is_a_400() {
        let store = scratch();
        assert_eq!(save_llm_backend(&store, br#"{"id":"x"}"#).status, 400);
        assert_eq!(save_llm_backend(&store, b"{}").status, 400);
        assert_eq!(delete_llm_backend(&store, br#"{"id":""}"#).status, 400);
    }

    /// A test is always a `200` carrying a verdict — this backend's `harness:adi` names no
    /// provider, so it fails locally without ever reaching a network, which is what makes it a
    /// safe fixture for a unit test.
    #[test]
    fn test_by_id_is_a_200_carrying_the_verdict() {
        let store = scratch();
        save(&store, "anthropic", "claude-opus-5", "~/.claude/a.json");
        let Response { status, body } = test_llm_backend(&store, br#"{"id":"anthropic"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["verdict"], "failed", "{body}");
        assert!(!v["message"].as_str().unwrap().is_empty(), "{body}");
    }

    #[test]
    fn test_by_id_of_an_unknown_backend_is_404() {
        let store = scratch();
        assert_eq!(test_llm_backend(&store, br#"{"id":"ghost"}"#).status, 404);
    }

    /// A draft is tested exactly as sent, whether or not it has ever been saved — and nothing
    /// about testing it writes a definition into the store.
    #[test]
    fn a_draft_is_tested_without_ever_being_saved() {
        let store = scratch();
        let Response { status, body } = test_llm_backend(
            &store,
            br#"{"draft":{"id":"","runtime":"harness:adi","model":"claude-opus-5"}}"#,
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["verdict"], "failed", "{body}");
        assert!(LlmBackends::with_config(store.config().clone()).list().unwrap().is_empty());
    }

    #[test]
    fn a_test_body_naming_neither_an_id_nor_a_draft_is_a_400() {
        let store = scratch();
        assert_eq!(test_llm_backend(&store, b"{}").status, 400);
        assert_eq!(
            test_llm_backend(&store, br#"{"draft":{"id":"x","runtime":""}}"#).status,
            400
        );
    }
}
