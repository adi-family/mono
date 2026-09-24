//! Who asked for a run — the vocabulary written to
//! [`SessionRecord::launched_by`](crate::store::SessionRecord::launched_by), and reused verbatim by
//! [`AgentManifest::created_by`](crate::AgentManifest::created_by): one asks who started a
//! *conversation*, the other who created the *definition*, and both are answered in the same three
//! words so a reader never has to learn a second vocabulary for the second question.
//!
//! A fleet starts most of its own work: an agent launches a helper, a trigger fires one on an
//! event, a script runs one on a schedule. In a rail of four hundred conversations that makes "the
//! ones I started myself" a question nothing could answer — a run a person opened and a run another
//! agent spawned looked identical, because nothing wrote down the difference. This is that
//! difference, recorded at the one moment it is known: the launch.
//!
//! Three words and an absence:
//!
//! * [`HUMAN`] — a person asked for it. The control panel (somebody is looking at it), or a
//!   terminal somebody typed into.
//! * `agent:<name>` — another agent's run asked for it, through a tool on its PATH. See
//!   [`by_caller`].
//! * [`AUTOMATION`] — something started it with nobody there: a trigger, a cron, a script.
//! * `""` — nobody wrote it down. Every session opened before this existed, and any launcher that
//!   declines to say. Never treated as [`HUMAN`]: a filter for "mine" that swept in everything
//!   unattributed would be no filter at all on a machine with a year of history.
//!   [`created_by`](crate::AgentManifest::created_by) reads this one differently — see [`is_mine`]
//!   for why.

/// A person asked for this run.
pub const HUMAN: &str = "human";

/// Something with nobody watching asked for this run — a trigger, a schedule, a script.
pub const AUTOMATION: &str = "automation";

/// The prefix an agent-launched run carries, ahead of the launching agent's name.
pub const AGENT_PREFIX: &str = "agent:";

/// `agent:<name>` when this process is running inside an agent's conversation, `None` when it is
/// not.
///
/// The same environment [`awaits::caller`](crate::awaits::caller) reads, and for the same reason: a
/// tool run by an agent is handed its conversation in `ADI_AGENT` / `ADI_RUN_ID`, so a launch made
/// through one is attributable without anybody passing an extra flag. A person's shell has neither
/// set, which is what makes the absence meaningful rather than merely missing.
#[must_use]
pub fn by_caller() -> Option<String> {
    crate::awaits::caller().map(|who| format!("{AGENT_PREFIX}{}", who.agent))
}

/// Whether `launched_by` says a person asked for the run.
#[must_use]
pub fn is_human(launched_by: &str) -> bool {
    launched_by == HUMAN
}

/// Whether this vocabulary's owner counts as "mine" for the **Agents page's** Mine filter — a
/// different question from [`is_human`], which the sessions rail's own "started by me" filter asks
/// of the same three words.
///
/// The difference is deliberate, and it is `""`: a session's empty `launched_by` means "opened
/// before the store wrote this down", one gap among the ones that keep rolling off as old sessions
/// age out, so [`is_human`] is right to treat it as not a person's. An agent *definition*'s empty
/// `created_by` means something else — no definition on the machine has ever had one filled in
/// (`created_by` has no backfill), and a definition never ages out the way a session does. Reading
/// `""` as "not mine" here would leave the Mine filter, and every agent picker built on it, empty of
/// nearly every agent that is real — so unknown counts as mine on this side, and only the two known
/// machine-made words (`automation`, `agent:<name>`) are filtered out.
///
/// Flip this in one place — the whole reason it is a function and not `created_by == HUMAN` at each
/// call site — if backfilling ever makes `""` rare enough that treating it as "not mine" stops
/// emptying the page.
#[must_use]
pub fn is_mine(created_by: &str) -> bool {
    created_by != AUTOMATION && !created_by.starts_with(AGENT_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one rule a reader depends on: an unattributed session is not a person's.
    #[test]
    fn only_the_word_human_reads_as_a_person() {
        assert!(is_human(HUMAN));
        assert!(!is_human(""));
        assert!(!is_human(AUTOMATION));
        assert!(!is_human("agent:adi-agent"));
    }

    /// The Agents page's Mine differs from the rail's in exactly one case: the empty string.
    #[test]
    fn mine_counts_human_and_unknown_but_not_machine_made() {
        assert!(is_mine(HUMAN));
        assert!(is_mine(""), "no definition has ever had this filled in");
        assert!(!is_mine(AUTOMATION));
        assert!(!is_mine("agent:adi-agent"));
    }
}
