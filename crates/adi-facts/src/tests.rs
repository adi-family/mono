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

use crate::judge::{Judge, JudgeError, Judgement, Relation, Side};
use crate::{BaseId, Error, FactStore, Incoming, Reader, Verdict};

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

    fn classify(&self, pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError> {
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

    fn classify(&self, _pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError> {
        Err(JudgeError("connection refused".into()))
    }
}

/// A store on a scratch directory, with a deterministic embedder and no model to load.
fn store(relation: Relation) -> (tempfile::TempDir, FactStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FactStore::with_config(Config::with_root(dir.path()))
        .with_embedder(Arc::new(HashEmbedder))
        .with_judge(StubJudge::new(relation));
    (dir, store)
}

fn base(id: &str) -> BaseId {
    id.parse().expect("base id")
}

/// The identities every test writes under, so a call site says what it is testing and not who.
fn writer() -> Incoming {
    Incoming::new("igor", "agent:chat@1")
}

/// Stage facts and commit them with no pair open, for a test that needs a populated base.
fn seed(store: &FactStore, id: &BaseId, facts: &[&str]) -> Vec<String> {
    let staging = store
        .add(
            id,
            &writer(),
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

/// Stage one derived artifact from `sources` and land it, ruling `coexist` on whatever it
/// raises.
///
/// There is no shortcut into the base for this — a derived node goes through exactly the
/// transaction a stated fact does, which is the point (`a_derived_write_is_checked_like_any_other`).
fn derive(store: &FactStore, id: &BaseId, sources: &[String], fact: &str) -> String {
    let incoming = Incoming::new("igor", "agent:planner@1")
        .from_sources(sources.to_vec())
        .as_artifact();
    let staging = store.add(id, &incoming, vec![fact.to_string()]).expect("derive");
    for pair in staging.open() {
        store
            .resolve(id, &staging.tx, pair.pair, Verdict::Coexist, None, None, "igor")
            .expect("resolve");
    }
    let done = store.commit(id, &staging.tx).expect("commit");
    done.added.first().expect("the artifact landed").0.clone()
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
        .add(&mine, &Incoming::new("igor", "agent:reviewer@1"), vec!["x".into()])
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
            &writer(),
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
            &writer(),
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
            &writer(),
            vec!["We support all countries.".into(), "We support all regions.".into()],
        )
        .expect("add");
    assert_eq!(staging.open().len(), 1);
    assert_eq!(staging.pending[0].kind, "unclassified");
    assert!(staging.judge_error.as_deref().unwrap().contains("connection refused"));

    // `--text` is the one path that must fail outright: with nothing extracted there is nothing
    // to stage, and an empty transaction would read as "this note said nothing".
    let err = store
        .add_note(&id, &writer(), "some prose", None)
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
            &writer(),
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
            &writer(),
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
            &writer(),
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
            &writer(),
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
            &writer(),
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
        .add(&id, &writer(), vec!["We do not support the CIS.".into()])
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
            &writer(),
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

    let plan = derive(
        &store,
        &id,
        std::slice::from_ref(&fact),
        "Market entry plan: skip China for now.",
    );
    let summary = derive(
        &store,
        &id,
        std::slice::from_ref(&plan),
        "This quarter's plan avoids China.",
    );
    assert!(store.stale(&id).expect("stale").is_empty(), "nothing has moved yet");

    // A new fact reverses the old one, and the caller rules that it supersedes it.
    let staging = store
        .add(&id, &writer(), vec!["We can support China after all.".into()])
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
    let dependent = derive(
        &store,
        &id,
        std::slice::from_ref(&fact),
        "Filing plan assumes Delaware.",
    );
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
        .add(&id, &writer(), vec!["delta epsilon zeta.".into()])
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
            &writer(),
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
            &writer(),
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
        .add(&id, &writer(), vec!["We support Ukraine.".into()])
        .expect("add");
    store.commit(&id, &staging.tx).expect("commit");
    assert!(matches!(
        store.commit(&id, &staging.tx).unwrap_err(),
        Error::TransactionClosed { .. }
    ));

    let second = store
        .add(&id, &writer(), vec!["The office is in Warsaw.".into()])
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

/// Recall against `golden-flat.json` at a range of K, and a check that the port still compares
/// the vectors the design was measured with.
///
/// `RESULTS.md` §8 published the ranking result for this fixture with `nomic-embed-text` — 33
/// extracted facts, 528 pairs, 14 hand-labelled as related, all 14 inside the top 125. That is
/// what the assertion guards: **a drift there means these are not the design's vectors**, which
/// matters more than any selection parameter.
///
/// The printed table is the reason the test exists in this shape. It used to report what a series
/// of *floors* admitted; the floor is gone, so it reports what a series of **K** catches.
///
/// Read it knowing what it cannot show. **This fixture is too small to choose K**: 33 facts means
/// K=20 already reaches most of the base, so every K from 5 up catches all 14 labelled pairs and
/// what actually varies is cost — K=5 selects 20% of all pairs, K=30 selects 98%. The knee that
/// picked K=20 (96 / 108 / 112 actionable at K=10 / 20 / 30) was measured on the 114-fact live
/// base, which is not in this tree. What this test *does* prove is the ranking underneath: the
/// deepest labelled pair at rank 125 of 528 is §8's published number, and a drift there means the
/// vectors are wrong, which is worth more than any parameter.
///
/// Ignored because it needs a local ollama with `nomic-embed-text` pulled.
#[test]
#[ignore = "needs a local ollama with nomic-embed-text"]
#[allow(
    clippy::too_many_lines,
    reason = "one measurement end to end: read the fixture, embed, rank, report. Every line is \
              part of the number it prints, and splitting it would hide the method."
)]
fn top_k_recall_on_the_measured_corpus() {
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
    let vectors = embedder
        .embed(&texts)
        .expect("embed — is ollama up and nomic-embed-text pulled?");

    let n = claims.len();
    let sim = |i: usize, j: usize| adi_knowledge::backend::cosine(&vectors[i], &vectors[j]);
    let index_of = |id: &str| claims.iter().position(|(c, _)| c == id).expect("claim id");
    let labelled_pairs: Vec<(usize, usize)> = labelled
        .iter()
        .map(|(a, b)| (index_of(a), index_of(b)))
        .collect();

    // The §8 result: where the labelled pairs sit in a global ranking of all 528.
    let mut all: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            all.push((sim(i, j), i, j));
        }
    }
    all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let deepest = labelled_pairs
        .iter()
        .map(|(a, b)| {
            all.iter()
                .position(|(_, i, j)| (i, j) == (a, b) || (i, j) == (b, a))
                .expect("every labelled pair is a pair")
                + 1
        })
        .max()
        .expect("labelled pairs");
    println!(
        "{} on golden-flat: {} labelled pairs, {} pairs in all; deepest labelled at rank {deepest}",
        embedder.model_name(),
        labelled.len(),
        all.len()
    );

    // Symmetric top-K: a pair is selected when either side holds the other.
    let top_k_of = |i: usize, k: usize| -> Vec<usize> {
        let mut ranked: Vec<(f32, usize)> =
            (0..n).filter(|j| *j != i).map(|j| (sim(i, j), j)).collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then_with(|| a.1.cmp(&b.1)));
        ranked.truncate(k);
        ranked.into_iter().map(|(_, j)| j).collect()
    };
    for k in [5usize, 10, 20, 30] {
        let neighbours: Vec<Vec<usize>> = (0..n).map(|i| top_k_of(i, k)).collect();
        let selected = |a: usize, b: usize| neighbours[a].contains(&b) || neighbours[b].contains(&a);
        let caught = labelled_pairs.iter().filter(|(a, b)| selected(*a, *b)).count();
        let total: usize = (0..n)
            .flat_map(|i| ((i + 1)..n).map(move |j| (i, j)))
            .filter(|(i, j)| selected(*i, *j))
            .count();
        println!(
            "  K={k:<3} {caught}/{} labelled caught, {total} of {} pairs selected ({:.0}%)",
            labelled.len(),
            all.len(),
            100.0 * total as f32 / all.len() as f32
        );
    }

    // §8's published result, and the only thing here that is an assertion rather than a report.
    assert!(
        deepest <= 150,
        "RESULTS.md §8 puts all 14 inside the top 125 of 528; the deepest here is {deepest}. \
         Suspect the port, not the parameter."
    );
    // K=20 is the shipped default; on this fixture it must still reach most of the labelled set.
    let neighbours: Vec<Vec<usize>> = (0..n).map(|i| top_k_of(i, crate::DEFAULT_TOP_K)).collect();
    let caught = labelled_pairs
        .iter()
        .filter(|(a, b)| neighbours[*a].contains(b) || neighbours[*b].contains(a))
        .count();
    assert!(
        caught * 10 >= labelled.len() * 8,
        "K={} caught only {caught} of {}",
        crate::DEFAULT_TOP_K,
        labelled.len()
    );
}

// ------------------------------------------------------- one door into the base

/// The bug this section exists for, inverted into an assertion.
///
/// `derive` used to write a node and its edges straight into the base: no transaction, no
/// neighbour scan, no pair. A conclusion that flatly contradicted a committed fact landed beside
/// it and nothing said a word — `transactions` and `pending` were untouched. Two doors with
/// different safety, and an agent that wanted provenance was forced through the unlocked one.
#[test]
fn a_derived_write_is_checked_like_any_other_and_cannot_land_unresolved() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let stated = seed(&store, &id, &["The USA is our main market."])[0].clone();

    let conclusion = Incoming::new("igor", "agent:planner@1")
        .from_sources(vec![stated.clone()])
        .as_artifact();
    let staging = store
        .add(
            &id,
            &conclusion,
            vec!["The USA is not our market; we focus on the EU.".into()],
        )
        .expect("add");

    // The contradiction is raised, not written.
    let pair = staging
        .open()
        .into_iter()
        .find(|p| p.base_id.as_deref() == Some(stated.as_str()))
        .expect("the conclusion is checked against the fact it was drawn from")
        .clone();
    assert_eq!(pair.kind, "controversy");
    assert_eq!(store.count(&id).expect("count"), 1, "nothing landed yet");

    // …and it cannot be committed around.
    let err = store.commit(&id, &staging.tx).unwrap_err();
    assert!(matches!(err, Error::StillOpen { count: 1, .. }), "{err:?}");
    assert_eq!(store.count(&id).expect("count"), 1);

    // Ruled on, it lands — with the edge that makes it stale later.
    store
        .resolve(&id, &staging.tx, pair.pair, Verdict::Coexist, None, None, "igor")
        .expect("resolve");
    let done = store.commit(&id, &staging.tx).expect("commit");
    assert_eq!(done.added.len(), 1);
    assert_eq!(done.linked, 1, "one source, one edge");
    assert!(
        done.added[0].0.starts_with("d_"),
        "an artifact is named so a stale report says at a glance who wrote it: {}",
        done.added[0].0
    );
}

#[test]
fn provenance_and_staleness_are_the_same_edge() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let premise = seed(&store, &id, &["The company is incorporated in Delaware."])[0].clone();

    let conclusion = derive(
        &store,
        &id,
        std::slice::from_ref(&premise),
        "Filing plan assumes Delaware.",
    );
    assert!(store.stale(&id).expect("stale").is_empty());

    // The premise moves; the conclusion drawn from it is out of date, naming what changed.
    store.refresh(&id, &premise).expect("refresh");
    let stale = store.stale(&id).expect("stale");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].id, conclusion);
    assert_eq!(stale[0].root_cause, premise);
}

/// A source that moved while the transaction was open must be stamped at the version it has when
/// the batch lands, not the one it had when the caller started typing — otherwise the derived
/// node is born claiming to be current against text it never saw.
#[test]
fn an_edge_is_stamped_at_the_sources_version_at_commit_not_at_staging() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    let premise = seed(&store, &id, &["The company is incorporated in Delaware."])[0].clone();

    let conclusion = Incoming::new("igor", "agent:planner@1")
        .from_sources(vec![premise.clone()])
        .as_artifact();
    let staging = store
        .add(&id, &conclusion, vec!["Filing plan assumes Delaware.".into()])
        .expect("add");

    // Somebody edits the premise while this transaction sits open.
    store.refresh(&id, &premise).expect("refresh");
    assert_eq!(
        store.fact(&id, &premise).expect("fact").expect("there").version,
        2
    );

    for pair in store.show(&id, &staging.tx).expect("show").open() {
        store
            .resolve(&id, &staging.tx, pair.pair, Verdict::Coexist, None, None, "igor")
            .expect("resolve");
    }
    store.commit(&id, &staging.tx).expect("commit");

    // Stamped at v2, so nothing is stale: the plan was written against what the premise says now.
    assert!(
        store.stale(&id).expect("stale").is_empty(),
        "an edge stamped at the staging-time version would report a node stale the moment it landed"
    );
    // And it still goes stale on the next edit, which is what proves the edge is live.
    store.refresh(&id, &premise).expect("refresh");
    assert_eq!(store.stale(&id).expect("stale").len(), 1);
}

#[test]
fn a_from_that_names_nothing_fails_and_says_which_id() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["The company is incorporated in Delaware."]);

    let bad = Incoming::new("igor", "agent:planner@1").from_sources(vec!["f_typo".into()]);
    let err = store
        .add(&id, &bad, vec!["Filing plan assumes Delaware.".into()])
        .unwrap_err();
    assert!(matches!(err, Error::NoSuchFact(_)), "{err:?}");
    assert!(err.to_string().contains("f_typo"), "{err}");
    // The batch was refused outright, not staged with an edge quietly missing.
    assert_eq!(store.count(&id).expect("count"), 1);

    // `#N` out of range is the same refusal: there is no such staged fact to derive from.
    let out_of_range = Incoming::new("igor", "agent:planner@1").from_sources(vec!["#7".into()]);
    let err = store
        .add(&id, &out_of_range, vec!["A conclusion.".into()])
        .unwrap_err();
    assert!(matches!(err, Error::BadSource(_)), "{err:?}");
    assert!(err.to_string().contains("#7"), "{err}");
}

/// `--from #N` names a fact staged in this same batch — how an agent writes premises and the
/// conclusion drawn from them in one call. If a verdict then throws that premise away, the edge
/// has nothing to point at, and writing no edge would leave a conclusion that never goes stale.
#[test]
fn a_from_pointing_at_a_fact_this_batch_dropped_fails_rather_than_dangling() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["The company is incorporated in Delaware."]);

    let batch = Incoming::new("igor", "agent:planner@1").from_sources(vec!["#0".into()]);
    let staging = store
        .add(
            &id,
            &batch,
            vec![
                "The company is incorporated in Delaware.".into(),
                "Filing plan assumes Delaware.".into(),
            ],
        )
        .expect("add");

    // Rule the duplicate against the base as a `drop`: the premise never lands.
    let against_base = staging
        .open()
        .into_iter()
        .find(|p| p.new_seq == 0 && p.base_id.is_some())
        .expect("#0 duplicates the committed fact")
        .clone();
    store
        .resolve(&id, &staging.tx, against_base.pair, Verdict::Drop, None, None, "igor")
        .expect("resolve");
    for pair in store.show(&id, &staging.tx).expect("show").open() {
        store
            .resolve(&id, &staging.tx, pair.pair, Verdict::Coexist, None, None, "igor")
            .expect("resolve");
    }

    let err = store.commit(&id, &staging.tx).unwrap_err();
    assert!(matches!(err, Error::SourceDropped(_)), "{err:?}");
    assert!(err.to_string().contains("#0"), "{err}");
    assert!(err.to_string().contains("tx show"), "it says what to do next: {err}");
}

/// A batch that derives from one of its own members: everything else links to it, and it does not
/// link to itself.
#[test]
fn a_fact_never_derives_from_itself() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    let batch = Incoming::new("igor", "agent:chat@1").from_sources(vec!["#0".into()]);
    let staging = store
        .add(
            &id,
            &batch,
            vec![
                "The company is incorporated in Delaware.".into(),
                "Filing plan assumes Delaware.".into(),
                "The registered agent is in Wilmington.".into(),
            ],
        )
        .expect("add");
    for pair in staging.open() {
        store
            .resolve(&id, &staging.tx, pair.pair, Verdict::Coexist, None, None, "igor")
            .expect("resolve");
    }
    let done = store.commit(&id, &staging.tx).expect("commit");
    assert_eq!(done.added.len(), 3);
    assert_eq!(done.linked, 2, "two dependents, and no self-edge");

    let premise = done.added[0].0.clone();
    store.refresh(&id, &premise).expect("refresh");
    let stale = store.stale(&id).expect("stale");
    assert_eq!(stale.len(), 2, "the other two are stale");
    assert!(
        !stale.iter().any(|s| s.id == premise),
        "and the premise did not make itself stale"
    );
}

// ------------------------------------------------------------------- reading

/// Neither `search` nor `near` cuts anything for scoring low. An answer of "nothing found" about
/// a base that plainly holds something closest is not one a caller can act on — and under the old
/// similarity floor both commands could give it.
#[test]
fn search_and_near_answer_with_the_best_the_base_has_however_weak() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(
        &store,
        &id,
        &[
            "China pricing is set per seat.",
            "Sourdough needs a long cold retard.",
        ],
    );

    let hits = store.search(&id, "china pricing", 5).expect("search");
    assert_eq!(hits.len(), 2, "every fact is ranked, none is cut");
    assert_eq!(hits[0].fact, "China pricing is set per seat.");
    assert!(
        hits[0].strength > hits[1].strength,
        "and the caller sees the scores, to judge a weak match: {hits:?}"
    );
    assert_eq!(store.search(&id, "china pricing", 1).expect("search").len(), 1, "--top caps it");

    // `near` on a fact whose only neighbour is unrelated still answers with that neighbour.
    let sourdough = &store.list(&id, 10).expect("list")[1];
    let queue = store.near(&id, &sourdough.id, 5).expect("near");
    assert_eq!(queue.len(), 1, "the one other fact, whatever it scores: {queue:?}");
}

#[test]
fn list_shows_the_whole_base_most_recently_changed_first() {
    let (_dir, store) = store(Relation::Independent);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    assert!(store.list(&id, 50).expect("list").is_empty());

    seed(&store, &id, &["The first fact.", "The second fact."]);
    let rows = store.list(&id, 50).expect("list");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|f| f.version == 1 && f.kind == "fact"));
    assert_eq!(store.list(&id, 1).expect("list").len(), 1, "the limit is honoured");
}

/// What a real run got wrong, and could not have got right: shown two sentences and nothing else,
/// the classifier called a person's statement and an agent's conclusion drawn from it a
/// `duplicate`. A merge there deletes what somebody actually said.
#[test]
fn the_classifier_is_shown_who_said_each_side() {
    use std::sync::Mutex;

    /// Records the provenance it was handed, so a test can assert the prompt carries it.
    #[derive(Debug, Default)]
    struct Recording {
        seen: Mutex<Vec<(String, String)>>,
    }

    impl Judge for Recording {
        #[allow(clippy::unnecessary_literal_bound, reason = "the trait fixes the signature")]
        fn name(&self) -> &str {
            "recording"
        }

        fn extract(&self, _note: &str) -> Result<Vec<String>, JudgeError> {
            Ok(Vec::new())
        }

        fn classify(&self, pairs: &[(Side<'_>, Side<'_>)]) -> Result<Vec<Judgement>, JudgeError> {
            let mut seen = self.seen.lock().expect("lock");
            for (a, b) in pairs {
                seen.push((
                    format!("{}|{}|{}", a.author, a.creator, a.kind),
                    format!("{}|{}|{}", b.author, b.creator, b.kind),
                ));
            }
            Ok(pairs
                .iter()
                .map(|_| Judgement {
                    relation: Relation::Independent,
                    why: String::new(),
                })
                .collect())
        }
    }

    let (_dir, plain) = store(Relation::Independent);
    let judge = Arc::new(Recording::default());
    let store = plain.with_judge(judge.clone());
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["We are not sure we can enter the China market."]);

    let conclusion = Incoming::new("igor", "agent:planner@1")
        .from_sources(vec![])
        .as_artifact();
    store
        .add(&id, &conclusion, vec!["We are not sure about the China market.".into()])
        .expect("add");

    let seen = judge.seen.lock().expect("lock");
    let pair = seen.first().expect("the near pair reached the classifier");
    assert_eq!(
        pair.0, "igor|agent:planner@1|artifact",
        "the incoming side carries the batch's own provenance"
    );
    assert_eq!(
        pair.1, "igor|agent:chat@1|fact",
        "and the base side carries its own, read back through facts_v"
    );
}

/// The prompt has to *say* what the labels mean, or a model shown them will still judge on
/// wording alone.
#[test]
fn the_classification_prompt_tells_the_model_what_provenance_means() {
    let prompt = crate::judge::CLASSIFY_SYSTEM;
    assert!(prompt.contains("who said it"), "{prompt}");
    assert!(
        prompt.contains("NEVER duplicates"),
        "the rule has to be stated, not implied: {prompt}"
    );
    // The extraction prompt is still the prototype's word for word — changing it would move the
    // cosines the floor is calibrated on (RESULTS.md §10).
    assert!(crate::judge::EXTRACT_SYSTEM.contains("You are the extraction step of a knowledge base."));
}

/// The mechanical half of the statement-versus-conclusion fix. The prompt asks the classifier
/// not to do this and measurably helps, but it is not reliable — so a rule that is decidable
/// from data already in hand is decided in code.
#[test]
fn a_duplicate_across_two_kinds_of_record_is_corrected_to_narrows() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["We are not sure we can enter the China market."]);

    // An agent's conclusion, worded almost identically to what the person said.
    let conclusion = Incoming::new("igor", "agent:planner@1").as_artifact();
    let staging = store
        .add(
            &id,
            &conclusion,
            vec!["We are not confident about entering the China market.".into()],
        )
        .expect("add");

    let pair = &staging.pending[0];
    assert_eq!(
        pair.kind, "narrows",
        "the classifier said `duplicate`; a fact and an artifact are different records"
    );
    assert!(pair.why.contains("different records"), "and it says why: {}", pair.why);
    assert_eq!(staging.open().len(), 1, "the pair still reaches the reviewer");
}

/// …and it does not fire on two records of the same kind, which is where `duplicate` is right.
#[test]
fn two_facts_that_say_the_same_thing_are_still_a_duplicate() {
    let (_dir, store) = store(Relation::Duplicate);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["China pricing is set per seat."]);

    let staging = store
        .add(&id, &writer(), vec!["Pricing for China is per seat.".into()])
        .expect("add");
    assert_eq!(staging.pending[0].kind, "duplicate");
}

// ------------------------------------------------------------ neighbour selection

/// The point of top-K, and the thing a regression would quietly undo.
///
/// Under the similarity floor the pair count grew with the base — measured on the live 97-fact
/// base, one fact drew 43 pairs and another 76 of 96. K bounds it at any size.
#[test]
fn the_pair_count_is_bounded_by_k_and_does_not_grow_with_the_base() {
    let (_dir, store) = store(Relation::Controversy);
    let store = store.with_top_k(3);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    // Twelve facts that all share wording, so every one of them is close to every other — the
    // shape that made a floor admit most of the base.
    let crowd: Vec<&str> = vec![
        "We support market alpha.", "We support market beta.", "We support market gamma.",
        "We support market delta.", "We support market epsilon.", "We support market zeta.",
        "We support market eta.", "We support market theta.", "We support market iota.",
        "We support market kappa.", "We support market lambda.", "We support market mu.",
    ];
    seed(&store, &id, &crowd);
    assert_eq!(store.count(&id).expect("count"), 12);

    let staging = store
        .add(&id, &writer(), vec!["We support market nu.".into()])
        .expect("add");
    assert!(
        staging.pending.len() <= 3 * 2,
        "one fact against a base of twelve selected {} pairs at K=3; a floor would have taken \
         most of the base",
        staging.pending.len()
    );

    // Double the base and the count must not follow it up.
    let before = staging.pending.len();
    store.abort(&id, &staging.tx).expect("abort");
    seed(
        &store,
        &id,
        &[
            "We support market nu.", "We support market xi.", "We support market omicron.",
            "We support market pi.", "We support market rho.", "We support market sigma.",
            "We support market tau.", "We support market upsilon.", "We support market phi.",
            "We support market chi.", "We support market psi.", "We support market omega.",
        ],
    );
    assert_eq!(store.count(&id).expect("count"), 24);
    let after = store
        .add(&id, &writer(), vec!["We support market koppa.".into()])
        .expect("add");
    assert!(
        after.pending.len() <= 3 * 2,
        "the base doubled and the queue went {before} -> {}",
        after.pending.len()
    );
}

/// Symmetry, and why it is not tidiness: a fact in a sparse neighbourhood keeps a busy fact in
/// its own top K while the busy one, surrounded by closer things, does not reciprocate. Selecting
/// on one direction only loses exactly those pairs.
#[test]
fn a_pair_surfaces_when_only_one_side_ranks_the_other() {
    let (_dir, store) = store(Relation::Controversy);
    let store = store.with_top_k(1);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");

    // Two near-identical facts crowd each other out at K=1, plus a loner that shares one word
    // with them and nothing with anything else.
    seed(
        &store,
        &id,
        &[
            "alpha beta gamma delta epsilon.",
            "alpha beta gamma delta zeta.",
            "epsilon quite alone.",
        ],
    );

    // The incoming fact's own top 1 is one of the crowded pair. The loner's top 1, however, is
    // the incoming fact — nothing else shares a word with it.
    let staging = store
        .add(&id, &writer(), vec!["epsilon and nothing else.".into()])
        .expect("add");

    let loner = store
        .list(&id, 10)
        .expect("list")
        .into_iter()
        .find(|f| f.fact == "epsilon quite alone.")
        .expect("the loner is in the base");
    assert!(
        staging
            .pending
            .iter()
            .any(|p| p.base_id.as_deref() == Some(loner.id.as_str())),
        "the loner holds the newcomer in its top 1, so the pair must surface even though the \
         newcomer's own top 1 is elsewhere: {:?}",
        staging.pending
    );
}

/// There is no floor anywhere. Two facts with almost nothing in common still pair.
#[test]
fn nothing_is_dropped_for_scoring_low() {
    let (_dir, store) = store(Relation::Controversy);
    let id = base("global/default");
    store.ensure_base(&id).expect("ensure");
    seed(&store, &id, &["Sourdough needs a long cold retard."]);

    let staging = store
        .add(&id, &writer(), vec!["The registered agent is in Wilmington.".into()])
        .expect("add");
    assert_eq!(
        staging.open().len(),
        1,
        "two barely-related facts are still each other's nearest neighbour"
    );
    assert!(
        staging.pending[0].strength < 0.2,
        "and the score says how weak it is without gating on it: {}",
        staging.pending[0].strength
    );
}
