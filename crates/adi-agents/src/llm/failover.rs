//! What a run does about a turn that ended on a limit: hold it, move down the chain, or stop and
//! ask.
//!
//! Deliberately pure. Nothing here opens a database, sends a turn or writes to a transcript —
//! [`decide`] is handed what happened and answers what should happen, so the rule can be read and
//! tested without a store behind it. [`Agents`](crate::Agents) does the acting.
//!
//! # The rule, in one place
//!
//! ```text
//! quota / rate      hold it, move to the best row still available, say so in one line
//! auth              hold the whole login, move nothing, say why
//! transient         nothing — a blip is retried in place
//! unknown           nothing — the engine's own error stands, and no row is skipped on a guess
//! nowhere to go     stop and say every backend is out
//! ask_on_switch     never move on its own, whatever the class
//! ```
//!
//! # Why a switch can be refused even when a row is free
//!
//! Requirement 7 is that the whole conversation moves — replayed in full, never summarized and
//! never cut. That is not something every runtime can be handed, so [`can_carry`] is a separate
//! question from "is it held", and a chain that cannot carry its history stops loudly rather than
//! starting the new backend on a conversation missing its own past.

use crate::backend::Backend;
use crate::llm::backend::LimitClass;
use crate::llm::chain::{PinnedChain, ResolvedBackend};
use crate::llm::classify::Classification;

/// What a run should do about the turn that just failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Leave it alone. The failure was not one of the machine's problems — a blip, or something
    /// nobody wrote a rule for — so the engine's own error stands as the answer, which is what it
    /// has always done and what "do not reroute on a guess" means in practice.
    Nothing,
    /// Move to the row at `to` and re-ask the same message there.
    Switch {
        /// The index into the pinned chain to move to.
        to: usize,
        /// The one line the chat is told, e.g. `switched to codex, anthropic limited until 14:00`.
        notice: String,
    },
    /// Stop, and tell the person why nothing moved. Not a blocking question: it is a line in the
    /// conversation naming the thing only they can settle — a broken login, an empty chain, or a
    /// switch they asked to approve.
    Ask {
        /// What to say.
        reason: String,
    },
}

/// Everything the decision needs about the turn that failed, beyond its chain.
#[derive(Clone, Copy)]
pub struct Failure<'a> {
    /// Whether there is a conversation behind this turn — anything the new backend would have to be
    /// handed along with the message being retried. `false` for a one-shot run and for a chat's very
    /// first turn, where a switch carries nothing and any row can take it.
    pub has_history: bool,
    /// A rough size of that history in tokens, for the context check. `0` skips it.
    pub history_tokens: u64,
    /// The global "ask before every switch" switch. Off by default; on, nothing moves on its own.
    pub ask_on_switch: bool,
    /// Whether each row is blocked right now, and by what — the shared hold store, asked once by the
    /// caller and passed in as an answer rather than a handle.
    pub blocked: &'a dyn Fn(&ResolvedBackend) -> Option<String>,
}

/// Hand-written because [`blocked`](Failure::blocked) is a closure and closures have no `Debug`. The
/// rest is printed, which is the half worth reading in a log line anyway.
impl std::fmt::Debug for Failure<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Failure")
            .field("has_history", &self.has_history)
            .field("history_tokens", &self.history_tokens)
            .field("ask_on_switch", &self.ask_on_switch)
            .finish_non_exhaustive()
    }
}

/// Decide what a failed turn does next.
///
/// `found` is the classification of the failure *on the row the conversation is currently on*, read
/// against that row's own rules — see [`classify`](crate::llm::classify).
#[must_use]
pub fn decide(chain: &PinnedChain, found: &Classification, turn: &Failure<'_>) -> Decision {
    let Some(current) = chain.current() else {
        return Decision::Nothing;
    };

    match found.class {
        // A blip and an unrecognised error are both left where they are, and for the same reason
        // from opposite ends: one is fixed by trying again, the other is not understood well enough
        // to route around. Rerouting on either would empty a chain on the strength of a guess.
        LimitClass::Transient | LimitClass::Unknown => return Decision::Nothing,
        // Waiting out a cooldown would never fix a dead token, and neither would the next row if it
        // is on the same login. So this one always stops, whatever else is free.
        LimitClass::Auth => {
            return Decision::Ask {
                reason: format!(
                    "{}'s login is not working — {}. Fix the credential, or pick another backend; \
                     nothing was switched, because a broken login is not something waiting fixes.",
                    current.backend, found.evidence
                ),
            };
        }
        LimitClass::Quota | LimitClass::Rate => {}
    }

    // Why each row is out, gathered as it is tested so the "nowhere to go" message can say so
    // instead of leaving somebody to guess which of five backends failed and how.
    let mut refused: Vec<String> = Vec::new();
    let target = chain.next_available(|row| {
        if let Some(why) = (turn.blocked)(row) {
            refused.push(format!("{} ({why})", row.backend));
            return true;
        }
        if let Err(why) = can_carry(current, row, turn) {
            refused.push(format!("{} ({why})", row.backend));
            return true;
        }
        false
    });

    let Some(to) = target else {
        return Decision::Ask {
            reason: format!(
                "{} is out ({}) and there is nowhere to move this conversation: {}.",
                current.backend,
                found.evidence,
                if refused.is_empty() {
                    "it is the only backend this agent lists".to_string()
                } else {
                    refused.join(", ")
                }
            ),
        };
    };

    let next = &chain.entries[to];
    if turn.ask_on_switch {
        return Decision::Ask {
            reason: format!(
                "{} is out ({}). {} is free — say the word and this conversation moves there.",
                current.backend, found.evidence, next.backend
            ),
        };
    }

    Decision::Switch {
        to,
        notice: notice(&current.backend, &next.backend, found),
    }
}

/// The one line a chat is told when its backend changes: what it is on now, and why it moved.
///
/// The PRD's own wording, and its shape is the point — the new backend first, because that is what
/// the reader needs, then the old one and when it comes back, because that is what they will ask
/// next.
#[must_use]
pub fn notice(from: &str, to: &str, found: &Classification) -> String {
    let until = if found.hold_for > 0 {
        format!(
            " until {}",
            crate::llm::holds::clock(now_unix() + found.hold_for)
        )
    } else {
        String::new()
    };
    format!("switched to {to}, {from} limited{until}")
}

/// Whether a conversation running on `from` can be handed to `to` with the whole of itself.
///
/// `Err` carries the sentence explaining the refusal, which goes straight into the notice the
/// person reads — a switch refused without a reason is indistinguishable from a switch that broke.
///
/// # What can actually carry a conversation
///
/// This is a fact about the *runtime*, not about the model, and the four behave differently:
///
/// * `harness:adi` continues by replaying ADI's own committed transcript into the provider on every
///   turn, so it can be handed any conversation this store holds, in full. It is the one runtime
///   that satisfies requirement 7 unconditionally.
/// * `harness:claude-sdk` continues with `claude --resume <session-id>` against Claude's own store,
///   which lives beside the credential's settings. It can therefore take a conversation only from
///   *itself on the same credential* — which is the Opus-to-Sonnet fall this design is largely for,
///   and where it works exactly right. From anywhere else there is no session on that side to
///   resume.
/// * `process:codex` has the same boundary: it continues with `codex exec resume <thread-id>` and
///   can change model on the same login, but another credential has no copy of that thread.
/// * `process:claude` establishes no session at all, so a run on it is a single message with
///   nothing behind it. Handing it that message again is a complete move — which is why the history
///   check is skipped when there is no history.
/// * `pty:*` is a live terminal and ADI keeps no transcript of one. Nothing can be replayed into it
///   and nothing replayed out.
///
/// # Errors
/// A sentence naming what stops the move: the runtime, the credential, or the context window.
pub fn can_carry(
    from: &ResolvedBackend,
    to: &ResolvedBackend,
    turn: &Failure<'_>,
) -> Result<(), String> {
    if to.runtime.executor() == "pty" {
        return Err("a terminal session cannot be handed a conversation".to_string());
    }
    if turn.has_history {
        if from.runtime.executor() == "pty" {
            return Err("a terminal session keeps no transcript to move".to_string());
        }
        if !carries_history(&from.runtime, &to.runtime, from.credential == to.credential) {
            return Err(format!(
                "{} cannot be handed a conversation that started on {}",
                to.runtime, from.runtime
            ));
        }
        // Requirement 7's loud failure. Never silently truncated: a conversation that arrives at its
        // new backend missing its own beginning is worse than one that stops and says it does not
        // fit, because nobody can see that it happened.
        if to.context_tokens > 0 && turn.history_tokens > to.context_tokens {
            return Err(format!(
                "this conversation is about {} tokens and {} holds {}",
                turn.history_tokens, to.backend, to.context_tokens
            ));
        }
    }
    Ok(())
}

/// The runtime pairs a conversation can actually cross. See [`can_carry`] for why each.
fn carries_history(from: &Backend, to: &Backend, same_credential: bool) -> bool {
    match to {
        Backend::HarnessAdi => true,
        Backend::HarnessClaudeSdk => {
            matches!(from, Backend::HarnessClaudeSdk) && same_credential
        }
        Backend::ProcessCodex => matches!(from, Backend::ProcessCodex) && same_credential,
        _ => false,
    }
}

/// A rough token count for a body of conversation text.
///
/// Four characters to the token — the usual approximation, and good enough for the one question it
/// is asked: *is this obviously too big for the next backend's window*. It is deliberately not
/// exact, and it errs high on prose in every language that is not English, which is the safe
/// direction for a check whose failure mode is a truncated conversation.
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX).div_ceil(4)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::{HoldScope, LimitRule, Resume};
    use crate::llm::chain::{AgentBackendEntry, ResolvedChain};
    use crate::llm::{LlmBackendManifest, catalog};
    use std::collections::BTreeMap;

    fn manifest(runtime: &str, model: &str, credential: &str, context: u64) -> LlmBackendManifest {
        LlmBackendManifest {
            runtime: runtime.into(),
            model: model.into(),
            context_tokens: context,
            provider: Some(credential.to_string()),
            ..LlmBackendManifest::default()
        }
    }

    /// Three rows on three separate logins, all on the runtime that can take a replay.
    fn chain_of(entries: &[(&str, LlmBackendManifest)]) -> PinnedChain {
        let catalog = catalog(
            entries
                .iter()
                .map(|(id, manifest)| crate::llm::LlmBackend {
                    id: (*id).to_string(),
                    manifest: manifest.clone(),
                })
                .collect(),
        );
        let rows: Vec<AgentBackendEntry> = entries
            .iter()
            .map(|(id, _)| AgentBackendEntry::new(*id))
            .collect();
        ResolvedChain::resolve(&rows, &catalog, None, None)
            .expect("resolve")
            .pin()
    }

    fn three() -> PinnedChain {
        chain_of(&[
            ("anthropic", manifest("harness:adi", "claude-opus-5", "a", 200_000)),
            ("codex", manifest("harness:adi", "gpt-5", "b", 400_000)),
            ("glm", manifest("harness:adi", "glm-5.3", "c", 128_000)),
        ])
    }

    fn free(_: &ResolvedBackend) -> Option<String> {
        None
    }

    fn turn(blocked: &dyn Fn(&ResolvedBackend) -> Option<String>) -> Failure<'_> {
        Failure {
            has_history: true,
            history_tokens: 0,
            ask_on_switch: false,
            blocked,
        }
    }

    fn quota() -> Classification {
        Classification {
            class: LimitClass::Quota,
            scope: HoldScope::Model,
            hold_for: 0,
            evidence: "usage limit reached".into(),
            matched: Some(0),
        }
    }

    /// The headline case: row 1 runs out, the conversation lands on row 2, and the chat is told in
    /// one line naming both.
    #[test]
    fn a_quota_moves_to_the_next_row_and_says_so_once() {
        let chain = three();
        let Decision::Switch { to, notice } = decide(&chain, &quota(), &turn(&free)) else {
            panic!("a quota with a free row below it switches");
        };
        assert_eq!(to, 1);
        assert!(notice.starts_with("switched to codex, anthropic limited"), "{notice}");
    }

    /// The shared hold's whole purpose, seen from the chain: a row another run already found spent
    /// is skipped without spending a turn discovering it again.
    #[test]
    fn a_row_another_run_already_found_spent_is_skipped_not_tried() {
        let chain = three();
        let held = |row: &ResolvedBackend| {
            (row.backend == "codex").then(|| "limited until 14:00 UTC".to_string())
        };
        let Decision::Switch { to, .. } = decide(&chain, &quota(), &turn(&held)) else {
            panic!("it moves past the held row");
        };
        assert_eq!(to, 2, "codex is held, so the fall is to glm");
    }

    /// Requirement 5's exception. A conversation never climbs back on its own, but once the row it
    /// is on runs out too, the best row it can still reach is the one to take — even one above it.
    #[test]
    fn a_second_failure_re_picks_the_best_row_and_not_the_next_one_down() {
        let mut chain = three();
        chain.move_to(1);
        let Decision::Switch { to, notice } = decide(&chain, &quota(), &turn(&free)) else {
            panic!("it switches");
        };
        assert_eq!(to, 0, "row 1 has recovered, so it is the best row available");
        assert!(notice.starts_with("switched to anthropic, codex limited"), "{notice}");
    }

    /// Every row out is the case that must not silently do nothing: the chat is told which backends
    /// were tried and why each one was refused.
    #[test]
    fn a_chain_with_nowhere_left_stops_and_names_what_it_tried() {
        let chain = three();
        let all_held = |row: &ResolvedBackend| Some(format!("{} is spent", row.backend));
        let Decision::Ask { reason } = decide(&chain, &quota(), &turn(&all_held)) else {
            panic!("it asks");
        };
        assert!(reason.contains("codex"), "{reason}");
        assert!(reason.contains("glm"), "{reason}");
        assert!(reason.contains("usage limit reached"), "{reason}");
    }

    /// A single-row chain has nothing behind it — and says that, rather than listing an empty set.
    #[test]
    fn a_one_row_chain_says_it_is_the_only_backend() {
        let chain = chain_of(&[("only", manifest("harness:adi", "m", "a", 0))]);
        let Decision::Ask { reason } = decide(&chain, &quota(), &turn(&free)) else {
            panic!("it asks");
        };
        assert!(reason.contains("the only backend"), "{reason}");
    }

    /// A dead token is not something the next row fixes, and not something waiting fixes either.
    #[test]
    fn a_broken_login_stops_even_with_a_free_row_below_it() {
        let chain = three();
        let auth = Classification {
            class: LimitClass::Auth,
            evidence: "invalid api key".into(),
            ..quota()
        };
        let Decision::Ask { reason } = decide(&chain, &auth, &turn(&free)) else {
            panic!("an auth failure asks rather than switching");
        };
        assert!(reason.contains("anthropic"), "{reason}");
        assert!(reason.contains("invalid api key"), "{reason}");
    }

    /// The safe default, from both ends: neither a blip nor an unclassified crash takes a working
    /// backend out of the chain.
    #[test]
    fn a_blip_and_an_unrecognised_error_both_leave_the_chain_alone() {
        let chain = three();
        for class in [LimitClass::Transient, LimitClass::Unknown] {
            let found = Classification { class, ..quota() };
            assert_eq!(
                decide(&chain, &found, &turn(&free)),
                Decision::Nothing,
                "{class:?}",
            );
        }
    }

    /// The global switch, and it applies to the class that would otherwise move silently.
    #[test]
    fn ask_on_switch_offers_the_move_instead_of_making_it() {
        let chain = three();
        let mut asking = turn(&free);
        asking.ask_on_switch = true;
        let Decision::Ask { reason } = decide(&chain, &quota(), &asking) else {
            panic!("it offers rather than moves");
        };
        assert!(reason.contains("codex is free"), "{reason}");
    }

    /// Requirement 7's loud failure. The conversation does not arrive at its new backend missing
    /// its own beginning — the switch is refused and says by how much it did not fit.
    #[test]
    fn a_conversation_too_big_for_the_next_window_refuses_the_switch() {
        let chain = chain_of(&[
            ("big", manifest("harness:adi", "m", "a", 400_000)),
            ("small", manifest("harness:adi", "m", "b", 8_000)),
        ]);
        let mut oversized = turn(&free);
        oversized.history_tokens = 120_000;
        let Decision::Ask { reason } = decide(&chain, &quota(), &oversized) else {
            panic!("it refuses rather than truncating");
        };
        assert!(reason.contains("120000"), "{reason}");
        assert!(reason.contains("8000"), "{reason}");
    }

    /// A first turn carries nothing, so the same pair that refuses a conversation takes a message.
    #[test]
    fn a_turn_with_no_history_behind_it_fits_anywhere() {
        let chain = chain_of(&[
            ("big", manifest("harness:adi", "m", "a", 400_000)),
            ("small", manifest("harness:adi", "m", "b", 8_000)),
        ]);
        let mut opening = turn(&free);
        opening.history_tokens = 120_000;
        opening.has_history = false;
        assert!(matches!(
            decide(&chain, &quota(), &opening),
            Decision::Switch { to: 1, .. },
        ));
    }

    /// The runtime table, which is the part of requirement 7 that cannot be met by trying harder.
    #[test]
    fn only_some_runtimes_can_be_handed_a_conversation_that_started_elsewhere() {
        let sdk_a = manifest("harness:claude-sdk", "claude-opus-5", "a", 0);
        let sdk_a_sonnet = manifest("harness:claude-sdk", "claude-sonnet-5", "a", 0);
        let sdk_b = manifest("harness:claude-sdk", "claude-opus-5", "b", 0);

        // Opus to Sonnet on one subscription: the same engine resuming its own session.
        let chain = chain_of(&[("opus", sdk_a.clone()), ("sonnet", sdk_a_sonnet)]);
        assert!(matches!(
            decide(&chain, &quota(), &turn(&free)),
            Decision::Switch { to: 1, .. },
        ));

        // The same engine on a different login has no session on that side to resume.
        let chain = chain_of(&[("mine", sdk_a.clone()), ("theirs", sdk_b)]);
        let Decision::Ask { reason } = decide(&chain, &quota(), &turn(&free)) else {
            panic!("it refuses a credential it cannot resume across");
        };
        assert!(reason.contains("theirs"), "{reason}");

        // The loop that replays ADI's own transcript takes a conversation from anywhere.
        let chain = chain_of(&[("mine", sdk_a), ("loop", manifest("harness:adi", "m", "c", 0))]);
        assert!(matches!(
            decide(&chain, &quota(), &turn(&free)),
            Decision::Switch { to: 1, .. },
        ));

        let codex_a = manifest("process:codex", "gpt-6-astra", "openai-a", 0);
        let codex_a_fast = manifest("process:codex", "gpt-5.6-luna", "openai-a", 0);
        let codex_b = manifest("process:codex", "gpt-6-astra", "openai-b", 0);

        // Codex likewise resumes its own thread while changing models on one login.
        let chain = chain_of(&[("astra", codex_a.clone()), ("luna", codex_a_fast)]);
        assert!(matches!(
            decide(&chain, &quota(), &turn(&free)),
            Decision::Switch { to: 1, .. },
        ));

        let chain = chain_of(&[("mine", codex_a), ("theirs", codex_b)]);
        let Decision::Ask { reason } = decide(&chain, &quota(), &turn(&free)) else {
            panic!("it refuses a Codex credential that has no copy of the thread");
        };
        assert!(reason.contains("theirs"), "{reason}");
    }

    /// A pty row is refused in both directions, and refused even on a first turn: there is no
    /// transcript of a terminal to move, and no way to hand one a queued message.
    #[test]
    fn a_terminal_row_is_never_switched_onto() {
        let chain = chain_of(&[
            ("loop", manifest("harness:adi", "m", "a", 0)),
            ("terminal", manifest("pty:claude", "claude-opus-5", "b", 0)),
        ]);
        let mut opening = turn(&free);
        opening.has_history = false;
        let Decision::Ask { reason } = decide(&chain, &quota(), &opening) else {
            panic!("a pty row is not a place to move to");
        };
        assert!(reason.contains("terminal"), "{reason}");
    }

    /// The notice names the reset time when the classification found one, because "limited" without
    /// a when is the half of the sentence nobody can act on.
    #[test]
    fn the_notice_carries_the_reset_time_when_the_provider_stated_one() {
        let with_time = Classification {
            hold_for: 3_600,
            ..quota()
        };
        let line = notice("anthropic", "codex", &with_time);
        assert!(line.starts_with("switched to codex, anthropic limited until "), "{line}");
        assert!(line.contains("UTC"), "{line}");

        assert_eq!(
            notice("anthropic", "codex", &quota()),
            "switched to codex, anthropic limited",
        );
    }

    #[test]
    fn a_rough_token_count_is_four_characters_to_the_token() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    /// The rules a real backend carries, run end to end: the provider's words classify, the class
    /// decides, and the notice quotes the reset the provider itself named.
    #[test]
    fn a_providers_own_words_drive_the_whole_decision() {
        let rules = vec![LimitRule {
            pattern: "(?i)usage limit reached".into(),
            class: LimitClass::Quota,
            scope: HoldScope::Model,
            resume: Resume::FromMessage,
            fixed: Some("4h".into()),
        }];
        let found = crate::llm::classify(
            "Claude usage limit reached. Your limit will reset in 2 hours.",
            &rules,
            None,
            0,
        );
        assert_eq!(found.hold_for, 7_200);

        let chain = three();
        let Decision::Switch { notice, .. } = decide(&chain, &found, &turn(&free)) else {
            panic!("it switches");
        };
        assert!(notice.contains("switched to codex"), "{notice}");
    }

    /// The overrides a row carries are the row's business, not the decision's — but a chain built
    /// from rows with overrides must still decide the same way, which is what this pins.
    #[test]
    fn rows_with_overrides_decide_no_differently() {
        let catalog = catalog(vec![
            crate::llm::LlmBackend {
                id: "anthropic".into(),
                manifest: manifest("harness:adi", "claude-opus-5", "a", 0),
            },
            crate::llm::LlmBackend {
                id: "codex".into(),
                manifest: manifest("harness:adi", "gpt-5", "b", 0),
            },
        ]);
        let rows = vec![
            AgentBackendEntry {
                backend: "anthropic".into(),
                overrides: BTreeMap::from([("thinking".to_string(), serde_json::json!("high"))]),
            },
            AgentBackendEntry::new("codex"),
        ];
        let chain = ResolvedChain::resolve(&rows, &catalog, None, None)
            .expect("resolve")
            .pin();
        assert!(matches!(
            decide(&chain, &quota(), &turn(&free)),
            Decision::Switch { to: 1, .. },
        ));
    }
}
