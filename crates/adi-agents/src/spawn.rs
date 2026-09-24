//! Which agent may launch which — matching an agent's [`can_spawn`](crate::AgentManifest::can_spawn)
//! against a candidate, the reverse view that answers "who can launch *me*", and the prompt line
//! that tells a run what it may reach.
//!
//! Enforcement itself lives in [`Agents::launch_run`](crate::Agents::launch_run), the one launch
//! path every caller goes through; this module is only the rule the check is made of.

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
    rules.iter().any(|rule| {
        if rule == WILDCARD {
            true
        } else if let Some(id) = rule.strip_prefix(PROJECT_PREFIX) {
            project == Some(id)
        } else {
            glob(rule, name)
        }
    })
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
    all.iter()
        .filter(|a| {
            allows(
                &a.manifest.can_spawn,
                &target.name,
                target.manifest.project.as_deref(),
            )
        })
        .map(|a| a.name.clone())
        .collect()
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
}
