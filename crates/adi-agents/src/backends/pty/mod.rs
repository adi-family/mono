//! The `pty` executor's engine commands, and the naming that ties a session to its agent.
//!
//! The session lifecycle — open, capture, type into, stop — is [`crate::runner::pty::PtyRunner`],
//! which reaches [`adi_pty`] itself. What stays here is the part that is *not* about one session:
//! how an agent's name becomes a session name, and the sweep over every live one.

pub(crate) mod claude;
pub(crate) mod codex;

use std::collections::BTreeSet;

/// Marks a pty session as one of ours. Shared with the runner, which derives a session name from
/// the agent it was handed, and with [`running_sessions`], which has no agent to ask.
pub(crate) const SESSION_PREFIX: &str = "adi-agent-";

/// The pty session name for an agent. Agent names may contain `.` (valid on disk); dots become
/// dashes so a session name stays a single flat token.
#[must_use]
pub fn session_name(agent_name: &str) -> String {
    format!("{SESSION_PREFIX}{}", agent_name.replace('.', "-"))
}

/// Session names of every live `adi-agent-*` pty session.
///
/// Not addressed by agent, and so not a runner verb: pty sessions live inside the process that
/// opened them and keep no run directory to name an agent from. The caller matches these against
/// the agents it already holds — which is how the concurrency cap counts panes.
#[must_use]
pub fn running_sessions() -> BTreeSet<String> {
    adi_pty::running(SESSION_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_prefixed_and_scoped() {
        assert_eq!(session_name("athz-solver"), "adi-agent-athz-solver");
        assert_eq!(session_name("a.b"), "adi-agent-a-b");
    }
}
