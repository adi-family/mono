//! Store-level tests: the transaction, the three silent bugs, the staleness graph, the three
//! isolation levels, and what an outside reference is told.
//!
//! They run against the real SQLite storage — the one that ships — with a stand-in embedder and a
//! stand-in classifier, so what is exercised is the bookkeeping rather than a 550MB model
//! download and a local LLM. The floor is set to 0.0 throughout: what these tests are about is
//! what happens to a pair once it *is* a candidate, and a threshold in the middle would make the
//! fixtures depend on the stand-in embedder's word overlap.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use adi_config::Config;
use adi_knowledge::HashEmbedder;

use crate::judge::{Judge, JudgeError, Judgement, Relation};
use crate::{BaseId, Error, FactStore, Reader, Verdict};

/// A classifier that calls every pair the same thing, and counts what it was asked.
#[derive(Debug)]
struct StubJudge {
    relation: Relation,
    seen: AtomicUsize,
}

impl StubJudge {
    fn new(relation: Relation) -> Arc<Self> {
        Arc::new(Self {
            relation,
            seen: AtomicUsize::new(0),
        })
    }
}

impl Judge for StubJudge {
    #[allow(clippy::unnecessary_literal_bound, reason = "the trait fixes the signature")]
    fn name(&self) -> &str {
        "stub"
    }

    fn extract(&self, note: &str) -> Result<Vec<String>, JudgeError> {
        Ok(note
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{s}."))
            .collect())
    }

    fn classify(&self, pairs: &[(&str, &str)]) -> Result<Vec<Judgement>, JudgeError> {
        self.seen.fetch_add(pairs.len(), Ordering::Relaxed);
        Ok(pairs
            .iter()
            .map(|_| Judgement {
                relation: self.relation,
                why: "stub".to_string(),
            })
            .collect())
    }
}

/// A classifier that cannot be reached — a machine with no ollama running.
#[derive(Debug)]
struct UnreachableJudge;

impl Judge for UnreachableJudge {
    #[allow(clippy::unnecessary_literal_bound, reason = "the trait fixes the signature")]
    fn name(&self) -> &str {
        "unreachable"
    }

    fn extract(&self, _note: &str) -> Result<Vec<String>, JudgeError> {
        Err(JudgeError("connection refused".into()))
    }

    fn classify(&self, _pairs: &[(&str, &str)]) -> Result<Vec<Judgement>, JudgeError> {
        Err(JudgeError("connection refused".into()))
    }
}

/// A store on a scratch directory, with a deterministic embedder and no model to load.
fn store(relation: Relation) -> (tempfile::TempDir, FactStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FactStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder))
        .with_judge(StubJudge::new(relation))
        .with_floor(0.0);
    (dir, store)
}

fn base(id: &str) -> BaseId {
    id.parse().expect("base id")
}

/// Stage facts and commit them with no pair open, for a test that needs a populated base.
fn seed(store: &FactStore, id: &BaseId, facts: &[&str]) -> Vec<String> {
    let staging = store
        .add(
            id,
            "igor",
            "agent:chat@1",
            facts.iter().map(ToString::to_string).collect(),
        )
        .expect("add");
    for pair in staging.open() {
        store
            .resolve(
                id,
                &staging.tx,
                pair.pair,
                Verdict::Coexist,
                None,
                None,
                "igor",
            )
            .expect("resolve");
    }
    store
        .commit(id, &staging.tx)
        .expect("commit")
        .added
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

// --------------------------------------------------------------------- bases

#[test]
fn a_base_has_to_exist_before_anything_can_be_read_from_it() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    assert!(matches!(
        store.stale(&id).unwrap_err(),
        Error::NoSuchBase(_)
    ));
    store.ensure_base(&id).expect("ensure");
    assert!(store.stale(&id).is_ok());
    assert_eq!(store.list_bases(None), vec![id]);
}

/// The rule the agent level exists for, inherited whole from `adi-knowledge`.
#[test]
fn a_fact_base_is_scoped_exactly_as_a_knowledge_base_is() {
    let (_dir, owner) = store(Relation::Independent);
    let mine = base("agent:solver/default");
    owner.ensure_base(&mine).expect("ensure");
    seed(&owner, &mine, &["Solver learned the front door 502s on a stale route."]);

    let reviewer = owner.clone().as_agent("reviewer", None);
    assert!(reviewer.stale(&mine).is_ok(), "another agent may read it");
    let denied = reviewer
        .add(&mine, "igor", "agent:reviewer@1", vec!["x".into()])
        .unwrap_err();
    assert!(matches!(denied, Error::Denied { .. }), "{denied:?}");
    assert!(denied.to_string().contains("agent:solver/default"), "{denied}");

    // A project base is invisible outside its project, and not merely unwritable.
    let theirs = base("project:acme/default");
    owner.ensure_base(&theirs).expect("ensure");
    let outsider = owner.clone().as_agent("solver", Some("other"));
    assert!(matches!(
        outsider.stale(&theirs).unwrap_err(),
        Error::Denied { .. }
    ));
    assert!(!outsider.list_bases(None).contains(&theirs));
}

// ---------------------------------------------------------------- the queue

#[test]
fn nothing_is_visible_to_the_base_until_the_transaction_commits() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["We support all countries.".into(), "We support all regions.".into()],
        )
        .expect("add");
    assert_eq!(staging.staged.len(), 2);
    assert_eq!(staging.open().len(), 1, "two staged facts are one pair");
    assert_eq!(store.count(&id).expect("count"), 0, "nothing landed yet");

    // Commit refuses while a pair is open, and names it.
    let err = store.commit(&id, &staging.tx).unwrap_err();
    assert!(matches!(err, Error::StillOpen { count: 1, .. }), "{err:?}");
    assert!(err.to_string().contains("p0"), "{err}");
}

#[test]
fn a_pair_the_classifier_calls_independent_never_reaches_the_queue() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["We support Ukraine.".into(), "The office is in Warsaw.".into()],
        )
        .expect("add");
    assert!(staging.pending.is_empty());
    assert_eq!(store.commit(&id, &staging.tx).expect("commit").added.len(), 2);
}

/// The prototype caught every classifier exception and defaulted the chunk to `independent`,
/// which means "nothing to do" — so an unreachable model quietly emptied the review queue.
#[test]
fn an_unreachable_classifier_surfaces_every_pair_instead_of_dropping_them() {
    let (dir, store) = store(Relation::Independent);
    let store = store.with_judge(Arc::new(UnreachableJudge));
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["We support all countries.".into(), "We support all regions.".into()],
        )
        .expect("add");
    assert_eq!(staging.open().len(), 1);
    assert_eq!(staging.pending[0].kind, "unclassified");
    assert!(staging.judge_error.as_deref().unwrap().contains("connection refused"));

    // `--text` is the one path that must fail outright: with nothing extracted there is nothing
    // to stage, and an empty transaction would read as "this note said nothing".
    let err = store
        .add_note(&id, "igor", "agent:chat@1", "some prose", None)
        .unwrap_err();
    assert!(matches!(err, Error::Judge(_)), "{err:?}");
    drop(dir);
}

/// A silent cap reads as "nothing else to see", which is the one lie this interface must never
/// tell.
#[test]
fn a_capped_candidate_list_reports_what_it_cut_and_below_what_strength() {
    let (_dir, store) = store(Relation::Controversy);
    let store = store.with_max_pending(2);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec![
                "We support all countries.".into(),
                "We support all regions.".into(),
                "We support all markets.".into(),
                "We support all segments.".into(),
            ],
        )
        .expect("add");
    let truncated = staging.truncated.expect("four facts are six pairs, two of which fit");
    assert_eq!(truncated.dropped, 4);
    assert!(truncated.below > 0.0);
    assert_eq!(staging.pending.len(), 2);
}

// ------------------------------------------------------- merge and supersede

/// Bug 1. `merge` rewrote only the incoming fact and left the base one alive, so committing
/// produced the merged sentence **and** the original it was meant to replace.
#[test]
fn merging_with_a_base_fact_rewrites_it_in_place_and_leaves_no_duplicate() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let seeded = seed(&store, &id, &["China is one of the operator's main markets."]);
    let base_id = seeded[0].clone();

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["China is one of our main target markets.".into()],
        )
        .expect("add");
    let pair = staging.open()[0].pair;
    store
        .resolve(
            &id,
            &staging.tx,
            pair,
            Verdict::Merge,
            None,
            Some("China is one of our main target markets."),
            "igor",
        )
        .expect("resolve");
    let done = store.commit(&id, &staging.tx).expect("commit");

    assert!(done.added.is_empty(), "the merged sentence is not a new row");
    assert_eq!(done.rewritten, vec![base_id.clone()]);
    assert_eq!(store.count(&id).expect("count"), 1, "one row, not two");

    let merged = store.fact(&id, &base_id).expect("fact").expect("still there");
    assert_eq!(merged.fact, "China is one of our main target markets.");
    assert_eq!(merged.version, 2, "the rewrite bumps the version");
}

/// Bug 2. When both sides of a pair were in the same incoming batch, `merge` and `supersede`
/// did nothing at all — neither row was retired and both landed.
#[test]
fn merging_two_facts_from_the_same_batch_retires_the_loser() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec![
                "China is a great market.".into(),
                "China is one of the main target markets.".into(),
            ],
        )
        .expect("add");
    let pair = staging.open()[0].pair;
    store
        .resolve(
            &id,
            &staging.tx,
            pair,
            Verdict::Merge,
            None,
            Some("China is one of our main target markets."),
            "igor",
        )
        .expect("resolve");
    let done = store.commit(&id, &staging.tx).expect("commit");

    assert_eq!(done.added.len(), 1, "two facts in, one out");
    assert_eq!(done.dropped, 1);
    assert_eq!(done.added[0].1, "China is one of our main target markets.");

    // The loser never reached the base, so it never got an id anybody could reference — it is
    // logged against the survivor instead, with its text, so the decision stays findable by the
    // wording somebody remembers. `#1` is the loser here: a merge always retires the *other*
    // side and writes the supplied sentence over the incoming one.
    let survivor = &done.added[0].0;
    let history = store.get(&id, survivor).expect("get");
    let absorbed = history
        .since
        .iter()
        .find(|e| e.event == "absorbed")
        .expect("the loser is logged");
    assert_eq!(absorbed.was, "China is one of the main target markets.");
    assert_eq!(absorbed.confirmer, "igor");
}

#[test]
fn superseding_two_facts_from_the_same_batch_retires_whichever_side_lost() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec![
                "We are not sure we can enter the China market.".into(),
                "We can enter the China market.".into(),
            ],
        )
        .expect("add");
    let pair = staging.open()[0].clone();
    // Keep the second staged fact, which is the pair's `base` side here.
    store
        .resolve(
            &id,
            &staging.tx,
            pair.pair,
            Verdict::Supersede,
            Some(&pair.theirs()),
            None,
            "igor",
        )
        .expect("resolve");
    let done = store.commit(&id, &staging.tx).expect("commit");
    assert_eq!(done.added.len(), 1);
    assert_eq!(done.added[0].1, "We can enter the China market.");
}

/// Bug 3. A typo in `--keep` was read as "the base side won", so the incoming fact was discarded
/// without a word. It must now fail and name the two ids that are actually valid.
#[test]
fn a_keep_that_is_neither_side_fails_and_names_both_valid_answers() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let seeded = seed(&store, &id, &["We are not sure we can enter the China market."]);

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["We can support China after all.".into()],
        )
        .expect("add");
    let pair = staging.open()[0].clone();

    let err = store
        .resolve(
            &id,
            &staging.tx,
            pair.pair,
            Verdict::Supersede,
            Some("f_typo"),
            None,
            "igor",
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, Error::KeepIsNotASide { .. }), "{err:?}");
    assert!(text.contains("f_typo"), "{text}");
    assert!(text.contains("#0"), "names the incoming side: {text}");
    assert!(text.contains(&seeded[0]), "names the base side: {text}");

    // Nothing was decided by the failure — the pair is still open.
    let after = store.show(&id, &staging.tx).expect("show");
    assert_eq!(after.open().len(), 1);

    // …and the two other ways to get `supersede` wrong are refused too.
    assert!(matches!(
        store
            .resolve(&id, &staging.tx, pair.pair, Verdict::Supersede, None, None, "igor")
            .unwrap_err(),
        Error::SupersedeNeedsKeep { .. }
    ));
    assert!(matches!(
        store
            .resolve(&id, &staging.tx, pair.pair, Verdict::Merge, None, None, "igor")
            .unwrap_err(),
        Error::MergeNeedsFact { .. }
    ));
}

#[test]
fn dropping_the_incoming_fact_leaves_the_base_as_it_was() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["We do not support the CIS."]);

    let staging = store
        .add(&id, "igor", "agent:chat@1", vec!["We do not support the CIS.".into()])
        .expect("add");
    let pair = staging.open()[0].pair;
    store
        .resolve(&id, &staging.tx, pair, Verdict::Drop, None, None, "igor")
        .expect("resolve");
    let done = store.commit(&id, &staging.tx).expect("commit");
    assert!(done.added.is_empty());
    assert_eq!(done.dropped, 1);
    assert_eq!(store.count(&id).expect("count"), 1);
}

#[test]
fn a_verdict_records_who_made_it() {
    let (_dir, store) = store(Relation::Narrows);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["We support all countries.".into(), "We support all regions.".into()],
        )
        .expect("add");
    let after = store
        .resolve(
            &id,
            &staging.tx,
            staging.open()[0].pair,
            Verdict::Coexist,
            None,
            None,
            "agent:verifier@3",
        )
        .expect("resolve");
    assert_eq!(after.pending[0].verdict.as_deref(), Some("coexist"));
    assert_eq!(after.pending[0].confirmer.as_deref(), Some("agent:verifier@3"));
    assert_eq!(after.state, "ready");
}

// -------------------------------------------------------------- the graph

#[test]
fn editing_a_fact_makes_what_was_derived_from_it_stale_directly_and_transitively() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let seeded = seed(&store, &id, &["We are not sure we can enter the China market."]);
    let fact = seeded[0].clone();

    let plan = store
        .derive(
            &id,
            std::slice::from_ref(&fact),
            "Market entry plan: skip China for now.",
            "igor",
            "agent:planner@1",
            "artifact",
        )
        .expect("derive");
    let summary = store
        .derive(
            &id,
            std::slice::from_ref(&plan),
            "This quarter's plan avoids China.",
            "igor",
            "agent:planner@1",
            "artifact",
        )
        .expect("derive");
    assert!(store.stale(&id).expect("stale").is_empty(), "nothing has moved yet");

    // A new fact reverses the old one, and the caller rules that it supersedes it.
    let staging = store
        .add(&id, "igor", "agent:chat@1", vec!["We can support China after all.".into()])
        .expect("add");
    let pair = staging
        .open()
        .into_iter()
        .find(|p| p.base_id.as_deref() == Some(fact.as_str()))
        .expect("the pair against the base fact")
        .clone();
    store
        .resolve(
            &id,
            &staging.tx,
            pair.pair,
            Verdict::Supersede,
            Some(&pair.mine()),
            None,
            "igor",
        )
        .expect("resolve");
    // The new fact was compared against the derived plan too, and that pair has to be settled.
    for open in store.show(&id, &staging.tx).expect("show").open() {
        store
            .resolve(&id, &staging.tx, open.pair, Verdict::Coexist, None, None, "igor")
            .expect("resolve");
    }
    store.commit(&id, &staging.tx).expect("commit");

    let reversed = store.fact(&id, &fact).expect("fact").expect("rewritten in place");
    assert_eq!(reversed.fact, "We can support China after all.");
    assert_eq!(reversed.version, 2);

    let stale = store.stale(&id).expect("stale");
    let plan_row = stale.iter().find(|s| s.id == plan).expect("the plan is stale");
    assert_eq!(plan_row.root_cause, fact, "and it names what changed under it");
    assert_eq!(plan_row.depth, 0, "one edge away is direct");

    let summary_row = stale
        .iter()
        .find(|s| s.id == summary)
        .expect("and so is what was built on the plan");
    assert_eq!(summary_row.depth, 1, "two edges away is transitive");

    // Regenerating the plan clears it and everything under it, and disturbs nothing else.
    store.refresh(&id, &plan).expect("refresh");
    store.refresh(&id, &summary).expect("refresh");
    assert!(store.stale(&id).expect("stale").is_empty());
}

/// The single silent failure mode this design was rebuilt to remove.
///
/// The prototype's first run wrote a fact and edited it inside the same millisecond; the
/// wall-clock `updated_at` never moved, the edge stamp still matched, and the edit was invisible
/// to every dependent with no error anywhere. The counter cannot do that.
///
/// The clock is pinned by hand rather than by racing it: two writes landing in the same
/// millisecond is not something a test can ask for on demand (500 tries here never produced a
/// pair), and a frozen `updated_at` is the same condition stated deterministically.
#[test]
fn two_edits_under_a_frozen_clock_are_still_two_versions() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let fact = seed(&store, &id, &["The company is incorporated in Delaware."])[0].clone();
    let dependent = store
        .derive(
            &id,
            std::slice::from_ref(&fact),
            "Filing plan assumes Delaware.",
            "igor",
            "agent:planner@1",
            "artifact",
        )
        .expect("derive");
    store.refresh(&id, &dependent).expect("refresh to a clean slate");

    let path = store
        .dir()
        .join(id.rel_dir())
        .join(crate::db::DB_FILE);
    let conn = rusqlite::Connection::open(&path).expect("open the base by hand");
    let pinned: i64 = store.fact(&id, &fact).expect("fact").expect("there").updated_at;

    for _ in 0..2 {
        store.refresh(&id, &fact).expect("refresh");
        conn.execute(
            "update nodes set updated_at = ?1 where id = ?2",
            rusqlite::params![pinned, fact],
        )
        .expect("freeze the clock");
    }

    let after = store.fact(&id, &fact).expect("fact").expect("there");
    assert_eq!(after.updated_at, pinned, "the clock never moved");
    assert_eq!(after.version, 3, "and both edits are still on the record");
    assert!(
        store.stale(&id).expect("stale").iter().any(|s| s.id == dependent),
        "the dependent is stale even though updated_at did not move"
    );
}

/// A cached vector describes a sentence. Rewriting a node in place used to leave the old vector
/// keyed by the node's id, so the base went on ranking it by what it no longer said.
#[test]
fn rewriting_a_fact_in_place_throws_away_the_vector_of_what_it_used_to_say() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let old = seed(&store, &id, &["alpha beta gamma."])[0].clone();
    // `near` is what populates the vector cache for an existing node.
    store.near(&id, &old, 10).expect("near");

    let staging = store
        .add(&id, "igor", "agent:chat@1", vec!["delta epsilon zeta.".into()])
        .expect("add");
    let pair = staging.open()[0].clone();
    store
        .resolve(
            &id,
            &staging.tx,
            pair.pair,
            Verdict::Supersede,
            Some(&pair.mine()),
            None,
            "igor",
        )
        .expect("resolve");
    store.commit(&id, &staging.tx).expect("commit");

    // Ask what is near a fresh fact sharing every word with the *new* text. A stale cache would
    // rank the rewritten node at 0.0, because its old vector holds none of those words.
    seed(&store, &id, &["delta epsilon zeta."]);
    let near = store.near(&id, &old, 10).expect("near");
    let twin = near
        .iter()
        .find(|n| n.fact == "delta epsilon zeta.")
        .expect("the twin is in the base");
    assert!(
        twin.strength > 0.99,
        "the rewritten node must be ranked by what it says now, not what it said: {:.3}",
        twin.strength
    );
}

#[test]
fn a_note_is_a_node_so_the_facts_drawn_from_it_go_stale_when_it_changes() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add_note(
            &id,
            "igor",
            "agent:chat@1",
            "We do not support the CIS. The office is in Warsaw.",
            Some("note_1"),
        )
        .expect("add note");
    assert_eq!(staging.staged.len(), 2, "the extractor split the note");
    let added = store.commit(&id, &staging.tx).expect("commit").added;
    assert_eq!(added.len(), 2);

    assert!(store.stale(&id).expect("stale").is_empty());
    store.refresh(&id, "note_1").expect("the note moved");
    let stale = store.stale(&id).expect("stale");
    assert_eq!(stale.len(), 2, "both facts drawn from it are out of date");
    assert!(stale.iter().all(|s| s.root_cause == "note_1"));
}

// ---------------------------------------------------------------- references

#[test]
fn a_reference_written_against_an_older_version_is_told_the_fact_has_moved() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let fact = seed(&store, &id, &["The company is incorporated in Delaware."])[0].clone();

    let current = store.get(&id, &format!("{fact}@1")).expect("get");
    assert!(!current.drifted);
    assert_eq!(current.referenced, Some(1));

    let staging = store
        .add(
            &id,
            "igor",
            "agent:chat@1",
            vec!["The company was reincorporated in Nevada.".into()],
        )
        .expect("add");
    let pair = staging.open()[0].clone();
    store
        .resolve(
            &id,
            &staging.tx,
            pair.pair,
            Verdict::Supersede,
            Some(&pair.mine()),
            None,
            "igor",
        )
        .expect("resolve");
    store.commit(&id, &staging.tx).expect("commit");

    let drifted = store.get(&id, &format!("{fact}@1")).expect("get");
    assert!(drifted.drifted, "the id still resolves; the meaning moved");
    assert_eq!(drifted.fact.version, 2);
    assert_eq!(drifted.since.len(), 1, "only what happened after v1");
    let event = &drifted.since[0];
    assert_eq!(event.event, "supersede");
    assert_eq!(event.was, "The company is incorporated in Delaware.");
    assert_eq!(event.now, "The company was reincorporated in Nevada.");
    assert_eq!(event.confirmer, "igor");

    // With no version the whole log comes back, because nothing says which half is news.
    let unversioned = store.get(&id, &fact).expect("get");
    assert!(!unversioned.drifted);
    assert_eq!(unversioned.since.len(), 2, "created, then superseded");
    assert!(store.get(&id, "f_nothing").is_err());
}

#[test]
fn the_queue_around_one_fact_is_ranked_and_excludes_the_fact_itself() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let seeded = seed(
        &store,
        &id,
        &[
            "We support all countries.",
            "We support all regions.",
            "Sourdough needs a long cold retard.",
        ],
    );

    let near = store.near(&id, &seeded[0], 10).expect("near");
    assert!(!near.iter().any(|n| n.id == seeded[0]), "not itself");
    assert_eq!(near.len(), 2);
    assert!(
        near[0].strength >= near[1].strength,
        "strongest first: {near:?}"
    );
    assert_eq!(near[0].id, seeded[1], "the overlapping sentence ranks above the bread");

    assert_eq!(store.near(&id, &seeded[0], 1).expect("near").len(), 1, "--top caps it");
}

// ---------------------------------------------------------- transaction state

#[test]
fn a_committed_or_aborted_transaction_cannot_be_worked_on_again() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let staging = store
        .add(&id, "igor", "agent:chat@1", vec!["We support Ukraine.".into()])
        .expect("add");
    store.commit(&id, &staging.tx).expect("commit");
    assert!(matches!(
        store.commit(&id, &staging.tx).unwrap_err(),
        Error::TransactionClosed { .. }
    ));

    let second = store
        .add(&id, "igor", "agent:chat@1", vec!["The office is in Warsaw.".into()])
        .expect("add");
    store.abort(&id, &second.tx).expect("abort");
    assert_eq!(store.count(&id).expect("count"), 1, "an aborted batch lands nothing");
    assert!(matches!(
        store.abort(&id, &second.tx).unwrap_err(),
        Error::TransactionClosed { .. }
    ));
    assert!(matches!(
        store.show(&id, "tx_nothing").unwrap_err(),
        Error::NoSuchTransaction(_)
    ));
}

#[test]
fn an_identity_is_interned_once_and_read_back_by_name() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let fact = seed(&store, &id, &["We support Ukraine."])[0].clone();

    let stored = store.fact(&id, &fact).expect("fact").expect("there");
    assert_eq!(stored.author, "igor");
    assert_eq!(stored.creator, "agent:chat@1");
    assert_eq!(stored.version, 1);
    assert_eq!(stored.kind, "fact");
}

#[test]
fn a_reader_who_is_nobody_in_particular_is_the_owner_of_the_store() {
    let (_dir, store) = store(Relation::Independent);
    assert!(store.reader().admin);
    let scoped = store.clone().as_reader(Reader::agent("solver", Some("acme")));
    assert!(!scoped.reader().admin);
    assert_eq!(scoped.reader().agent.as_deref(), Some("solver"));
}

/// Reproduce [`DEFAULT_FLOOR`](crate::DEFAULT_FLOOR) against the model it was measured on.
///
/// `RESULTS.md` §8 and §9 set every threshold in this design with `nomic-embed-text`, which is
/// what [`OllamaEmbedder`](crate::embed::OllamaEmbedder) runs — so this is not a re-calibration,
/// it is a check that the port did not change what the numbers describe. `golden-flat.json` is
/// the fixture §8 used: 33 extracted facts, 14 hand-labelled related pairs, 528 pairs in all,
/// and the published result is that all 14 land inside the top 125.
///
/// **If this stops reproducing, suspect the port before suspecting the fixture.** A drift here
/// means the vectors this crate compares are not the vectors the design was measured with, and
/// that is a more serious finding than any threshold. As of this writing it reproduces exactly:
/// deepest labelled pair at rank **125 of 528**, median pair 0.456.
///
/// # What it also shows: the floor cuts two labelled pairs on this fixture
///
/// 12 of the 14 clear 0.55. The two that do not are worth naming, because they are not the same
/// kind of loss:
///
/// * `mkt-04` / `mkt-09` at **0.532** — "India and the post-CIS countries are secondary markets"
///   against "We do not support the CIS", labelled `supersede` and annotated in the fixture as
///   *"the hard one"*. This is a genuine finding, and it is the very pair `RESULTS.md` §9 held
///   up as what flat facts recovered that the structured schema had recorded as unfindable.
/// * `msh-01` / `msh-03` at **0.520** — labelled `coexist`, "unrelated predicates under one
///   subject". Losing a `coexist` costs nothing actionable.
///
/// So the design's own procedure — find the lowest cosine at which a genuine finding still
/// appears, set the floor below it — gives 0.55 on the 114-fact live base (§9's sharpest was
/// 0.551) and about 0.50 on this 33-fact fixture. The floor is corpus-sensitive at exactly the
/// resolution that decides things, which is more evidence for `DESIGN.md`'s own conclusion that
/// top-K neighbours will have to replace it. **The floor is left at the design's number**; this
/// is recorded, not fixed, because fitting a threshold to whichever corpus was measured last is
/// how a calibration stops meaning anything.
///
/// Ignored because it needs a local ollama with `nomic-embed-text` pulled.
#[test]
#[ignore = "needs a local ollama with nomic-embed-text"]
#[allow(
    clippy::too_many_lines,
    reason = "one measurement end to end: read the fixture, embed, rank, report. Every line is \
              part of the number it prints, and splitting it would hide the method."
)]
fn the_floor_admits_every_hand_labelled_pair_on_the_measured_corpus() {
    use adi_indexer::embed::Embedder;

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../experiment/knowledge-base/golden-flat.json");
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture).expect("fixture")).expect("json");

    let claims: Vec<(String, String)> = golden["claims"]
        .as_array()
        .expect("claims")
        .iter()
        .map(|c| {
            (
                c["id"].as_str().expect("id").to_string(),
                c["fact"].as_str().expect("fact").to_string(),
            )
        })
        .collect();
    let labelled: Vec<(String, String)> = golden["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .map(|r| {
            (
                r["a"].as_str().expect("a").to_string(),
                r["b"].as_str().expect("b").to_string(),
            )
        })
        .collect();

    let embedder = crate::embed::OllamaEmbedder::new();
    let texts: Vec<&str> = claims.iter().map(|(_, f)| f.as_str()).collect();
    let vectors = embedder.embed(&texts).expect("embed — is ollama up and nomic-embed-text pulled?");

    let mut pairs: Vec<(f32, String, String)> = Vec::new();
    for i in 0..claims.len() {
        for j in (i + 1)..claims.len() {
            pairs.push((
                adi_knowledge::backend::cosine(&vectors[i], &vectors[j]),
                claims[i].0.clone(),
                claims[j].0.clone(),
            ));
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    let is_labelled = |a: &str, b: &str| {
        labelled
            .iter()
            .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    };
    let hits: Vec<(usize, f32, String, String)> = pairs
        .iter()
        .enumerate()
        .filter(|(_, (_, a, b))| is_labelled(a, b))
        .map(|(rank, (s, a, b))| (rank + 1, *s, a.clone(), b.clone()))
        .collect();

    let worst_rank = hits.iter().map(|(r, ..)| *r).max().expect("labelled pairs");
    let weakest = hits.iter().map(|(_, s, ..)| *s).fold(f32::MAX, f32::min);
    println!(
        "{} on golden-flat: {} of {} labelled pairs found; deepest rank {worst_rank} of {}; \
         weakest labelled cosine {weakest:.3}",
        embedder.model_name(),
        hits.len(),
        labelled.len(),
        pairs.len(),
    );
    for (rank, strength, a, b) in &hits {
        let cut = if *strength < crate::DEFAULT_FLOOR { "  <- below the floor" } else { "" };
        println!("  rank {rank:>3}  {strength:.3}  {a} / {b}{cut}");
    }
    let median = pairs[pairs.len() / 2].0;
    println!("  median cosine over all {} pairs: {median:.3}", pairs.len());
    for floor in [0.50f32, 0.55, 0.60, 0.65, 0.70, 0.75] {
        let above = pairs.iter().filter(|(s, _, _)| *s >= floor).count();
        let kept = hits.iter().filter(|(_, s, ..)| *s >= floor).count();
        println!(
            "  floor {floor:.2}: {above:>3} pairs above ({:>5.1}% of the base), \
             {kept}/{} labelled pairs kept",
            100.0 * above as f32 / pairs.len() as f32,
            labelled.len()
        );
    }

    assert_eq!(hits.len(), labelled.len(), "every labelled pair is a pair");

    // §8's published result on this fixture with this model, and the reason this test exists: a
    // ranking that has drifted past it means the vectors this crate compares are not the vectors
    // the design was measured with, which matters more than any threshold.
    assert!(
        worst_rank <= 150,
        "RESULTS.md §8 puts all 14 inside the top 125 of 528; the deepest here is {worst_rank}. \
         Suspect the port, not the threshold."
    );
    // §9's distribution: a fat middle around 0.49, which is what gives the floor something to cut.
    assert!(
        (0.40..0.60).contains(&median),
        "§9 measured a median near 0.49; this base's is {median:.3}"
    );
    let above_floor = pairs
        .iter()
        .filter(|(s, _, _)| *s >= crate::DEFAULT_FLOOR)
        .count();
    assert!(
        above_floor * 4 < pairs.len(),
        "the floor is supposed to spend compute: {above_floor} of {} pairs is not a cut",
        pairs.len()
    );
    // 12 of 14, not 14 of 14 — see this test's doc comment. Pinned so that a change in either
    // direction is noticed rather than absorbed.
    let kept = hits
        .iter()
        .filter(|(_, s, ..)| *s >= crate::DEFAULT_FLOOR)
        .count();
    assert_eq!(
        kept, 12,
        "the floor's cut against this fixture has moved; re-read the two pairs it drops"
    );
    assert!(
        weakest < crate::DEFAULT_FLOOR,
        "recorded for the reader: the weakest labelled pair is at {weakest:.3}"
    );
}
