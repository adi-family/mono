//! Which agent may launch which — matching an agent's [`can_spawn`](crate::AgentManifest::can_spawn)
//! against a candidate, the reverse view that answers "who can launch *me*", and the prompt line
//! that tells a run what it may reach.
//!
//! Enforcement itself lives in [`Agents::launch_run`](crate::Agents::launch_run), the one launch
//! path every caller goes through; this module is only the rule the check is made of.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::StoredAgent;

/// The rule that permits launching anything.
const WILDCARD: &str = "*";

/// The prefix on a rule naming a whole project's own agents.
const PROJECT_PREFIX: &str = "project:";

/// Whether `rules` (an agent's own `can_spawn`) permits launching an agent named `name`, filed
/// under `project` (`None` for a global agent).
///
/// Each rule is tried in order and the first match wins, though nothing here actually depends on
/// order — an allowlist has no "deny" rule to be shadowed by an earlier "allow". Three shapes:
///
/// * `*` — everything.
/// * `project:<id>` — every agent filed directly under that project (not its sub-projects; see
///   [`crate::AgentManifest::project`], which names one project, not a tree).
/// * anything else — matched against `name` as a [`glob`]: `dr-*` matches `dr-8f3a1c1e`, an
///   ephemeral worker's generated name, exactly as it matches a name somebody typed by hand.
#[must_use]
pub fn allows(rules: &[String], name: &str, project: Option<&str>) -> bool {
    matching_rule(rules, name, project).is_some()
}

/// The rule in `rules` responsible for [`allows`] matching `name`/`project` — `None` when none
/// does. Rules are tried in the order they are stored, same as `allows`; unlike `allows`, this
/// keeps *which* one, which is what tells a reader "an exact name, removable in one click" from
/// "only reachable through a pattern" (see [`spawned_by_via`]).
fn matching_rule<'a>(rules: &'a [String], name: &str, project: Option<&str>) -> Option<&'a str> {
    rules
        .iter()
        .find(|rule| {
            rule.as_str() == WILDCARD
                || rule
                    .strip_prefix(PROJECT_PREFIX)
                    .is_some_and(|id| project == Some(id))
                || glob(rule, name)
        })
        .map(String::as_str)
}

/// A minimal `*`-glob: `*` stands for any run of characters, including none; every other byte is
/// literal. No crate for this — the vocabulary this field speaks is one wildcard in one spot
/// (`dr-*`), never a character class or a `?`, so hand-rolling it is less than depending on one.
fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some(a), Some(b)) if a == b => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

/// The agents in `all` whose own `can_spawn` would let them launch `target` — the reverse of
/// everyone's `can_spawn`, computed here rather than stored a second time (see the field's own
/// doc for why: "who can launch C" is answered by reading everyone else's list, not a field on
/// C).
#[must_use]
pub fn spawned_by(all: &[StoredAgent], target: &StoredAgent) -> Vec<String> {
    spawned_by_via(all, target)
        .into_iter()
        .map(|e| e.caller)
        .collect()
}

/// One caller allowed to launch a given target, and which of its own rules is responsible — an
/// exact name (the only shape a target's own page may remove, since it is the one string that
/// names nothing else), or a pattern/`project:<id>`/`*` the target's page cannot safely narrow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnedByEntry {
    pub caller: String,
    /// The rule on the caller's own `can_spawn` responsible for the match.
    pub via: String,
    /// Whether `via` is the target's exact name.
    pub exact: bool,
}

/// [`spawned_by`], keeping the rule responsible for each entry — what a panel's editable reverse
/// view needs to tell "removable here" (`exact`) from "only points at the caller's own page"
/// (matched through a pattern, `project:<id>`, or `*`).
#[must_use]
pub fn spawned_by_via(all: &[StoredAgent], target: &StoredAgent) -> Vec<SpawnedByEntry> {
    all.iter()
        .filter_map(|a| {
            let via = matching_rule(
                &a.manifest.can_spawn,
                &target.name,
                target.manifest.project.as_deref(),
            )?;
            Some(SpawnedByEntry {
                caller: a.name.clone(),
                exact: via == target.name,
                via: via.to_string(),
            })
        })
        .collect()
}

/// One recorded launch naming who asked for it and what they named — the input
/// [`refusals`] groups by caller and target. Built from a [`SessionRecord`](crate::store::SessionRecord)
/// whose `launched_by` is `agent:<name>`; a human or automated launch never becomes one of these,
/// exactly as it is never checked by [`allows`].
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    /// The agent that asked for the run — `launched_by` with the `agent:` prefix stripped.
    pub caller: &'a str,
    /// The agent that was launched — the session's own `agent`.
    pub target: &'a str,
    /// The target's *current* project, for a `project:<id>` rule to match against — `None` for a
    /// global agent, or one this launch's target no longer names at all.
    pub target_project: Option<&'a str>,
    /// When the launch happened, unix milliseconds.
    pub at: u64,
}

/// One caller → target pair whose recorded launches do not match the caller's *current*
/// `can_spawn` — what switching `spawn_policy` to `enforce` would have stopped, going back
/// however far the caller who built the list looked. Grouped and counted rather than one row per
/// session: a caller that retried the same refused target forty times is one row, not forty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub caller: String,
    pub target: String,
    /// How many recorded launches this pair covers.
    pub count: u32,
    /// The most recent of them, unix milliseconds.
    pub last_at: u64,
}

/// Group `launches` into [`Refusal`]s, keeping only the pairs `all`'s *current* rules refuse — the
/// same [`allows`] [`crate::Agents::launch_run`] itself calls, so a panel built on this can never
/// show a row enforcing the policy would not actually stop, or miss one it would. A caller absent
/// from `all` (its definition deleted, or never registered) reads as having no permissions of its
/// own, the same as an empty `can_spawn` — see `Agents::launch_run`'s own comment on the point.
///
/// Ordered newest-refusal-first: an operator deciding whether it is safe to enforce cares most
/// about what is still happening.
#[must_use]
pub fn refusals<'a>(
    all: &[StoredAgent],
    launches: impl IntoIterator<Item = Launch<'a>>,
) -> Vec<Refusal> {
    let mut grouped: BTreeMap<(&'a str, &'a str), Refusal> = BTreeMap::new();
    for launch in launches {
        let caller_rules = all
            .iter()
            .find(|a| a.name == launch.caller)
            .map_or(&[] as &[String], |a| a.manifest.can_spawn.as_slice());
        if allows(caller_rules, launch.target, launch.target_project) {
            continue;
        }
        let entry = grouped
            .entry((launch.caller, launch.target))
            .or_insert_with(|| Refusal {
                caller: launch.caller.to_string(),
                target: launch.target.to_string(),
                count: 0,
                last_at: 0,
            });
        entry.count += 1;
        entry.last_at = entry.last_at.max(launch.at);
    }
    let mut out: Vec<Refusal> = grouped.into_values().collect();
    out.sort_by(|a, b| {
        b.last_at
            .cmp(&a.last_at)
            .then_with(|| a.caller.cmp(&b.caller))
            .then_with(|| a.target.cmp(&b.target))
    });
    out
}

/// The prompt section telling a run what it may launch — empty when it may launch nothing, which
/// is most agents.
#[must_use]
pub fn block(can_spawn: &[String]) -> String {
    if can_spawn.is_empty() {
        return String::new();
    }
    let list = can_spawn
        .iter()
        .map(|rule| format!("`{rule}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "# What you can launch\n\n\
         You may launch: {list} (`adi-agents run <name> …`, or any tool built on it). Launching \
         an agent not on this list is refused, with the name of this field in the refusal — ask \
         whoever runs this machine to widen it if a run of yours genuinely needs to reach \
         another agent."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::StoredAgentManifest;

    fn agent(name: &str, project: Option<&str>, can_spawn: &[&str]) -> StoredAgent {
        StoredAgent {
            name: name.to_string(),
            manifest: StoredAgentManifest {
                backend: Some("harness:adi".into()),
                project: project.map(ToString::to_string),
                can_spawn: can_spawn.iter().map(ToString::to_string).collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn an_exact_name_matches_only_itself() {
        assert!(allows(&["b".into()], "b", None));
        assert!(!allows(&["b".into()], "c", None));
    }

    #[test]
    fn a_glob_matches_a_prefix_including_an_ephemeral_generated_name() {
        let rules = vec!["dr-*".into()];
        assert!(allows(&rules, "dr-8f3a1c1e", None));
        assert!(allows(&rules, "dr-", None));
        assert!(!allows(&rules, "research-worker", None));
    }

    #[test]
    fn a_project_rule_matches_only_agents_filed_directly_under_it() {
        let rules = vec!["project:acme".into()];
        assert!(allows(&rules, "anything", Some("acme")));
        assert!(!allows(&rules, "anything", Some("other")));
        assert!(!allows(&rules, "anything", None));
    }

    #[test]
    fn a_wildcard_rule_matches_everything() {
        let rules = vec![WILDCARD.to_string()];
        assert!(allows(&rules, "whatever-99", Some("any-project")));
        assert!(allows(&rules, "whatever-99", None));
    }

    #[test]
    fn an_empty_list_refuses_everything() {
        assert!(!allows(&[], "b", Some("acme")));
    }

    #[test]
    fn spawned_by_is_the_reverse_of_everyones_can_spawn() {
        let all = vec![
            agent("a", None, &["b"]),
            agent("b", None, &["c"]),
            agent("d", None, &["dr-*"]),
        ];
        let c = agent("c", None, &[]);
        assert_eq!(spawned_by(&all, &c), vec!["b".to_string()]);

        let dr_worker = agent("dr-8f3a1c1e", None, &[]);
        assert_eq!(spawned_by(&all, &dr_worker), vec!["d".to_string()]);
    }

    /// An exact-name rule is `exact` — the one shape a target's own page may remove — while a
    /// pattern that merely happens to reach the same target is not, whatever it is spelled as.
    #[test]
    fn spawned_by_via_marks_only_an_exact_name_rule_as_removable_from_the_targets_page() {
        let all = vec![
            agent("a", None, &["b"]),
            agent("d", None, &["dr-*"]),
            agent("w", None, &["*"]),
        ];
        let b = agent("b", None, &[]);
        let entries = spawned_by_via(&all, &b);
        assert_eq!(
            entries.iter().find(|e| e.caller == "a"),
            Some(&SpawnedByEntry {
                caller: "a".into(),
                via: "b".into(),
                exact: true,
            })
        );

        let dr_worker = agent("dr-8f3a1c1e", None, &[]);
        let entries = spawned_by_via(&all, &dr_worker);
        assert_eq!(
            entries.iter().find(|e| e.caller == "d"),
            Some(&SpawnedByEntry {
                caller: "d".into(),
                via: "dr-*".into(),
                exact: false,
            })
        );

        let anything = agent("anything", None, &[]);
        let entries = spawned_by_via(&all, &anything);
        assert_eq!(entries, vec![SpawnedByEntry {
            caller: "w".into(),
            via: "*".into(),
            exact: false,
        }]);
    }

    /// When a caller lists the target's exact name *and* a pattern that also reaches it, the exact
    /// rule is the one surfaced — rules are tried in the order they are stored, and the exact name
    /// is what `set_can_spawn_rule`'s removal needs to see.
    #[test]
    fn an_exact_rule_ahead_of_a_covering_pattern_is_the_one_reported() {
        let all = vec![agent("a", None, &["b", "b-*"])];
        let b = agent("b", None, &[]);
        assert_eq!(spawned_by_via(&all, &b), vec![SpawnedByEntry {
            caller: "a".into(),
            via: "b".into(),
            exact: true,
        }]);
    }

    #[test]
    fn the_block_is_empty_for_an_agent_that_may_launch_nothing() {
        assert!(block(&[]).is_empty());
    }

    #[test]
    fn the_block_names_every_rule() {
        let text = block(&["b".to_string(), "dr-*".to_string()]);
        assert!(text.contains('b'));
        assert!(text.contains("dr-*"));
        assert!(text.contains("# What you can launch"));
    }

    fn launch<'a>(caller: &'a str, target: &'a str, project: Option<&'a str>, at: u64) -> Launch<'a> {
        Launch {
            caller,
            target,
            target_project: project,
            at,
        }
    }

    /// A launch whose target *is* on the caller's current `can_spawn` never becomes a refusal —
    /// what makes an Allow click retire a row on the very next read.
    #[test]
    fn a_launch_the_caller_is_now_allowed_is_not_a_refusal() {
        let all = vec![agent("a", None, &["b"])];
        let refusals = refusals(&all, [launch("a", "b", None, 1)]);
        assert!(refusals.is_empty());
    }

    /// A caller with no rule for the target at all — the ordinary case before ADI-MONO-113 ever
    /// shipped — is exactly what the list exists to surface.
    #[test]
    fn a_launch_outside_the_callers_rules_is_a_refusal() {
        let all = vec![agent("a", None, &[])];
        let refusals = refusals(&all, [launch("a", "b", None, 1)]);
        assert_eq!(refusals, vec![Refusal {
            caller: "a".into(),
            target: "b".into(),
            count: 1,
            last_at: 1,
        }]);
    }

    /// Repeated launches of the same pair group into one row, counted, with the latest moment kept
    /// — not the whole history as separate entries.
    #[test]
    fn repeated_refusals_of_the_same_pair_group_into_one_row() {
        let all = vec![agent("a", None, &[])];
        let refusals = refusals(
            &all,
            [launch("a", "b", None, 5), launch("a", "b", None, 20), launch("a", "b", None, 12)],
        );
        assert_eq!(refusals, vec![Refusal {
            caller: "a".into(),
            target: "b".into(),
            count: 3,
            last_at: 20,
        }]);
    }

    /// A caller whose definition no longer exists reads as having no permissions of its own — the
    /// same rule `launch_run` follows — so its old launches still surface rather than vanishing
    /// with the definition.
    #[test]
    fn a_caller_that_no_longer_exists_still_refuses_everything() {
        let refusals = refusals(&[], [launch("gone", "b", None, 1)]);
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].caller, "gone");
    }

    /// A `project:<id>` rule is checked against the target's *current* project, not whatever it
    /// was filed under at launch time — the whole point of deriving rather than storing this.
    #[test]
    fn project_rules_are_matched_against_the_targets_current_project() {
        let all = vec![agent("a", None, &["project:acme"]), agent("b", Some("acme"), &[])];
        assert!(refusals(&all, [launch("a", "b", Some("acme"), 1)]).is_empty());

        let elsewhere = vec![agent("a", None, &["project:acme"]), agent("b", Some("other"), &[])];
        assert_eq!(refusals(&elsewhere, [launch("a", "b", Some("other"), 1)]).len(), 1);
    }

    /// Newest refusal first, so an operator scanning the list sees what is still happening before
    /// what happened once, long ago.
    #[test]
    fn refusals_are_ordered_newest_first() {
        let all = vec![agent("a", None, &[]), agent("c", None, &[])];
        let refusals = refusals(&all, [launch("a", "b", None, 5), launch("c", "d", None, 50)]);
        assert_eq!(
            refusals.iter().map(|r| r.caller.as_str()).collect::<Vec<_>>(),
            ["c", "a"],
        );
    }
}
