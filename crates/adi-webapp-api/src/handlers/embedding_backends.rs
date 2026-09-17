//! `/api/embeddings/backends/*` — the registry of embedding backends, and which consumer resolves
//! through each.
//!
//! A **backend** is one complete way to turn text into a vector: a runtime, a model, the dials it
//! runs with, and the fallbacks it may fail over to — set up once here, mirroring
//! `/api/llm/backends`'s flat shape (never built on another backend). It is narrower on purpose:
//! there is no hold, no prober, and no context-window shape to carry, because there is no chat
//! conversation to fail over *within* — see `docs/embedding-backends.md`'s "why this is narrower
//! than the LLM design."
//!
//! Two things only this registry needs of its own, and both are computed rather than stored: **is
//! it here**, and **whether this binary can actually build it** (false only for `candle` without
//! this build's `candle` cargo feature — see [`adi_embeddings::Runtime::available`]), and **which
//! consumer is presently assigned to it** (`embeddings/settings.toml`'s one row per consumer).
//!
//! Like `/api/knowledge/search`, resolving or building an embedder can load a model, so nothing
//! here runs on an async worker (see `adi-app/src/main.rs`'s own note on that route) — not that
//! anything here actually builds one: this whole surface answers off the manifest and the cheap,
//! local [`adi_embeddings::Runtime::available`] check, never [`adi_embeddings::resolve`].

use adi_agents::Agents;
use adi_embeddings::{
    CONSUMER_FACTS, CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, EmbeddingBackend, EmbeddingBackendManifest,
    EmbeddingBackends, EmbeddingSettings, Runtime,
};

use crate::types::{
    ConsumerAssignmentDto, EmbeddingBackendDto, EmbeddingBackendRef, EmbeddingBackendsDto,
    SaveEmbeddingBackend, SaveEmbeddingSettings, TestEmbeddingBackend, TestResultDto,
};

use super::response::{FromBody, Response, error, ok_json};

/// Every consumer this build knows to ask, in the order the page shows them — not alphabetical:
/// `indexer` embeds code, `knowledge` embeds notes, `facts` embeds sentences, and that is the order
/// an operator reads them in on the page.
const CONSUMERS: [&str; 3] = [CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, CONSUMER_FACTS];

/// `GET /api/embeddings/backends` — every backend, whether this binary can build it, and which
/// consumer is assigned to it right now.
///
/// A store that has never been seeded (`embeddings/settings.toml` does not exist) answers with an
/// empty registry rather than seeding it — the same laziness [`EmbeddingSettings::load`] documents.
/// `adi-mono embeddings seed` (or simply using an embedding consumer once) is what materializes the
/// defaults; a page view is a read and must stay one.
#[must_use]
pub fn embedding_backends(store: &Agents) -> Response {
    let registry = EmbeddingBackends::with_config(store.config().clone());
    let backends = match registry.list() {
        Ok(backends) => backends,
        Err(e) => return Response::from(&e),
    };
    let settings = match EmbeddingSettings::open(store.config()) {
        Ok(settings) => settings,
        Err(e) => return Response::from(&e),
    };
    ok_json(&backends_dto(&backends, &settings))
}

/// `POST /api/embeddings/backends/save` — create or update one backend, then report the fresh
/// registry.
#[must_use]
pub fn save_embedding_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, SaveEmbeddingBackend);
    let runtime = match runtime_from_wire(&req.runtime) {
        Ok(runtime) => runtime,
        Err(resp) => return resp,
    };
    let registry = EmbeddingBackends::with_config(store.config().clone());
    let id = req.id.trim().to_string();
    let manifest = manifest_from(req, runtime);
    if let Err(e) = registry.save(&id, manifest) {
        return Response::from(&e);
    }
    embedding_backends(store)
}

/// `POST /api/embeddings/backends/delete` — remove a backend, then report the fresh registry.
///
/// Refused while a consumer is still assigned to it. Unlike an LLM agent's dangling row (which a
/// chain simply skips over), a consumer whose assignment names nothing cannot resolve *at all* —
/// the next thing that tries to embed with it gets `Error::Unassigned`/`Error::NotFound` instead of
/// a vector, so this refusal is the only warning an operator gets before that happens.
#[must_use]
pub fn delete_embedding_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, EmbeddingBackendRef);
    let id = req.id.trim();
    let settings = match EmbeddingSettings::open(store.config()) {
        Ok(settings) => settings,
        Err(e) => return Response::from(&e),
    };
    let users: Vec<&str> = CONSUMERS
        .into_iter()
        .filter(|consumer| settings.assignments.get(*consumer).map(String::as_str) == Some(id))
        .collect();
    if !users.is_empty() {
        return error(
            409,
            &format!(
                "{id} is still assigned to {} — point {} at another backend first",
                users.join(", "),
                if users.len() == 1 { "it" } else { "them" }
            ),
        );
    }
    let registry = EmbeddingBackends::with_config(store.config().clone());
    match registry.delete(id) {
        Ok(true) => embedding_backends(store),
        Ok(false) => error(404, &format!("no embedding backend named {id}")),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/embeddings/settings` — save the consumer assignments, then report the fresh registry.
///
/// Refused when an assignment names a backend that is not here: worth checking at save time the
/// same way a backend's own fallback list is (`EmbeddingBackends::save`) — a typo caught here never
/// reaches `Error::NotFound` at the next resolve, which is a worse place for an operator to meet it.
#[must_use]
pub fn save_embedding_settings(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, SaveEmbeddingSettings);
    let registry = EmbeddingBackends::with_config(store.config().clone());
    for (consumer, backend) in &req.assignments {
        let backend = backend.trim();
        if backend.is_empty() {
            continue; // clearing a consumer's assignment back out is allowed
        }
        match registry.get(backend) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error(
                    400,
                    &format!("{consumer} cannot be assigned to {backend} — no such backend"),
                );
            }
            Err(e) => return Response::from(&e),
        }
    }
    let settings = EmbeddingSettings {
        assignments: req
            .assignments
            .into_iter()
            .map(|(consumer, backend)| (consumer, backend.trim().to_string()))
            .filter(|(_, backend)| !backend.is_empty())
            .collect(),
    };
    if let Err(e) = settings.save(&store.config().module(adi_embeddings::EMBEDDINGS_MODULE)) {
        return Response::from(&e);
    }
    embedding_backends(store)
}

/// `POST /api/embeddings/backends/test` — embed one short string through this backend, right now.
/// Either a saved backend's `id`, tested as it stands in the store, or `draft`: the form as
/// currently edited, tested whether or not it has ever been saved — the whole reason
/// [`adi_embeddings::test_manifest`] takes a manifest rather than an id.
///
/// Always a `200` with the verdict inside — a failed test is a successful answer to "does this
/// work", not a broken request.
#[must_use]
pub fn test_embedding_backend(store: &Agents, body: &[u8]) -> Response {
    let req = require!(body, TestEmbeddingBackend);
    let manifest = if let Some(draft) = req.draft {
        let runtime = match runtime_from_wire(&draft.runtime) {
            Ok(runtime) => runtime,
            Err(resp) => return resp,
        };
        manifest_from(draft, runtime)
    } else {
        let id = req.id.trim();
        match EmbeddingBackends::with_config(store.config().clone()).get(id) {
            Ok(Some(backend)) => backend.manifest,
            Ok(None) => return error(404, &format!("no embedding backend named {id}")),
            Err(e) => return Response::from(&e),
        }
    };
    ok_json(&test_result_dto(&adi_embeddings::test_manifest(&manifest)))
}

// ------------------------------------------------------------------ mapping

/// A backend definition off the wire — shared by [`save_embedding_backend`] (which writes it) and
/// [`test_embedding_backend`] (which never does). `candle`/`hash` never take a model or width from
/// configuration — the panel and the CLI both leave those fields out of the form entirely for
/// these two runtimes, so whatever arrived here is stale or blank and the fixed pair always wins.
fn manifest_from(req: SaveEmbeddingBackend, runtime: Runtime) -> EmbeddingBackendManifest {
    let (model, dimensions) = match runtime.fixed_model() {
        Some((model, dimensions)) => (model.to_string(), dimensions),
        None => (req.model.trim().to_string(), req.dimensions),
    };
    EmbeddingBackendManifest {
        label: req.label.trim().to_string(),
        runtime,
        model,
        dimensions,
        base_url: blank_to_none(&req.base_url),
        api_key_env: blank_to_none(&req.api_key_env),
        fallbacks: req
            .fallbacks
            .into_iter()
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect(),
        // The store owns the timestamps; a draft under test never reaches it at all.
        created_at: 0,
        updated_at: 0,
    }
}

fn test_result_dto(result: &adi_embeddings::TestResult) -> TestResultDto {
    let dimensions = match &result.verdict {
        adi_embeddings::TestVerdict::Answered { dimensions } => Some(*dimensions),
        adi_embeddings::TestVerdict::Failed { .. } => None,
    };
    TestResultDto {
        verdict: result.verdict.tag().to_string(),
        message: result.message(),
        elapsed_ms: result.elapsed_ms,
        dimensions,
    }
}

/// Every backend as its wire shape, plus one row per consumer saying what it is presently assigned
/// to and whether that assignment would actually resolve.
fn backends_dto(backends: &[EmbeddingBackend], settings: &EmbeddingSettings) -> EmbeddingBackendsDto {
    let out = backends
        .iter()
        .map(|backend| {
            let used_by = CONSUMERS
                .into_iter()
                .filter(|consumer| {
                    settings.assignments.get(*consumer).map(String::as_str) == Some(backend.id.as_str())
                })
                .map(str::to_string)
                .collect();
            backend_dto(backend, used_by)
        })
        .collect();
    let assignments = CONSUMERS
        .into_iter()
        .map(|consumer| {
            let backend_id = settings.assignments.get(consumer).cloned().unwrap_or_default();
            let resolvable = backends
                .iter()
                .find(|b| b.id == backend_id)
                .is_some_and(|b| b.manifest.runtime.available());
            ConsumerAssignmentDto {
                consumer: consumer.to_string(),
                backend: backend_id,
                resolvable,
            }
        })
        .collect();
    EmbeddingBackendsDto {
        backends: out,
        assignments,
    }
}

/// One stored backend as its wire shape, with which consumers name it and whether this binary can
/// build it.
fn backend_dto(backend: &EmbeddingBackend, used_by: Vec<String>) -> EmbeddingBackendDto {
    let m = &backend.manifest;
    EmbeddingBackendDto {
        id: backend.id.clone(),
        label: m.label.clone(),
        runtime: m.runtime.to_string(),
        model: m.model.clone(),
        dimensions: m.dimensions,
        base_url: m.base_url.clone().unwrap_or_default(),
        api_key_env: m.api_key_env.clone().unwrap_or_default(),
        fallbacks: m.fallbacks.clone(),
        created_at: m.created_at,
        updated_at: m.updated_at,
        available: m.runtime.available(),
        used_by,
    }
}

/// The four runtimes, as the closed set they are — an unrecognised spelling is refused by name
/// rather than silently read as one of them (`hash`, being this crate's `#[default]`, is the one a
/// permissive read would quietly fall into).
fn runtime_from_wire(value: &str) -> Result<Runtime, Response> {
    serde_json::from_value(serde_json::Value::String(value.trim().to_string()))
        .map_err(|_| error(400, "runtime must be one of candle, ollama, openai, hash"))
}

fn blank_to_none(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl From<&adi_embeddings::Error> for Response {
    fn from(e: &adi_embeddings::Error) -> Self {
        use adi_embeddings::Error;
        let status = match e {
            Error::InvalidName(_) | Error::Arguments(_) => 400,
            Error::NotFound(_) | Error::Unassigned(_) => 404,
            Error::Config(_) | Error::Io(_) | Error::Embed(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for SaveEmbeddingBackend {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\", \"runtime\": \"candle|ollama|openai|hash\", \"model\": \"…\", \"dimensions\": 768, … } with a non-empty id and runtime";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty() && !self.runtime.trim().is_empty()
    }
}

impl FromBody for EmbeddingBackendRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for SaveEmbeddingSettings {
    const EXPECTED: &'static str = "expected JSON body { \"assignments\": { \"indexer\": \"candle\", \"knowledge\": \"candle\", \"facts\": \"ollama\" } }";
}

impl FromBody for TestEmbeddingBackend {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" } or { \"draft\": { \"runtime\": \"candle|ollama|openai|hash\", … } }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
            || self.draft.as_ref().is_some_and(|d| !d.runtime.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn scratch() -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-embedding-backends-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(adi_config::Config::with_root(root))
    }

    fn save(store: &Agents, id: &str, runtime: &str, model: &str, dimensions: u32) -> Response {
        save_embedding_backend(
            store,
            json!({
                "id": id,
                "runtime": runtime,
                "model": model,
                "dimensions": dimensions,
            })
            .to_string()
            .as_bytes(),
        )
    }

    #[test]
    fn a_fresh_store_lists_nothing_until_something_seeds_it() {
        let store = scratch();
        let v: Value = serde_json::from_str(&embedding_backends(&store).body).unwrap();
        assert!(v["backends"].as_array().unwrap().is_empty());
        // Every known consumer is still reported, unassigned and unresolvable — the page has
        // something to say about `facts` even though nothing has ever embedded through it here.
        assert_eq!(v["assignments"].as_array().unwrap().len(), 3);
        assert!(v["assignments"][0]["backend"].as_str().unwrap().is_empty());
        assert_eq!(v["assignments"][0]["resolvable"], false);
    }

    #[test]
    fn a_backend_round_trips_and_reports_availability() {
        let store = scratch();
        let Response { status, body } = save(&store, "ollama", "ollama", "nomic-embed-text", 768);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["backends"][0]["id"], "ollama");
        assert_eq!(v["backends"][0]["model"], "nomic-embed-text");
        // `ollama` never depends on a cargo feature, so it always reports available.
        assert_eq!(v["backends"][0]["available"], true);
    }

    #[test]
    fn an_unknown_runtime_spelling_is_refused_rather_than_read_as_hash() {
        let store = scratch();
        let Response { status, body } = save(&store, "x", "ollamaa", "m", 8);
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("candle, ollama, openai, hash"), "{body}");
    }

    #[test]
    fn the_settings_report_who_is_assigned_and_whether_it_resolves() {
        let store = scratch();
        save(&store, "ollama", "ollama", "nomic-embed-text", 768);
        let Response { status, body } = save_embedding_settings(
            &store,
            json!({"assignments": {"facts": "ollama"}}).to_string().as_bytes(),
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let facts = v["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["consumer"] == "facts")
            .unwrap();
        assert_eq!(facts["backend"], "ollama");
        assert_eq!(facts["resolvable"], true);
        assert_eq!(v["backends"][0]["used_by"], json!(["facts"]));
    }

    #[test]
    fn an_assignment_naming_nothing_is_refused() {
        let store = scratch();
        let Response { status, body } = save_embedding_settings(
            &store,
            json!({"assignments": {"facts": "ghost"}}).to_string().as_bytes(),
        );
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("ghost"), "{body}");
    }

    #[test]
    fn a_backend_a_consumer_still_names_is_not_deleted() {
        let store = scratch();
        save(&store, "ollama", "ollama", "nomic-embed-text", 768);
        let _ = save_embedding_settings(
            &store,
            json!({"assignments": {"facts": "ollama"}}).to_string().as_bytes(),
        );

        let Response { status, body } = delete_embedding_backend(&store, br#"{"id":"ollama"}"#);
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("facts"), "{body}");

        // Cleared off every consumer, it deletes.
        let _ = save_embedding_settings(
            &store,
            json!({"assignments": {"facts": ""}}).to_string().as_bytes(),
        );
        assert_eq!(delete_embedding_backend(&store, br#"{"id":"ollama"}"#).status, 200);
        assert_eq!(delete_embedding_backend(&store, br#"{"id":"ollama"}"#).status, 404);
    }

    #[test]
    fn a_body_naming_no_runtime_is_a_400() {
        let store = scratch();
        assert_eq!(save_embedding_backend(&store, br#"{"id":"x"}"#).status, 400);
        assert_eq!(save_embedding_backend(&store, b"{}").status, 400);
        assert_eq!(delete_embedding_backend(&store, br#"{"id":""}"#).status, 400);
    }

    /// `hash` never touches a network, which is what makes it a safe fixture for a unit test that
    /// actually embeds — and its width is fixed, so the reported `dimensions` is not a guess.
    #[test]
    fn test_by_id_answers_with_the_embedded_width() {
        let store = scratch();
        save(&store, "hash", "hash", "hash-bow-256", 256);
        let Response { status, body } = test_embedding_backend(&store, br#"{"id":"hash"}"#);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["verdict"], "ok", "{body}");
        assert_eq!(v["dimensions"], 256);
    }

    #[test]
    fn test_by_id_of_an_unknown_backend_is_404() {
        let store = scratch();
        assert_eq!(test_embedding_backend(&store, br#"{"id":"ghost"}"#).status, 404);
    }

    /// A draft is tested exactly as sent, whether or not it has ever been saved — and nothing about
    /// testing it writes a definition into the store.
    #[test]
    fn a_draft_is_tested_without_ever_being_saved() {
        let store = scratch();
        let Response { status, body } = test_embedding_backend(
            &store,
            br#"{"draft":{"id":"","runtime":"hash","model":"hash-bow-256","dimensions":256}}"#,
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["verdict"], "ok", "{body}");
        assert_eq!(v["dimensions"], 256);
        assert!(EmbeddingBackends::with_config(store.config().clone()).list().unwrap().is_empty());
    }

    /// `hash`/`candle` never take a model or width from configuration — `manifest_from` overrides
    /// whatever a stale or blank draft sent with the runtime's real, fixed pair, the same override
    /// `save_embedding_backend` applies. A draft naming the wrong width for `hash` therefore still
    /// tests clean, because there is nothing left in the manifest for it to disagree with by the
    /// time [`adi_embeddings::test_manifest`] sees it.
    #[test]
    fn a_fixed_model_runtimes_declared_width_is_never_trusted_from_the_draft() {
        let store = scratch();
        let Response { status, body } = test_embedding_backend(
            &store,
            br#"{"draft":{"id":"","runtime":"hash","model":"whatever","dimensions":9}}"#,
        );
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["verdict"], "ok", "{body}");
        assert_eq!(v["dimensions"], 256);
    }

    #[test]
    fn a_test_body_naming_neither_an_id_nor_a_draft_is_a_400() {
        let store = scratch();
        assert_eq!(test_embedding_backend(&store, b"{}").status, 400);
        assert_eq!(
            test_embedding_backend(&store, br#"{"draft":{"id":"x","runtime":""}}"#).status,
            400
        );
    }
}
