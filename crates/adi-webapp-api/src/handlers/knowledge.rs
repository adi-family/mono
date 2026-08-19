//! `/api/knowledge/*` — the control panel over [`adi_knowledge`].
//!
//! The panel acts for the person who owns the store, so every handler runs as the
//! [admin](adi_knowledge::Reader::admin) reader that [`KnowledgeStore::open`] gives: the three
//! isolation levels shape what an *agent* reaches, and the operator looking at their own machine
//! is not an agent. What the page shows about the levels is therefore descriptive — which base
//! sits at which level, whose it is — not a fence it is standing behind.
//!
//! The store is held by the host for the life of the process ([`adi-app`](../adi-app)'s `App`),
//! which is what keeps the embedding model resident. A handler that opened its own store per
//! request would reload ~300MB of weights on every search.

use adi_knowledge::{
    BaseId, Error as KnowledgeStoreError, Filter, Knowledge, KnowledgePatch, KnowledgeStore,
    NewKnowledge, Saved,
};

use crate::types::{
    KnowledgeBaseDto, KnowledgeBaseRef, KnowledgeHitDto, KnowledgeNoteDto, KnowledgeNoteRef,
    KnowledgeNotes, KnowledgeProviderDto, KnowledgeReembed, KnowledgeResults, KnowledgeSaved,
    KnowledgeSearch, KnowledgeState, NewKnowledgeBase, NewKnowledgeNote,
};

use super::response::{FromBody, Response, clean, error, ok_json, require};

/// How many notes a list returns when the request names no limit. Generous for a page that
/// shows one base at a time, and bounded so a base grown to thousands cannot wedge the panel.
const LIST_LIMIT: usize = 200;

/// How many hits a search returns by default.
const SEARCH_LIMIT: usize = 20;

/// `GET /api/knowledge` — every base with its counts, plus the providers this build offers.
///
/// Every mutation below answers with this same state, so the page refreshes from one round-trip.
#[must_use]
pub fn knowledge(store: &KnowledgeStore) -> Response {
    let bases = match store.list_bases(None) {
        Ok(bases) => bases,
        Err(e) => return Response::from(&e),
    };
    let mut out = Vec::with_capacity(bases.len());
    for base in bases {
        out.push(base_dto(store, &base));
    }
    ok_json(&KnowledgeState {
        bases: out,
        providers: store
            .providers()
            .all()
            .iter()
            .map(|p| KnowledgeProviderDto {
                name: p.name().to_string(),
                description: p.description().to_string(),
            })
            .collect(),
        model: store.model_name_if_loaded(),
    })
}

/// `POST /api/knowledge/base/create` — make a base, then report the fresh state.
#[must_use]
pub fn create_knowledge_base(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<NewKnowledgeBase>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match store.create_base(
        &id,
        clean(req.provider).as_deref(),
        clean(req.description).as_deref(),
        std::collections::BTreeMap::new(),
    ) {
        Ok(_) => knowledge(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/base/remove` — delete a base and everything in it.
#[must_use]
pub fn remove_knowledge_base(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeBaseRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match store.delete_base(&id) {
        Ok(_) => knowledge(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/reembed` — embed everything in a base that needs it.
///
/// This is the one endpoint that can take a while: it loads the model if nothing has yet, then
/// embeds every stale note. The page calls it deliberately, from a button, and shows the report.
#[must_use]
pub fn reembed_knowledge(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeBaseRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match store.reembed(&id, false) {
        Ok(report) => ok_json(&KnowledgeReembed {
            base: id.to_string(),
            scanned: report.scanned,
            embedded: report.embedded,
            unchanged: report.unchanged,
            chunks: report.chunks,
            failed: report
                .failed
                .into_iter()
                .map(|f| format!("{}: {}", f.id, f.error))
                .collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/notes` — the notes in one base, newest first.
#[must_use]
pub fn knowledge_notes(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeBaseRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let filter = Filter {
        tags: req.tags,
        limit: Some(req.limit.unwrap_or(LIST_LIMIT)),
        stale_only: false,
    };
    match store.list(&id, &filter) {
        Ok(notes) => ok_json(&KnowledgeNotes {
            base: id.to_string(),
            notes: notes.iter().map(note_dto).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/note/get` — one note in full.
#[must_use]
pub fn knowledge_note(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeNoteRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match store.get(&id, req.id.trim()) {
        Ok(Some(note)) => ok_json(&note_dto(&note)),
        Ok(None) => error(404, &format!("no such knowledge: {}", req.id.trim())),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/note/add` — write a note, embedding it on the way in.
#[must_use]
pub fn add_knowledge_note(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<NewKnowledgeNote>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let new = NewKnowledge {
        title: req.title,
        body: req.body,
        tags: req.tags,
        source: clean(req.source),
        id: None,
    };
    match store.add(&id, new) {
        Ok(saved) => ok_json(&saved_dto(saved)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/note/edit` — change a note; absent fields are left alone.
///
/// A change to the embedded text re-embeds; a change to anything else keeps the vectors, which
/// is the store's rule and not this handler's — it only forwards the patch.
#[must_use]
pub fn edit_knowledge_note(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<EditNote>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let patch = KnowledgePatch {
        title: req.title,
        body: req.body,
        tags: req.tags,
        // `Some(None)` clears the source; the page sends a blank string for that, and omitting
        // the field entirely leaves whatever the note already had.
        source: req.source.map(|s| clean(Some(s))),
    };
    match store.update(&id, req.id.trim(), patch) {
        Ok(saved) => ok_json(&saved_dto(saved)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/note/remove` — delete a note, then report the base's fresh list.
#[must_use]
pub fn remove_knowledge_note(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeNoteRef>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let id = match parse_base(&req.base) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match store.remove(&id, req.id.trim()) {
        Ok(_) => {
            let listing = KnowledgeBaseRef {
                base: req.base,
                tags: Vec::new(),
                limit: None,
            };
            match serde_json::to_vec(&listing) {
                Ok(bytes) => knowledge_notes(store, &bytes),
                Err(e) => error(500, &format!("re-listing the base: {e}")),
            }
        }
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/knowledge/search` — rank by meaning, or by words with `text: true`.
///
/// With no `bases` named this searches every base, which is what the page's search box does: a
/// question is rarely about one collection, and the query is embedded once however many bases
/// it is put to.
#[must_use]
pub fn search_knowledge(store: &KnowledgeStore, body: &[u8]) -> Response {
    let req = match require::<KnowledgeSearch>(body) {
        Ok(req) => req,
        Err(bad) => return bad,
    };
    let bases = if req.bases.is_empty() {
        match store.visible_bases() {
            Ok(bases) => bases,
            Err(e) => return Response::from(&e),
        }
    } else {
        let mut out = Vec::with_capacity(req.bases.len());
        for raw in &req.bases {
            match parse_base(raw) {
                Ok(id) => out.push(id),
                Err(resp) => return resp,
            }
        }
        out
    };
    let limit = req.limit.unwrap_or(SEARCH_LIMIT);
    let found = if req.text {
        store.search_text(&bases, req.query.trim(), limit)
    } else {
        store.search(&bases, req.query.trim(), limit)
    };
    match found {
        Ok(hits) => ok_json(&KnowledgeResults {
            query: req.query,
            semantic: !req.text,
            bases: bases.iter().map(ToString::to_string).collect(),
            hits: hits
                .into_iter()
                .map(|h| KnowledgeHitDto {
                    note: note_dto(&h.knowledge),
                    score: h.score,
                    chunk: h.chunk,
                })
                .collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// The edit request. Declared here rather than in `types` because every field is a
/// three-state (absent = unchanged), which only this endpoint speaks.
#[derive(Debug, serde::Deserialize)]
struct EditNote {
    base: String,
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    source: Option<String>,
}

fn base_dto(store: &KnowledgeStore, base: &adi_knowledge::Base) -> KnowledgeBaseDto {
    let mut dto = KnowledgeBaseDto {
        id: base.id.to_string(),
        level: base.level().to_string(),
        owner: base.id.scope.owner().map(ToString::to_string),
        name: base.id.name.clone(),
        provider: base.manifest.provider.clone(),
        description: base.manifest.description.clone(),
        memory: base.is_memory(),
        notes: 0,
        embedded: 0,
        stale: 0,
        error: None,
        created_at: base.manifest.created_at,
        updated_at: base.manifest.updated_at,
    };
    // A base whose provider this build cannot open still belongs in the listing — saying so is
    // the point. Reporting it as empty would read as "nothing in here", which is a different and
    // wrong answer.
    match store.base_status(&base.id) {
        Ok(status) => {
            dto.notes = status.notes;
            dto.embedded = status.embedded;
            dto.stale = status.stale;
        }
        Err(e) => dto.error = Some(e.to_string()),
    }
    dto
}

fn note_dto(note: &Knowledge) -> KnowledgeNoteDto {
    KnowledgeNoteDto {
        id: note.id.clone(),
        base: note.base.as_ref().map(ToString::to_string).unwrap_or_default(),
        title: note.title.clone(),
        body: note.body.clone(),
        tags: note.tags.clone(),
        source: note.source.clone(),
        embedded: note.is_embedded(),
        chunks: note.embedding.chunks,
        model: note.embedding.model.clone(),
        created_at: note.created_at,
        updated_at: note.updated_at,
    }
}

fn saved_dto(saved: Saved) -> KnowledgeSaved {
    KnowledgeSaved {
        note: note_dto(&saved.knowledge),
        embedded: saved.embedded,
        embed_error: saved.embed_error,
    }
}

/// Parse a written base id, answering with a 400 rather than a panic on anything else.
fn parse_base(raw: &str) -> Result<BaseId, Response> {
    raw.trim()
        .parse::<BaseId>()
        .map_err(|e| error(400, &e.to_string()))
}

impl FromBody for NewKnowledgeBase {
    const EXPECTED: &'static str = "expected JSON body { \"base\": \"global/<name>\", \"provider\"?: \"…\", \"description\"?: \"…\" }";

    fn is_complete(&self) -> bool {
        !self.base.trim().is_empty()
    }
}

impl FromBody for KnowledgeBaseRef {
    const EXPECTED: &'static str = "expected JSON body { \"base\": \"global/<name>\" }";

    fn is_complete(&self) -> bool {
        !self.base.trim().is_empty()
    }
}

impl FromBody for KnowledgeNoteRef {
    const EXPECTED: &'static str =
        "expected JSON body { \"base\": \"global/<name>\", \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.base.trim().is_empty() && !self.id.trim().is_empty()
    }
}

impl FromBody for NewKnowledgeNote {
    const EXPECTED: &'static str = "expected JSON body { \"base\": \"global/<name>\", \"title\": \"…\", \"body\"?: \"…\", \"tags\"?: [], \"source\"?: \"…\" } — a note needs a title or a body";

    fn is_complete(&self) -> bool {
        let has_content = !self.title.trim().is_empty() || !self.body.trim().is_empty();
        !self.base.trim().is_empty() && has_content
    }
}

impl FromBody for EditNote {
    const EXPECTED: &'static str = "expected JSON body { \"base\": \"global/<name>\", \"id\": \"…\", \"title\"?, \"body\"?, \"tags\"?, \"source\"? } — omitted fields are left as they are";

    fn is_complete(&self) -> bool {
        !self.base.trim().is_empty() && !self.id.trim().is_empty()
    }
}

impl FromBody for KnowledgeSearch {
    const EXPECTED: &'static str =
        "expected JSON body { \"query\": \"…\", \"bases\"?: [], \"limit\"?: 20, \"text\"?: false }";

    fn is_complete(&self) -> bool {
        !self.query.trim().is_empty()
    }
}

// A store error's HTTP status: a malformed name or id is the caller's (400), a missing base or
// note is a 404, a refusal is a 403, and everything else is the machine's (500).
impl From<&KnowledgeStoreError> for Response {
    fn from(e: &KnowledgeStoreError) -> Self {
        let status = match e {
            KnowledgeStoreError::InvalidName(_)
            | KnowledgeStoreError::BadBaseId(_)
            | KnowledgeStoreError::Empty => 400,
            KnowledgeStoreError::NoSuchBase(_)
            | KnowledgeStoreError::NoSuchKnowledge(_)
            | KnowledgeStoreError::NoSuchProvider(_) => 404,
            KnowledgeStoreError::BaseExists(_) => 409,
            KnowledgeStoreError::Denied { .. } => 403,
            // An embedder that will not load is a machine problem, and the message says which —
            // no model on disk, no network on a first run. A 503 rather than a 500 because it is
            // the kind of failure that stops being true.
            KnowledgeStoreError::Embed(_) => 503,
            KnowledgeStoreError::Backend(_)
            | KnowledgeStoreError::Config(_)
            | KnowledgeStoreError::Io(_) => 500,
        };
        error(status, &e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_knowledge::{HashEmbedder, Providers};
    use std::sync::Arc;

    /// A store on a scratch root with a stand-in embedder — the handlers are what is under test,
    /// not the model. Same rooting convention as the other handler tests here.
    fn scratch(tag: &str) -> KnowledgeStore {
        let root = std::env::temp_dir().join(format!(
            "adi-knowledge-api-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        KnowledgeStore::with_config(adi_config::Config::with_root(root))
            .with_providers(Arc::new(Providers::builtin()))
            .with_embedder(Arc::new(HashEmbedder))
    }

    fn json(resp: &Response) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("json body")
    }

    fn body<T: serde::Serialize>(value: &T) -> Vec<u8> {
        serde_json::to_vec(value).expect("encode")
    }

    /// A setup call that has to have worked. A test whose fixture failed silently proves
    /// nothing — it just asserts against an empty store.
    fn must(resp: Response) {
        assert!(
            (200..300).contains(&resp.status),
            "setup failed: {} {}",
            resp.status,
            resp.body
        );
    }

    #[test]
    fn the_state_lists_bases_with_their_level_and_counts() {
        let store = scratch("state");
        assert_eq!(
            create_knowledge_base(
                &store,
                &body(&serde_json::json!({ "base": "global/runbooks", "description": "ops" }))
            )
            .status,
            200
        );
        let resp = knowledge(&store);
        assert_eq!(resp.status, 200);
        let v = json(&resp);
        assert_eq!(v["bases"][0]["id"], "global/runbooks");
        assert_eq!(v["bases"][0]["level"], "global");
        assert_eq!(v["bases"][0]["provider"], "sqlite");
        assert_eq!(v["bases"][0]["notes"], 0);
        // The providers this build offers travel with the state, so the create form can offer them.
        assert!(v["providers"].as_array().expect("providers").len() >= 2);
    }

    #[test]
    fn a_note_is_added_embedded_and_listed() {
        let store = scratch("notes");
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));

        let resp = add_knowledge_note(
            &store,
            &body(&serde_json::json!({
                "base": "global/n", "title": "Restarting the panel",
                "body": "launchctl kickstart", "tags": ["ops"],
            })),
        );
        assert_eq!(resp.status, 200);
        let v = json(&resp);
        assert_eq!(v["note"]["id"], "restarting-the-panel");
        assert_eq!(v["embedded"], true, "{v}");

        let listed = json(&knowledge_notes(
            &store,
            &body(&serde_json::json!({ "base": "global/n" })),
        ));
        assert_eq!(listed["notes"].as_array().expect("notes").len(), 1);
        assert_eq!(listed["notes"][0]["tags"][0], "ops");
    }

    #[test]
    fn search_ranks_and_says_what_it_covered() {
        let store = scratch("search");
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));
        for title in ["Restart the control panel", "Sourdough hydration"] {
            must(add_knowledge_note(
                &store,
                &body(&serde_json::json!({ "base": "global/n", "title": title })),
            ));
        }
        let v = json(&search_knowledge(
            &store,
            &body(&serde_json::json!({ "query": "restart the panel" })),
        ));
        assert_eq!(v["semantic"], true);
        assert_eq!(v["bases"][0], "global/n", "an empty request covers everything");
        assert_eq!(v["hits"][0]["id"], "restart-the-control-panel");
        assert!(v["hits"][0]["score"].as_f64().expect("score") > 0.0);

        // …and the word path needs no model at all.
        let text = json(&search_knowledge(
            &store,
            &body(&serde_json::json!({ "query": "sourdough", "text": true })),
        ));
        assert_eq!(text["semantic"], false);
        assert_eq!(text["hits"][0]["id"], "sourdough-hydration");
    }

    #[test]
    fn an_edit_that_names_no_new_text_keeps_the_note_embedded() {
        let store = scratch("edit");
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));
        must(add_knowledge_note(
            &store,
            &body(&serde_json::json!({ "base": "global/n", "title": "A", "body": "one" })),
        ));
        let v = json(&edit_knowledge_note(
            &store,
            &body(&serde_json::json!({ "base": "global/n", "id": "a", "source": "docs/x.md" })),
        ));
        assert_eq!(v["note"]["source"], "docs/x.md");
        assert_eq!(v["note"]["body"], "one", "an omitted field is left alone");
        assert_eq!(v["embedded"], true);
    }

    #[test]
    fn removing_a_note_answers_with_the_bases_fresh_list() {
        let store = scratch("remove");
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));
        must(add_knowledge_note(
            &store,
            &body(&serde_json::json!({ "base": "global/n", "title": "A" })),
        ));
        let v = json(&remove_knowledge_note(
            &store,
            &body(&serde_json::json!({ "base": "global/n", "id": "a" })),
        ));
        assert_eq!(v["notes"].as_array().expect("notes").len(), 0);
    }

    /// The statuses the page keys its messages off: a bad id is the caller's fault, a missing
    /// base is a 404, and a base that already exists is a 409 rather than a silent overwrite.
    #[test]
    fn store_errors_map_onto_the_statuses_the_page_expects() {
        let store = scratch("errors");
        assert_eq!(
            create_knowledge_base(&store, &body(&serde_json::json!({ "base": "team:x/y" }))).status,
            400
        );
        assert_eq!(
            knowledge_notes(&store, &body(&serde_json::json!({ "base": "global/nope" }))).status,
            404
        );
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));
        assert_eq!(
            create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))).status,
            409
        );
        assert_eq!(
            knowledge_note(
                &store,
                &body(&serde_json::json!({ "base": "global/n", "id": "ghost" }))
            )
            .status,
            404
        );
    }

    #[test]
    fn a_malformed_body_says_what_it_wanted() {
        let store = scratch("bad");
        for resp in [
            search_knowledge(&store, b"{}"),
            add_knowledge_note(&store, b"{}"),
            knowledge_notes(&store, b"{}"),
            create_knowledge_base(&store, b"not json"),
        ] {
            assert_eq!(resp.status, 400);
            assert!(resp.body.contains("expected JSON body"), "{}", resp.body);
        }
    }

    #[test]
    fn deleting_a_base_takes_it_out_of_the_state() {
        let store = scratch("delbase");
        must(create_knowledge_base(&store, &body(&serde_json::json!({ "base": "global/n" }))));
        let v = json(&remove_knowledge_base(
            &store,
            &body(&serde_json::json!({ "base": "global/n" })),
        ));
        assert_eq!(v["bases"].as_array().expect("bases").len(), 0);
    }
}
