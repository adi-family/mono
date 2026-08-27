//! Fields of an agent's definition replaced for **one run** — this agent, but on a different model,
//! or with its permission mode loosened for one job.
//!
//! An agent definition is a template, and most of the time editing it is the right way to change
//! what a run does: the change is meant, and it is meant to stick. The exception is the launch that
//! is deliberately unlike the others — try this task on the big model, run this one under
//! `bypassPermissions` because it is a scratch checkout — where editing the agent means editing it
//! back afterwards, and forgetting to is how an agent ends up permanently on settings somebody chose
//! for one afternoon.
//!
//! So an override is not stored on the agent at all. It travels with the launch
//! ([`LaunchOptions::overrides`](crate::LaunchOptions)), is written onto the session it opens, and
//! is re-applied on every later turn of that conversation — because a chat that answered its first
//! message on `opus` and its second on whatever the agent says would be two agents wearing one
//! title.
//!
//! Three states per key, and the difference matters:
//!
//! * **absent** — inherit whatever the agent is defined with. The normal case, and what an
//!   untouched control sends.
//! * **empty string** — *unset* the argument for this run, back to the engine's own default. This
//!   is the only way to say "the agent pins `permission_mode`, and this run should not".
//! * **anything else** — the value this run uses in place of the agent's.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::StoredAgent;

/// What one run replaces in its agent's definition.
///
/// Deliberately small. Everything here is a *setting* — a value the engine reads — and nothing is a
/// capability: a run cannot grant itself a tool, a secret or a knowledge base it was not given,
/// because those are the agent's identity rather than its dials, and an override is a dial.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunOverrides {
    /// Backend arguments to replace, by the same names the agent form uses (`model`,
    /// `permission_mode`, `sandbox`, …). An empty string unsets the argument for this run.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, serde_json::Value>,
    /// Whether this run is unattended, when the launch means to differ from the agent. `None`
    /// inherits — which is not the same as `Some(false)`, and the difference is whether a run that
    /// asks a question stops or decides for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattended: Option<bool>,
}

impl RunOverrides {
    /// Whether this says nothing at all — the shape of every ordinary launch, and the check that
    /// keeps an empty object out of the session record.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arguments.is_empty() && self.unattended.is_none()
    }

    /// The agent as *this run* sees it.
    ///
    /// Borrowed when there is nothing to override, which is nearly always: the launch path already
    /// holds a `StoredAgent`, and cloning a manifest — tools, secrets, knowledge, prompt — to change
    /// nothing would be a copy on every run to serve the rare one.
    #[must_use]
    pub fn apply<'a>(&self, agent: &'a StoredAgent) -> Cow<'a, StoredAgent> {
        if self.is_empty() {
            return Cow::Borrowed(agent);
        }
        let mut patched = agent.clone();
        for (key, value) in &self.arguments {
            // An empty string is "unset it", not "set it to nothing": the engines read a blank
            // `--model ''` as a value and refuse it, and the whole point of this state is to get
            // back to the engine's own default.
            if value.as_str().is_some_and(str::is_empty) {
                patched.manifest.arguments.remove(key);
            } else {
                patched
                    .manifest
                    .arguments
                    .insert(key.clone(), value.clone());
            }
        }
        if let Some(unattended) = self.unattended {
            patched.manifest.unattended = unattended;
        }
        Cow::Owned(patched)
    }

    /// Read `key=value` pairs as a person types them on a command line.
    ///
    /// Values stay strings, whatever they look like: `model=opus` and `max_turns=12` are typed the
    /// same way, and it is the backend's own argument type that knows which is a number — a `"12"`
    /// that ought to be an integer is caught by the same validation an edited agent goes through,
    /// with the same message. `key=` (nothing after the `=`) is the unset form.
    ///
    /// # Errors
    /// Returns the offending argument when it carries no `=` at all, since a bare word is far more
    /// likely a typo than a request to set something to nothing.
    pub fn parse_pairs(pairs: &[String]) -> std::result::Result<Self, String> {
        let mut out = Self::default();
        for pair in pairs {
            let Some((key, value)) = pair.split_once('=') else {
                return Err(format!(
                    "--set takes key=value (e.g. --set model=opus); got `{pair}`"
                ));
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(format!("--set takes key=value; got `{pair}`"));
            }
            if key == "unattended" {
                out.unattended = Some(matches!(
                    value.trim(),
                    "1" | "true" | "yes" | "on" | "" // `--set unattended=` reads as "yes, it is"
                ));
                continue;
            }
            out.arguments
                .insert(key.to_string(), serde_json::Value::String(value.into()));
        }
        Ok(out)
    }

    /// One line naming what this run differs by — `model=opus · permission_mode unset` — for a log
    /// line or a listing. Empty when nothing is overridden.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (key, value) in &self.arguments {
            match value.as_str() {
                Some("") => parts.push(format!("{key} unset")),
                Some(text) => parts.push(format!("{key}={text}")),
                None => parts.push(format!("{key}={value}")),
            }
        }
        if let Some(unattended) = self.unattended {
            parts.push(format!("unattended={unattended}"));
        }
        parts.join(" \u{b7} ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentManifest};
    use crate::backend::Backend;

    fn agent_with(model: &str) -> StoredAgent {
        let mut arguments = BTreeMap::new();
        arguments.insert("model".to_string(), serde_json::json!(model));
        arguments.insert("permission_mode".to_string(), serde_json::json!("manual"));
        Agent {
            name: "reviewer".to_string(),
            manifest: AgentManifest {
                backend: Backend::HarnessClaudeSdk,
                arguments,
                tags: Vec::new(),
                starred: false,
                project: None,
                bin_tools: Vec::new(),
                prelude: Vec::new(),
                knowledge: Vec::new(),
                memory: false,
                secrets: Vec::new(),
                path: Vec::new(),
                env: BTreeMap::new(),
                unattended: false,
                created_at: 0,
                updated_at: 0,
            },
        }
    }

    /// Nothing to say means the caller's own agent back, not a copy of it.
    #[test]
    fn empty_overrides_borrow() {
        let agent = agent_with("sonnet");
        assert!(matches!(
            RunOverrides::default().apply(&agent),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_value_replaces_and_a_blank_unsets() {
        let agent = agent_with("sonnet");
        let overrides = RunOverrides::parse_pairs(&[
            "model=opus".to_string(),
            "permission_mode=".to_string(),
            "unattended=true".to_string(),
        ])
        .expect("pairs parse");
        let run = overrides.apply(&agent);
        assert_eq!(
            run.manifest.arguments.get("model"),
            Some(&serde_json::json!("opus"))
        );
        assert!(!run.manifest.arguments.contains_key("permission_mode"));
        assert!(run.manifest.unattended);
        // The definition itself is untouched — an override is about one run.
        assert_eq!(
            agent.manifest.arguments.get("model"),
            Some(&serde_json::json!("sonnet"))
        );
    }

    #[test]
    fn a_pair_without_an_equals_is_refused() {
        assert!(RunOverrides::parse_pairs(&["model".to_string()]).is_err());
    }

    #[test]
    fn the_summary_names_what_differs() {
        let overrides =
            RunOverrides::parse_pairs(&["model=opus".to_string(), "permission_mode=".to_string()])
                .expect("pairs parse");
        assert_eq!(
            overrides.summary(),
            "model=opus \u{b7} permission_mode unset"
        );
    }
}
