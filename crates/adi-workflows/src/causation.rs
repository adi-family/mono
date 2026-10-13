//! The self-retrigger fix (Igor's decision, ADI-MONO-146): a workflow listening on an event its
//! own activity produces — most obviously its own `on: event("adi.tasks.created")` matching the
//! very root task its own pour just created, but just as real a few hops removed, through
//! another workflow echoing it back — must never retrigger itself forever.
//!
//! The fix is generic, not a special case for the direct loop: every event emitted on behalf of
//! a workflow run carries the run that caused it — a task event for a task with `workflow_run`
//! set already does, structurally, since that field *is* the causing run; the three
//! `adi.workflows.run.*` events carry a dedicated [`WorkflowRunStarted::caused_by_run`] field for
//! the same purpose. [`causer_of_payload`] reads either spelling off an arbitrary event's parsed
//! JSON payload, uniformly. [`would_cycle`] then walks the chain of runs that caused each other
//! — purely through [`RunRow::caused_by_run`], one DB lookup per hop, never re-reading the event
//! log past the very first hop (which is how the caller got here in the first place) — up to
//! [`MAX_DEPTH`] hops, refusing if the target workflow appears anywhere in that chain.

use serde_json::Value;

use crate::store::WorkflowStore;

/// How many hops [`would_cycle`] follows before giving up the search and allowing the start —
/// Igor's decision: "caps a causation chain at depth 8". Bounds the work a single event-starter
/// tick can do per candidate match; a cycle that only closes beyond this many hops of workflow-
/// to-workflow causation is accepted as a risk this crate does not chase further.
pub const MAX_DEPTH: u8 = 8;

/// The run that caused an event to be emitted, read off its already-parsed JSON payload — the
/// crate's one place that knows both spellings: `caused_by_run` (the `adi.workflows.run.*`
/// family) and `workflow_run` (every task event, via [`adi_tasks::Task::workflow_run`] already
/// being exactly that fact). `None` when the payload has neither, or isn't an object — an event
/// with no causer of its own, as far as this crate can tell.
#[must_use]
pub fn causer_of_payload(payload: &Value) -> Option<String> {
    payload
        .get("caused_by_run")
        .or_else(|| payload.get("workflow_run"))
        .and_then(Value::as_str)
        .map(String::from)
}

/// Whether starting a new run of `target_workflow` because of a match whose causer is `start`
/// would be (directly or transitively) the workflow retriggering itself: walk `start`'s own
/// causation chain — `start`, then whatever caused `start`, then whatever caused *that* — up to
/// [`MAX_DEPTH`] hops, checking at each hop whether that run belongs to `target_workflow`.
///
/// A hop that names a run this store has never heard of (deleted, or from a store this crate
/// doesn't share — should not happen for `caused_by_run`, which only ever names a run this same
/// `WorkflowStore` poured) ends the chain there and reports no cycle, rather than erroring: an
/// unprovable chain is not evidence of one.
#[must_use]
pub fn would_cycle(store: &WorkflowStore, start: &str, target_workflow: &str) -> bool {
    let mut cursor = Some(start.to_string());
    for _ in 0..MAX_DEPTH {
        let Some(run_key) = cursor else { return false };
        let Ok(Some(row)) = store.get_run(&run_key) else {
            return false;
        };
        if row.workflow == target_workflow {
            return true;
        }
        cursor = row.caused_by_run;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;
    use serde_json::json;

    fn scratch(tag: &str) -> WorkflowStore {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-causation-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        WorkflowStore::with_config(Config::with_root(root))
    }

    #[test]
    fn causer_reads_either_spelling_and_neither_is_an_error() {
        assert_eq!(
            causer_of_payload(&json!({"caused_by_run": "w@1"})),
            Some("w@1".to_string())
        );
        assert_eq!(
            causer_of_payload(&json!({"workflow_run": "w@2"})),
            Some("w@2".to_string())
        );
        // `caused_by_run` wins when (implausibly) both are present.
        assert_eq!(
            causer_of_payload(&json!({"caused_by_run": "a", "workflow_run": "b"})),
            Some("a".to_string())
        );
        assert_eq!(causer_of_payload(&json!({})), None);
        assert_eq!(causer_of_payload(&json!(null)), None);
    }

    #[test]
    fn a_direct_self_cause_is_a_cycle_at_depth_one() {
        let store = scratch("direct");
        store
            .create_run("w@1", "w", "/w.ts", "h", &json!({}), &json!({}), None)
            .expect("create");
        assert!(would_cycle(&store, "w@1", "w"));
        assert!(!would_cycle(&store, "w@1", "other"));
    }

    #[test]
    fn a_chain_through_other_workflows_is_still_found_within_the_depth_cap() {
        let store = scratch("chain");
        // a -> b -> c -> back to a's own workflow ("a") three hops in.
        store
            .create_run("a@1", "a", "/a.ts", "h", &json!({}), &json!({}), None)
            .expect("a1");
        store
            .create_run("b@1", "b", "/b.ts", "h", &json!({}), &json!({}), Some("a@1"))
            .expect("b1");
        store
            .create_run("c@1", "c", "/c.ts", "h", &json!({}), &json!({}), Some("b@1"))
            .expect("c1");
        store
            .create_run("a@2", "a", "/a.ts", "h", &json!({}), &json!({}), Some("c@1"))
            .expect("a2");

        // Considering starting workflow "a" because of something caused by "a@2" must see the
        // cycle: a@2 -> c@1 -> b@1 -> a@1, and a@1's workflow is "a".
        assert!(would_cycle(&store, "a@2", "a"));
        // But workflow "d", never in the chain, is not a cycle.
        assert!(!would_cycle(&store, "a@2", "d"));
    }

    #[test]
    fn a_cycle_beyond_the_depth_cap_is_not_found() {
        let store = scratch("too-deep");
        store
            .create_run("w0@1", "w0", "/w.ts", "h", &json!({}), &json!({}), None)
            .expect("seed");
        let mut prev = "w0@1".to_string();
        // w1 is caused by w0 (1 hop), w2 by w1 (2 hops from w0), ..., w9 by w8 — 9 hops from w0,
        // one more than MAX_DEPTH (8).
        for i in 1..=9 {
            let key = format!("w{i}@1");
            store
                .create_run(&key, &format!("w{i}"), "/w.ts", "h", &json!({}), &json!({}), Some(&prev))
                .expect("chain");
            prev = key;
        }

        // w0 is 9 hops away from w9@1 — found at the 8-hop cap would mean this fails.
        assert!(!would_cycle(&store, &prev, "w0"));
        // A workflow only 3 hops away (w6, say) is still well within the cap.
        assert!(would_cycle(&store, &prev, "w6"));
    }

    #[test]
    fn an_unknown_causer_ends_the_chain_without_erroring() {
        let store = scratch("unknown");
        assert!(!would_cycle(&store, "ghost@1", "anything"));
    }
}
