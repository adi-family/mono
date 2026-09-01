//! Store-level tests: the three isolation levels, the re-embedding contract, pluggable
//! providers, and search across bases.
//!
//! They run against the real SQLite provider (the one that ships) with a stand-in embedder, so
//! what is exercised is the storage and the bookkeeping rather than a 300MB model download. The
//! one thing [`HashEmbedder`] cannot stand in for — that *meaning* ranks above spelling — is not
//! this crate's claim to prove; it is the model's, and the indexer already leans on it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use adi_config::Config;

use crate::backend::{Backend, BaseContext, MEMORY, Provider, Providers, SQLITE};
use crate::embed::{EmbedError, Embedder, HashEmbedder};
use crate::{
    BaseId, Error, Filter, KnowledgePatch, KnowledgeStore, NewKnowledge, Reader, Scope,
    resolve_agent_bases,
};

/// A store on a scratch directory, with a deterministic embedder and no model to load.
fn store() -> (tempfile::TempDir, KnowledgeStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder));
    (dir, store)
}

fn base(id: &str) -> BaseId {
    id.parse().expect("base id")
}

/// Counts how many texts it was asked to embed, so a test can assert that nothing re-embedded.
#[derive(Debug, Default)]
struct CountingEmbedder {
    texts: AtomicUsize,
}

impl CountingEmbedder {
    fn texts(&self) -> usize {
        self.texts.load(Ordering::Relaxed)
    }
}

impl Embedder for CountingEmbedder {
    fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        self.texts.fetch_add(texts.len(), Ordering::Relaxed);
        HashEmbedder.embed(texts)
    }

    fn dimensions(&self) -> u32 {
        HashEmbedder.dimensions()
    }

    fn model_name(&self) -> &str {
        "counting"
    }
}

/// An embedder that refuses — a machine with no model on disk and no network to fetch one.
#[derive(Debug)]
struct BrokenEmbedder;

impl Embedder for BrokenEmbedder {
    fn embed(&self, _texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unavailable("no model on this machine".into()))
    }

    fn dimensions(&self) -> u32 {
        0
    }

    fn model_name(&self) -> &str {
        "broken"
    }
}

// ------------------------------------------------------------------- bases

#[test]
fn a_base_has_to_exist_before_anything_can_be_put_in_it() {
    let (_dir, store) = store();
    let id = base("global/runbooks");
    let err = store.add(&id, NewKnowledge::new("T", "B")).unwrap_err();
    assert!(matches!(err, Error::NoSuchBase(_)), "{err:?}");

    store.ensure_base(&id).expect("ensure");
    assert!(store.add(&id, NewKnowledge::new("T", "B")).is_ok());
}

#[test]
fn renaming_a_project_moves_its_bases_and_the_notes_in_them() {
    let (_dir, store) = store();
    let old = base("project:old/runbook");
    store.ensure_base(&old).expect("ensure");
    store
        .add(&old, NewKnowledge::new("Front door", "Rebuild adi-hive"))
        .expect("add");
    let elsewhere = base("project:other/runbook");
    store.ensure_base(&elsewhere).expect("ensure other");
    let global = base("global/notes");
    store.ensure_base(&global).expect("ensure global");

    assert_eq!(store.rename_project("old", "new").expect("rename"), 1);

    let moved = base("project:new/runbook");
    assert!(store.get_base(&moved).expect("get").is_some());
    assert!(store.get_base(&old).expect("get old").is_none());
    let notes = store.list(&moved, &Filter::default()).expect("list");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Front door");

    // Bases belonging to another project, or to nobody, stayed where they were.
    assert!(store.get_base(&elsewhere).expect("get").is_some());
    assert!(store.get_base(&global).expect("get").is_some());

    // Nothing to move a second time; a project with no bases is a no-op, not a failure.
    assert_eq!(store.rename_project("old", "new").expect("again"), 0);
    assert_eq!(store.rename_project("ghost", "spirit").expect("none"), 0);
}

#[test]
fn renaming_a_project_onto_an_occupied_base_moves_nothing() {
    let (_dir, store) = store();
    store
        .ensure_base(&base("project:old/runbook"))
        .expect("old");
    store
        .ensure_base(&base("project:old/scratch"))
        .expect("old scratch");
    store
        .ensure_base(&base("project:new/runbook"))
        .expect("new");

    let err = store.rename_project("old", "new").unwrap_err();
    assert!(matches!(err, Error::BaseExists(_)), "{err:?}");
    // The refusal is checked before the first move, so the whole of `old` is still there.
    assert!(
        store
            .get_base(&base("project:old/runbook"))
            .expect("get")
            .is_some()
    );
    assert!(
        store
            .get_base(&base("project:old/scratch"))
            .expect("get")
            .is_some()
    );
}

#[test]
fn creating_a_base_twice_is_refused_but_ensuring_it_twice_is_not() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store
        .create_base(&id, None, Some("shared"), BTreeMap::new())
        .expect("create");
    assert!(matches!(
        store
            .create_base(&id, None, None, BTreeMap::new())
            .unwrap_err(),
        Error::BaseExists(_)
    ));
    assert_eq!(
        store.ensure_base(&id).expect("ensure").manifest.description,
        Some("shared".into()),
        "ensure must not overwrite the base that is there"
    );
}

#[test]
fn a_base_naming_a_provider_nobody_registered_is_never_written() {
    let (_dir, store) = store();
    let id = base("global/hosted");
    let err = store
        .create_base(&id, Some("pinecone"), None, BTreeMap::new())
        .unwrap_err();
    assert!(matches!(err, Error::NoSuchProvider(_)), "{err:?}");
    assert!(
        store.get_base(&id).expect("get").is_none(),
        "a base that cannot be opened must not be left in the listing"
    );
}

#[test]
fn bases_list_across_all_three_levels_and_survive_a_reopen() {
    let (dir, store) = store();
    for id in [
        "global/notes",
        "project:acme/runbook",
        "agent:solver/memory",
    ] {
        store.ensure_base(&base(id)).expect("ensure");
    }
    let listed: Vec<String> = store
        .list_bases(None)
        .expect("list")
        .iter()
        .map(|b| b.id.to_string())
        .collect();
    assert_eq!(
        listed,
        vec![
            "global/notes",
            "project:acme/runbook",
            "agent:solver/memory"
        ],
        "sorted by scope then name"
    );

    // Narrowing to one scope.
    let acme = store
        .list_bases(Some(&Scope::project("acme").unwrap()))
        .expect("list");
    assert_eq!(acme.len(), 1);
    assert_eq!(acme[0].level(), "project");

    // A fresh store over the same directory finds them all again.
    let reopened = KnowledgeStore::with_config(Config::with_root(dir.path()));
    assert_eq!(reopened.list_bases(None).expect("list").len(), 3);
}

#[test]
fn deleting_a_base_takes_its_notes_with_it() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store.add(&id, NewKnowledge::new("A", "body")).expect("add");

    assert!(store.delete_base(&id).expect("delete"));
    assert!(!store.delete_base(&id).expect("second delete"));
    assert!(store.get_base(&id).expect("get").is_none());
    assert!(!store.base_dir(&id).exists());

    // Recreating it gives an empty base, not the old contents.
    store.ensure_base(&id).expect("re-ensure");
    assert!(
        store
            .list(&id, &Filter::default())
            .expect("list")
            .is_empty()
    );
}

// ------------------------------------------------------------------- notes

#[test]
fn a_note_of_any_length_round_trips_and_is_embedded_on_the_way_in() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");

    let long = "This paragraph is here to make the note long. ".repeat(200);
    let saved = store
        .add(
            &id,
            NewKnowledge::new("Restarting the panel", long.as_str()).tagged(["Ops", "adi", "ops"]),
        )
        .expect("add");

    assert!(saved.embedded, "{:?}", saved.embed_error);
    assert_eq!(saved.knowledge.id, "restarting-the-panel");
    assert_eq!(saved.knowledge.tags, vec!["adi", "ops"], "normalized");
    assert_eq!(saved.knowledge.base.as_ref(), Some(&id));
    assert!(
        saved.knowledge.embedding.chunks > 1,
        "a long note must be chunked, not truncated: {:?}",
        saved.knowledge.embedding
    );

    let read = store
        .get(&id, "restarting-the-panel")
        .expect("get")
        .expect("present");
    assert_eq!(read.body, long.trim_end());
    assert!(!read.is_stale("hash-bow-256"));
}

#[test]
fn two_notes_with_the_same_title_are_two_notes() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");

    let first = store
        .add(&id, NewKnowledge::new("Deploy", "one"))
        .expect("add");
    let second = store
        .add(&id, NewKnowledge::new("Deploy", "two"))
        .expect("add");
    assert_eq!(first.knowledge.id, "deploy");
    assert_eq!(second.knowledge.id, "deploy-2");
    assert_eq!(store.list(&id, &Filter::default()).expect("list").len(), 2);
}

/// The escape hatch an importer needs: a stated id means "this note", run after run.
#[test]
fn an_explicit_id_replaces_rather_than_multiplying() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");

    let first = store
        .add(
            &id,
            NewKnowledge {
                id: Some("upstream-42".into()),
                ..NewKnowledge::new("Title", "first import")
            },
        )
        .expect("add");
    let second = store
        .add(
            &id,
            NewKnowledge {
                id: Some("upstream-42".into()),
                ..NewKnowledge::new("Title", "second import")
            },
        )
        .expect("re-add");

    assert_eq!(store.list(&id, &Filter::default()).expect("list").len(), 1);
    assert_eq!(second.knowledge.body, "second import");
    assert_eq!(
        second.knowledge.created_at, first.knowledge.created_at,
        "a re-import keeps the note's birthday"
    );
}

#[test]
fn a_note_with_nothing_in_it_is_refused() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    assert!(matches!(
        store
            .add(&id, NewKnowledge::new("   ", "  \n "))
            .unwrap_err(),
        Error::Empty
    ));
}

#[test]
fn a_note_with_only_a_body_still_gets_an_id() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    let saved = store
        .add(
            &id,
            NewKnowledge::new("", "the front door hot-reloads its routes"),
        )
        .expect("add");
    assert_eq!(saved.knowledge.id, "the-front-door-hot-reloads-its-routes");
}

#[test]
fn listing_filters_by_tag_and_respects_its_limit() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store
        .add(&id, NewKnowledge::new("A", "x").tagged(["ops"]))
        .expect("add");
    store
        .add(&id, NewKnowledge::new("B", "x").tagged(["net"]))
        .expect("add");
    store
        .add(&id, NewKnowledge::new("C", "x").tagged(["ops", "net"]))
        .expect("add");

    assert_eq!(
        store
            .list(&id, &Filter::tagged(["OPS"]))
            .expect("list")
            .len(),
        2
    );
    assert_eq!(
        store
            .list(&id, &Filter::tagged(["ops", "net"]))
            .expect("list")
            .len(),
        1
    );
    assert_eq!(store.list(&id, &Filter::limit(2)).expect("list").len(), 2);
}

#[test]
fn removing_a_note_takes_it_out_of_search_too() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store
        .add(&id, NewKnowledge::new("Findable", "unique-token"))
        .expect("add");
    assert_eq!(
        store
            .search(&[id.clone()], "unique-token", 5)
            .expect("search")
            .len(),
        1
    );

    assert!(store.remove(&id, "findable").expect("remove"));
    assert!(!store.remove(&id, "findable").expect("second remove"));
    assert!(
        store
            .search(&[id.clone()], "unique-token", 5)
            .expect("search")
            .is_empty()
    );
    assert!(
        store
            .search_text(&[id], "unique-token", 5)
            .expect("search")
            .is_empty()
    );
}

// -------------------------------------------------------- the re-embedding contract

/// The promise the whole design turns on.
#[test]
fn editing_a_note_re_embeds_it() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store
        .add(&id, NewKnowledge::new("Note", "alpha alpha alpha"))
        .expect("add");

    // Before the edit, the old text is what matches.
    let before = store.search(&[id.clone()], "alpha", 5).expect("search");
    assert_eq!(before.len(), 1);

    let saved = store
        .update(
            &id,
            "note",
            KnowledgePatch {
                body: Some("omega omega omega".into()),
                ..KnowledgePatch::default()
            },
        )
        .expect("update");
    assert!(saved.embedded, "{:?}", saved.embed_error);
    assert!(!saved.knowledge.is_stale("hash-bow-256"));

    // The vectors describe the new text, and no longer the old.
    let omega = store.search(&[id.clone()], "omega", 5).expect("search");
    assert_eq!(omega.len(), 1);
    let alpha = store.search(&[id], "alpha", 5).expect("search");
    assert!(
        alpha.is_empty(),
        "the note is still findable by text it no longer contains"
    );
}

/// The other half: an edit that does not touch the embedded text must not pay to embed again.
#[test]
fn an_edit_that_changes_no_text_does_not_re_embed() {
    let (dir, _) = store();
    let counter = Arc::new(CountingEmbedder::default());
    let store =
        KnowledgeStore::with_config(Config::with_root(dir.path())).with_embedder(counter.clone());
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store
        .add(&id, NewKnowledge::new("Note", "body"))
        .expect("add");
    let after_add = counter.texts();
    assert!(after_add > 0);

    let saved = store
        .update(
            &id,
            "note",
            KnowledgePatch {
                source: Some(Some("docs/deploy.md".into())),
                ..KnowledgePatch::default()
            },
        )
        .expect("update");
    assert_eq!(saved.knowledge.source.as_deref(), Some("docs/deploy.md"));
    assert!(
        saved.embedded,
        "the vectors were still accurate and must be kept"
    );
    assert_eq!(counter.texts(), after_add, "re-embedded for no reason");
}

#[test]
fn an_edit_that_changes_nothing_at_all_does_not_even_touch_the_timestamp() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    let first = store
        .add(&id, NewKnowledge::new("Note", "body"))
        .expect("add");

    let again = store
        .update(&id, "note", KnowledgePatch::default())
        .expect("update");
    assert_eq!(again.knowledge.updated_at, first.knowledge.updated_at);
}

/// A note written while the model was unavailable is kept, marked honestly, and swept up later.
#[test]
fn a_note_that_could_not_be_embedded_is_still_stored_and_says_so() {
    let (dir, _) = store();
    let broken = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(BrokenEmbedder));
    let id = base("global/notes");
    broken.ensure_base(&id).expect("ensure");

    let saved = broken
        .add(&id, NewKnowledge::new("Note", "body"))
        .expect("add");
    assert!(!saved.embedded);
    assert!(
        saved
            .embed_error
            .as_deref()
            .is_some_and(|e| e.contains("no model")),
        "the reason must travel with the result: {:?}",
        saved.embed_error
    );
    // It is there, and full-text finds it even though meaning cannot.
    assert_eq!(broken.list(&id, &Filter::default()).expect("list").len(), 1);
    assert_eq!(
        broken
            .search_text(&[id.clone()], "body", 5)
            .expect("search")
            .len(),
        1
    );

    // The same store with a working embedder sweeps it up.
    let fixed = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder));
    let report = fixed.reembed(&id, false).expect("reembed");
    assert_eq!(
        (report.scanned, report.embedded, report.unchanged),
        (1, 1, 0)
    );
    assert!(report.failed.is_empty());
    assert_eq!(fixed.search(&[id], "body", 5).expect("search").len(), 1);
}

#[test]
fn re_embedding_an_up_to_date_base_embeds_nothing() {
    let (dir, _) = store();
    let counter = Arc::new(CountingEmbedder::default());
    let store =
        KnowledgeStore::with_config(Config::with_root(dir.path())).with_embedder(counter.clone());
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store.add(&id, NewKnowledge::new("A", "one")).expect("add");
    store.add(&id, NewKnowledge::new("B", "two")).expect("add");
    let after_adds = counter.texts();

    let report = store.reembed(&id, false).expect("reembed");
    assert_eq!(
        (report.scanned, report.embedded, report.unchanged),
        (2, 0, 2)
    );
    assert_eq!(counter.texts(), after_adds);

    // `--force` is the override, and it does the work.
    let forced = store.reembed(&id, true).expect("forced");
    assert_eq!(forced.embedded, 2);
    assert!(counter.texts() > after_adds);
}

/// Swapping the model invalidates every vector at once — that is what the model name is for.
#[test]
fn changing_the_model_makes_the_whole_base_stale() {
    let (dir, _) = store();
    let first = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder));
    let id = base("global/notes");
    first.ensure_base(&id).expect("ensure");
    first.add(&id, NewKnowledge::new("A", "one")).expect("add");
    assert_eq!(first.base_status(&id).expect("status").stale, 0);

    let swapped = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(CountingEmbedder::default()));
    let status = swapped.base_status(&id).expect("status");
    assert_eq!((status.notes, status.embedded, status.stale), (1, 0, 1));
    assert_eq!(
        swapped
            .list(
                &id,
                &Filter {
                    stale_only: true,
                    ..Filter::default()
                }
            )
            .expect("list")
            .len(),
        1
    );

    assert_eq!(swapped.reembed(&id, false).expect("reembed").embedded, 1);
    assert_eq!(swapped.base_status(&id).expect("status").stale, 0);
}

// ------------------------------------------------------------------ search

#[test]
fn search_ranks_the_closer_note_first_and_honours_its_limit() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    store
        .add(&id, NewKnowledge::new("Restart the control panel", ""))
        .expect("add");
    store
        .add(&id, NewKnowledge::new("Sourdough hydration", ""))
        .expect("add");

    let hits = store
        .search(&[id.clone()], "restart the panel", 5)
        .expect("search");
    assert_eq!(hits[0].knowledge.id, "restart-the-control-panel");
    assert!(hits[0].score > 0.0);
    assert_eq!(
        store
            .search(&[id], "restart the panel", 1)
            .expect("search")
            .len(),
        1
    );
}

#[test]
fn one_search_spans_several_bases_and_says_which_one_each_hit_came_from() {
    let (_dir, store) = store();
    let shared = base("global/notes");
    let mine = base("agent:solver/memory");
    for id in [&shared, &mine] {
        store.ensure_base(id).expect("ensure");
    }
    store
        .add(&shared, NewKnowledge::new("Deploying the panel", ""))
        .expect("add");
    store
        .add(&mine, NewKnowledge::new("Deploying the panel my way", ""))
        .expect("add");

    let hits = store
        .search(&[shared.clone(), mine.clone()], "deploying the panel", 10)
        .expect("search");
    assert_eq!(hits.len(), 2);
    let bases: Vec<&BaseId> = hits
        .iter()
        .filter_map(|h| h.knowledge.base.as_ref())
        .collect();
    assert!(bases.contains(&&shared) && bases.contains(&&mine));

    // Naming the same base twice does not double the results.
    let twice = store
        .search(&[shared.clone(), shared], "deploying the panel", 10)
        .expect("search");
    assert_eq!(twice.len(), 1);
}

#[test]
fn text_search_works_with_no_embedder_at_all() {
    let (dir, _) = store();
    let broken = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(BrokenEmbedder));
    let id = base("global/notes");
    broken.ensure_base(&id).expect("ensure");
    broken
        .add(&id, NewKnowledge::new("Kickstart the agent", "launchctl"))
        .expect("add");

    assert_eq!(
        broken
            .search_text(&[id.clone()], "kickstart", 5)
            .expect("search")
            .len(),
        1
    );
    // …and the meaning search says plainly that it cannot run.
    let err = broken.search(&[id], "kickstart", 5).unwrap_err();
    assert!(matches!(err, Error::Embed(_)), "{err:?}");
}

// ------------------------------------------------------- the three levels

#[test]
fn an_agent_sees_global_its_own_and_other_agents_but_not_another_project() {
    let (_dir, store) = store();
    for id in [
        "global/notes",
        "project:acme/runbook",
        "project:other/runbook",
        "agent:solver/memory",
        "agent:reviewer/memory",
    ] {
        store.ensure_base(&base(id)).expect("ensure");
    }

    let solver = store.clone().as_agent("solver", Some("acme"));
    let visible: Vec<String> = solver
        .visible_bases()
        .expect("visible")
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        visible,
        vec![
            "global/notes",
            "project:acme/runbook",
            "agent:reviewer/memory",
            "agent:solver/memory",
        ],
        "project:other must not be in there"
    );
}

#[test]
fn an_agent_can_read_another_agents_knowledge_and_cannot_change_it() {
    let (_dir, store) = store();
    let reviewers = base("agent:reviewer/memory");
    store.ensure_base(&reviewers).expect("ensure");
    store
        .clone()
        .as_agent("reviewer", None)
        .add(
            &reviewers,
            NewKnowledge::new("What I learned", "the deploy needs a restart"),
        )
        .expect("add");

    let solver = store.clone().as_agent("solver", None);

    // Reading and searching: yes.
    let hits = solver
        .search(&[reviewers.clone()], "the deploy needs a restart", 5)
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert!(
        solver
            .get(&reviewers, "what-i-learned")
            .expect("get")
            .is_some()
    );
    assert_eq!(
        solver
            .list(&reviewers, &Filter::default())
            .expect("list")
            .len(),
        1
    );

    // Writing to it: no, in every direction.
    for err in [
        solver
            .add(&reviewers, NewKnowledge::new("Mine", "x"))
            .unwrap_err(),
        solver
            .update(
                &reviewers,
                "what-i-learned",
                KnowledgePatch {
                    body: Some("something else".into()),
                    ..KnowledgePatch::default()
                },
            )
            .unwrap_err(),
        solver.remove(&reviewers, "what-i-learned").unwrap_err(),
        solver.reembed(&reviewers, true).unwrap_err(),
        solver.delete_base(&reviewers).unwrap_err(),
    ] {
        assert!(matches!(err, Error::Denied { .. }), "{err:?}");
    }
    // …and the note is untouched.
    assert_eq!(
        store
            .get(&reviewers, "what-i-learned")
            .expect("get")
            .expect("present")
            .body,
        "the deploy needs a restart"
    );
}

#[test]
fn a_project_base_is_closed_to_everyone_outside_that_project() {
    let (_dir, store) = store();
    let acme = base("project:acme/runbook");
    store.ensure_base(&acme).expect("ensure");
    store
        .add(&acme, NewKnowledge::new("Secret", "internal"))
        .expect("add");

    let outsider = store.clone().as_agent("solver", Some("other"));
    assert!(matches!(
        outsider.get(&acme, "secret").unwrap_err(),
        Error::Denied { .. }
    ));
    assert!(matches!(
        outsider.search(&[acme.clone()], "internal", 5).unwrap_err(),
        Error::Denied { .. }
    ));
    assert!(outsider.visible_bases().expect("visible").is_empty());

    // An agent working in the project has it in full.
    let insider = store.clone().as_agent("solver", Some("acme"));
    assert!(insider.get(&acme, "secret").expect("get").is_some());
    assert!(
        insider
            .add(&acme, NewKnowledge::new("Another", "x"))
            .is_ok()
    );
}

#[test]
fn everybody_writes_the_global_base() {
    let (_dir, store) = store();
    let id = base("global/notes");
    store.ensure_base(&id).expect("ensure");
    for reader in [
        Reader::agent("solver", None),
        Reader::agent("reviewer", Some("acme")),
        Reader::project("acme"),
    ] {
        let label = reader.label();
        assert!(
            store
                .clone()
                .as_reader(reader)
                .add(&id, NewKnowledge::new(label.as_str(), "x"))
                .is_ok(),
            "{label} could not write the global base"
        );
    }
    assert_eq!(store.list(&id, &Filter::default()).expect("list").len(), 3);
}

#[test]
fn an_agents_memory_base_is_made_on_demand() {
    let (_dir, store) = store();
    let solver = store.clone().as_agent("solver", None);
    let memory = solver.ensure_memory("solver").expect("ensure memory");
    assert!(memory.is_memory());
    assert_eq!(memory.id.to_string(), "agent:solver/memory");
    assert!(
        solver
            .add(&memory.id, NewKnowledge::new("Learned", "x"))
            .is_ok()
    );
}

#[test]
fn an_agents_configured_bases_resolve_to_the_ones_it_may_actually_read() {
    let bases = resolve_agent_bases(
        "solver",
        Some("acme"),
        &[
            "global/notes".into(),
            "project:acme/runbook".into(),
            "project:other/runbook".into(), // another project's — dropped
            "agent:reviewer/memory".into(), // another agent's — kept, read-only
            "nonsense::".into(),            // unparseable — dropped
            "global/notes".into(),          // duplicate — dropped
        ],
        true,
    )
    .expect("resolve");

    let ids: Vec<String> = bases.iter().map(ToString::to_string).collect();
    assert_eq!(
        ids,
        vec![
            "agent:solver/memory",
            "global/notes",
            "project:acme/runbook",
            "agent:reviewer/memory",
        ],
        "own memory first, then the configured ones in order"
    );

    // With memory off, the agent's own base is not implied.
    let without =
        resolve_agent_bases("solver", None, &["global/notes".into()], false).expect("resolve");
    assert_eq!(without.len(), 1);
    assert_eq!(without[0].to_string(), "global/notes");
}

// -------------------------------------------------------- pluggable providers

#[test]
fn a_base_can_be_held_by_a_different_provider() {
    let (_dir, store) = store();
    let id = base("global/scratch");
    store
        .create_base(&id, Some(MEMORY), Some("ephemeral"), BTreeMap::new())
        .expect("create");
    assert_eq!(
        store
            .get_base(&id)
            .expect("get")
            .expect("base")
            .manifest
            .provider,
        MEMORY
    );

    let saved = store
        .add(&id, NewKnowledge::new("Held in a map", "body"))
        .expect("add");
    assert!(saved.embedded);
    assert_eq!(
        store
            .search(&[id.clone()], "held in a map", 5)
            .expect("search")
            .len(),
        1
    );

    // Nothing landed in the SQLite file the default provider would have made.
    assert!(!store.base_dir(&id).join("knowledge.db").exists());
}

/// The extension point, exercised the way a third crate would: a provider this one has never
/// heard of, registered from outside, holding a real base.
#[test]
fn a_provider_registered_from_outside_serves_a_base_like_any_other() {
    #[derive(Debug)]
    struct Recording {
        opened: AtomicUsize,
        inner: crate::backend::memory::MemoryProvider,
    }

    impl Provider for Recording {
        fn name(&self) -> &str {
            "recording"
        }
        fn description(&self) -> &str {
            "A memory base that counts how often it was opened."
        }
        fn open(&self, ctx: &BaseContext) -> crate::Result<Arc<dyn Backend>> {
            self.opened.fetch_add(1, Ordering::Relaxed);
            self.inner.open(ctx)
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let recording = Arc::new(Recording {
        opened: AtomicUsize::new(0),
        inner: crate::backend::memory::MemoryProvider::default(),
    });
    let mut providers = Providers::builtin();
    providers.register(recording.clone());
    assert!(providers.has("recording") && providers.has(SQLITE));

    let store = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_providers(Arc::new(providers))
        .with_embedder(Arc::new(HashEmbedder));
    let id = base("global/elsewhere");
    store
        .create_base(
            &id,
            Some("recording"),
            None,
            BTreeMap::from([("room".into(), "12".into())]),
        )
        .expect("create");

    store
        .add(&id, NewKnowledge::new("Stored elsewhere", "body"))
        .expect("add");
    assert_eq!(
        store
            .search(&[id.clone()], "stored elsewhere", 5)
            .expect("search")
            .len(),
        1
    );
    assert!(recording.opened.load(Ordering::Relaxed) > 0);

    // Provider settings survive the manifest round trip and reach the provider.
    let manifest = store.get_base(&id).expect("get").expect("base").manifest;
    assert_eq!(
        manifest.settings.get("room").map(String::as_str),
        Some("12")
    );
}

#[test]
fn a_base_whose_provider_is_missing_fails_loudly_rather_than_reading_as_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = base("global/elsewhere");
    KnowledgeStore::with_config(Config::with_root(dir.path()))
        .create_base(&id, Some(MEMORY), None, BTreeMap::new())
        .expect("create");

    // A build (or a machine) whose registry no longer has that provider.
    let stripped = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_providers(Arc::new(Providers::empty()));
    let err = stripped.list(&id, &Filter::default()).unwrap_err();
    assert!(matches!(err, Error::NoSuchProvider(_)), "{err:?}");
}

// ------------------------------------------------------------------ status

#[test]
fn status_counts_what_is_there_and_what_still_needs_embedding() {
    let (dir, _) = store();
    let broken = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(BrokenEmbedder));
    let id = base("global/notes");
    broken.ensure_base(&id).expect("ensure");
    broken.add(&id, NewKnowledge::new("A", "one")).expect("add");
    broken.add(&id, NewKnowledge::new("B", "two")).expect("add");

    let status = broken.base_status(&id).expect("status");
    assert_eq!((status.notes, status.embedded, status.stale), (2, 0, 2));

    let working = KnowledgeStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder));
    working.reembed(&id, false).expect("reembed");
    let status = working.base_status(&id).expect("status");
    assert_eq!((status.notes, status.embedded, status.stale), (2, 2, 0));
    assert_eq!(status.model.as_deref(), Some("hash-bow-256"));
}
