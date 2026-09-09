//! Reading a failure against a backend's own rules: what happened, how wide it spreads, and when
//! to try again.
//!
//! This is the part that must not be one regex. The difference between "your subscription is spent
//! for four hours" and "your token is invalid" is the difference between moving to the next backend
//! without saying much and stopping to ask a human — and getting it wrong in the permissive
//! direction is expensive in a way that is hard to see: an agent that silently reroutes every error
//! is an agent whose chain quietly empties while somebody believes it is working.
//!
//! So the default is [`LimitClass::Unknown`], which holds nothing and reroutes nothing. A backend
//! earns automatic failover by having somebody write a rule that says what its limit message looks
//! like.

use crate::llm::backend::{HoldScope, LimitClass, LimitRule, Resume};

/// What a failure turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// What happened, and therefore what the run does next.
    pub class: LimitClass,
    /// How wide a hold would spread.
    pub scope: HoldScope,
    /// How many seconds to hold it for. `0` when nothing should be held.
    pub hold_for: u64,
    /// The line that decided it — the provider's own words, kept so a human can see why their
    /// agent moved and catch a regex that matched something it should not have.
    pub evidence: String,
    /// Which of the backend's rules matched, by index. `None` when none did.
    pub matched: Option<usize>,
}

impl Classification {
    /// The classification of a failure nothing recognised: surface it, hold nothing, ask.
    #[must_use]
    pub fn unknown(evidence: impl Into<String>) -> Self {
        Self {
            class: LimitClass::Unknown,
            scope: HoldScope::Model,
            hold_for: 0,
            evidence: evidence.into(),
            matched: None,
        }
    }

    /// Whether this failure should be written into the shared hold store.
    #[must_use]
    pub fn should_hold(&self) -> bool {
        self.class.holds() && self.hold_for > 0
    }
}

/// Classify `text` — the error the backend produced, status line and body together — against a
/// backend's rules, in order, first match winning.
///
/// `retry_after` is the `Retry-After` header in seconds where the caller has one, and `now` is the
/// current unix time (taken by the caller so a classification is reproducible in a test).
#[must_use]
pub fn classify(
    text: &str,
    rules: &[LimitRule],
    retry_after: Option<u64>,
    now: u64,
) -> Classification {
    for (index, rule) in rules.iter().enumerate() {
        // A rule that does not compile is skipped rather than fatal: the save path already refuses
        // one, so this can only be a hand-edited file, and one bad rule must not stop the good
        // rules under it from classifying a real limit.
        let Ok(pattern) = regex::Regex::new(&rule.pattern) else {
            continue;
        };
        let Some(hit) = pattern.find(text) else {
            continue;
        };
        let hold_for = if rule.class.holds() {
            wait_for(rule, text, retry_after, now)
        } else {
            0
        };
        return Classification {
            class: rule.class,
            scope: rule.scope,
            hold_for,
            evidence: evidence_line(text, hit.start()),
            matched: Some(index),
        };
    }
    Classification::unknown(evidence_line(text, 0))
}

/// How long to hold, from whichever source this rule names.
fn wait_for(rule: &LimitRule, text: &str, retry_after: Option<u64>, now: u64) -> u64 {
    match rule.resume {
        Resume::RetryAfter => retry_after.unwrap_or_else(|| rule.fixed_seconds()),
        Resume::Fixed => rule.fixed_seconds(),
        Resume::FromMessage => parse_wait(text, now)
            .or(retry_after)
            .unwrap_or_else(|| rule.fixed_seconds()),
    }
}

/// Read a wait out of the provider's own words.
///
/// Two shapes cover nearly everything the providers actually say: a duration ("try again in 3
/// hours") and a wall clock ("resets at 14:00"). A clock is the rougher of the two — it is quoted
/// in the account's own timezone, which the error does not state — so it is read as UTC and rolled
/// forward to the next occurrence.
///
/// Being wrong here is survivable in both directions, which is why an approximation is acceptable:
/// too early and the backend re-trips its own rule and is held again with a longer wait, too late
/// and the [prober](crate::llm::holds) is what pays, not a conversation.
#[must_use]
pub fn parse_wait(text: &str, now: u64) -> Option<u64> {
    if let Some(seconds) = parse_relative(text) {
        return Some(seconds);
    }
    parse_clock(text, now)
}

/// "try again in 3 hours", "retry in 45 seconds", "available again in 20 minutes".
fn parse_relative(text: &str) -> Option<u64> {
    let pattern = regex::Regex::new(
        r"(?i)\b(?:try again|retry|retry after|available again|resets?|resume[sd]?)\s+in\s+(\d+)\s*(second|sec|minute|min|hour|hr|day)s?\b",
    )
    .ok()?;
    let hit = pattern.captures(text)?;
    let count: u64 = hit.get(1)?.as_str().parse().ok()?;
    let unit = hit.get(2)?.as_str().to_ascii_lowercase();
    let seconds = match unit.as_str() {
        "second" | "sec" => 1,
        "minute" | "min" => 60,
        "hour" | "hr" => 3_600,
        "day" => 86_400,
        _ => return None,
    };
    Some(count.saturating_mul(seconds))
}

/// "resets at 14:00", "try again at 9:30".
fn parse_clock(text: &str, now: u64) -> Option<u64> {
    let pattern = regex::Regex::new(
        r"(?i)\b(?:resets?|try again|retry|available again|resume[sd]?)\s+at\s+(\d{1,2}):(\d{2})",
    )
    .ok()?;
    let hit = pattern.captures(text)?;
    let hours: u64 = hit.get(1)?.as_str().parse().ok()?;
    let minutes: u64 = hit.get(2)?.as_str().parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let target_of_day = hours * 3_600 + minutes * 60;
    let day_start = now - (now % 86_400);
    let mut target = day_start + target_of_day;
    if target <= now {
        target += 86_400;
    }
    Some(target - now)
}

/// The line the match fell on, trimmed — enough evidence to read, not a whole stack trace.
fn evidence_line(text: &str, at: usize) -> String {
    const MAX: usize = 240;
    let at = at.min(text.len());
    let start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |index| at + index);
    let line = text[start..end].trim();
    let line = if line.is_empty() { text.trim() } else { line };
    if line.chars().count() > MAX {
        let cut: String = line.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, class: LimitClass) -> LimitRule {
        LimitRule {
            pattern: pattern.into(),
            class,
            scope: HoldScope::Model,
            resume: Resume::FromMessage,
            fixed: Some("4h".into()),
        }
    }

    fn anthropic_rules() -> Vec<LimitRule> {
        vec![
            rule("(?i)usage limit reached|5-hour limit", LimitClass::Quota),
            LimitRule {
                scope: HoldScope::Login,
                ..rule("(?i)invalid api key|authentication_error", LimitClass::Auth)
            },
            rule("(?i)rate limit|429", LimitClass::Rate),
        ]
    }

    #[test]
    fn a_quota_message_classifies_and_holds() {
        let found = classify(
            "Claude usage limit reached. Your limit will reset at 14:00.",
            &anthropic_rules(),
            None,
            0,
        );
        assert_eq!(found.class, LimitClass::Quota);
        assert_eq!(found.scope, HoldScope::Model);
        assert_eq!(found.matched, Some(0));
        assert!(found.should_hold());
        assert!(found.evidence.contains("usage limit reached"));
    }

    /// The first matching rule wins, so a specific rule above a general one is what an operator
    /// writes to get the specific answer.
    #[test]
    fn the_first_matching_rule_wins() {
        let rules = vec![
            rule("(?i)usage limit", LimitClass::Quota),
            rule("(?i)limit", LimitClass::Rate),
        ];
        assert_eq!(
            classify("usage limit reached", &rules, None, 0).class,
            LimitClass::Quota
        );
    }

    /// An auth failure holds the whole login, because a cooldown would never fix it and every model
    /// on that credential is equally dead.
    #[test]
    fn an_auth_failure_is_login_scoped() {
        let found = classify(
            "API error: authentication_error — invalid api key",
            &anthropic_rules(),
            None,
            0,
        );
        assert_eq!(found.class, LimitClass::Auth);
        assert_eq!(found.scope, HoldScope::Login);
        assert!(found.should_hold());
        assert!(!found.class.reroutes_silently(), "it stops and asks");
    }

    /// The safe default. An error nobody wrote a rule for must not take a working backend out of
    /// every agent's chain, and must not be silently routed around.
    #[test]
    fn an_unrecognised_error_holds_nothing_and_reroutes_nothing() {
        let found = classify("Segmentation fault", &anthropic_rules(), None, 0);
        assert_eq!(found.class, LimitClass::Unknown);
        assert_eq!(found.matched, None);
        assert!(!found.should_hold());
        assert!(!found.class.reroutes_silently());
        assert_eq!(found.evidence, "Segmentation fault");
    }

    #[test]
    fn a_backend_with_no_rules_classifies_everything_as_unknown() {
        assert_eq!(
            classify("usage limit reached", &[], None, 0).class,
            LimitClass::Unknown
        );
    }

    /// A hand-edited file may hold a rule that does not compile; it is skipped, and the rules under
    /// it still get their chance.
    #[test]
    fn an_uncompilable_rule_is_skipped_rather_than_fatal() {
        let rules = vec![
            rule("([unclosed", LimitClass::Auth),
            rule("(?i)usage limit", LimitClass::Quota),
        ];
        let found = classify("usage limit reached", &rules, None, 0);
        assert_eq!(found.class, LimitClass::Quota);
        assert_eq!(found.matched, Some(1));
    }

    #[test]
    fn a_relative_wait_is_read_out_of_the_message() {
        assert_eq!(parse_wait("try again in 3 hours", 0), Some(10_800));
        assert_eq!(parse_wait("retry in 45 seconds", 0), Some(45));
        assert_eq!(parse_wait("available again in 20 minutes", 0), Some(1_200));
        assert_eq!(parse_wait("resets in 1 day", 0), Some(86_400));
    }

    #[test]
    fn a_wall_clock_rolls_forward_to_the_next_occurrence() {
        // 12:00 on day zero, resetting at 14:00 — two hours away.
        assert_eq!(parse_wait("resets at 14:00", 43_200), Some(7_200));
        // 15:00, resetting at 14:00 — that is tomorrow's 14:00, 23 hours away.
        assert_eq!(parse_wait("resets at 14:00", 54_000), Some(82_800));
    }

    #[test]
    fn a_nonsense_clock_is_not_a_wait() {
        assert_eq!(parse_wait("resets at 99:99", 0), None);
        assert_eq!(parse_wait("nothing to see", 0), None);
    }

    #[test]
    fn from_message_falls_back_to_retry_after_then_to_fixed() {
        let rules = vec![rule("(?i)usage limit", LimitClass::Quota)];
        // Nothing parseable in the text, but a header — the header wins over the fixed default.
        let with_header = classify("usage limit", &rules, Some(90), 0);
        assert_eq!(with_header.hold_for, 90);
        // Neither — the rule's own fixed wait.
        let bare = classify("usage limit", &rules, None, 0);
        assert_eq!(bare.hold_for, 14_400);
    }

    #[test]
    fn retry_after_reads_the_header_and_fixed_ignores_everything() {
        let header_rule = LimitRule {
            resume: Resume::RetryAfter,
            ..rule("(?i)rate limit", LimitClass::Rate)
        };
        assert_eq!(
            classify("rate limit, try again in 9 hours", &[header_rule], Some(30), 0).hold_for,
            30
        );

        let fixed_rule = LimitRule {
            resume: Resume::Fixed,
            fixed: Some("30m".into()),
            ..rule("(?i)rate limit", LimitClass::Rate)
        };
        assert_eq!(
            classify("rate limit, try again in 9 hours", &[fixed_rule], Some(30), 0).hold_for,
            1_800
        );
    }

    /// A transient blip is retried in place, so it names no wait at all.
    #[test]
    fn a_transient_class_holds_for_nothing() {
        let rules = vec![rule("(?i)connection reset", LimitClass::Transient)];
        let found = classify("connection reset by peer", &rules, None, 0);
        assert_eq!(found.class, LimitClass::Transient);
        assert_eq!(found.hold_for, 0);
        assert!(!found.should_hold());
    }

    #[test]
    fn the_evidence_is_the_line_the_match_fell_on() {
        let text = "starting up\nClaude usage limit reached, resets at 14:00\ngiving up";
        let found = classify(text, &anthropic_rules(), None, 0);
        assert_eq!(found.evidence, "Claude usage limit reached, resets at 14:00");
    }

    #[test]
    fn a_very_long_evidence_line_is_cut() {
        let text = format!("usage limit reached {}", "x".repeat(1_000));
        let found = classify(&text, &anthropic_rules(), None, 0);
        assert!(found.evidence.chars().count() <= 241, "{}", found.evidence.len());
        assert!(found.evidence.ends_with('…'));
    }
}
