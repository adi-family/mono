//! Normalising a workflow's `on: schedule(cron, tz)` cron expression into [`adi_schedules`]' own
//! dialect.
//!
//! Igor's decision: a workflow file writes the standard 5-field unix cron the spec's own examples
//! use (`"0 5 * * MON"` — minute hour day-of-month month day-of-week), not the `cron` crate's
//! 6/7-field one `adi-schedules` is built on (`sec min hour dom month dow [year]`). The two share
//! every field but the leading seconds, so normalizing is exactly prepending `"0 "` — the five
//! unix fields keep their own order and meaning unchanged. Anything that is not exactly 5
//! whitespace-separated fields is refused here, with a message naming the field count, rather than
//! silently handed to `cron::Schedule` one field short (which would parse day-of-month as seconds
//! and shift every other field with it).

use crate::error::{Error, Result};

/// The number of fields standard unix cron always has.
const UNIX_CRON_FIELDS: usize = 5;

/// Convert `cron` from 5-field unix cron (`min hour dom month dow`) to the 6-field dialect
/// [`adi_schedules::ScheduleRule`] expects (`sec min hour dom month dow`), by prepending a literal
/// `"0"` seconds field — the five unix fields are passed through unchanged, in the same order.
///
/// # Errors
/// [`Error::InvalidCron`] when `cron` does not split into exactly 5 whitespace-separated fields —
/// including a bare 6/7-field expression in the engine's own dialect, which this rejects on sight
/// rather than reinterpreting.
pub fn normalize_five_field(cron: &str) -> Result<String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != UNIX_CRON_FIELDS {
        return Err(Error::InvalidCron(
            cron.to_string(),
            format!(
                "expected {UNIX_CRON_FIELDS} fields (min hour dom month dow), got {}",
                fields.len()
            ),
        ));
    }
    Ok(format!("0 {}", fields.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standard_five_field_expression_gets_a_leading_seconds_field() {
        assert_eq!(normalize_five_field("0 5 * * MON").unwrap(), "0 0 5 * * MON");
        assert_eq!(normalize_five_field("*/15 * * * *").unwrap(), "0 */15 * * * *");
    }

    #[test]
    fn extra_whitespace_between_fields_is_tolerated() {
        assert_eq!(
            normalize_five_field("  0   5  *   *  MON  ").unwrap(),
            "0 0 5 * * MON"
        );
    }

    #[test]
    fn a_bare_six_field_expression_in_the_engine_dialect_is_refused() {
        // Exactly the trap the module doc warns about: silently accepting this would read "0" as
        // minutes and shift day-of-month into month's place.
        let err = normalize_five_field("0 0 5 * * MON").unwrap_err();
        assert!(matches!(err, Error::InvalidCron(_, ref detail) if detail.contains("got 6")));
    }

    #[test]
    fn a_seven_field_expression_with_a_year_is_refused() {
        let err = normalize_five_field("0 0 5 * * MON 2026").unwrap_err();
        assert!(matches!(err, Error::InvalidCron(_, ref detail) if detail.contains("got 7")));
    }

    #[test]
    fn too_few_fields_is_refused() {
        let err = normalize_five_field("* * *").unwrap_err();
        assert!(matches!(err, Error::InvalidCron(_, ref detail) if detail.contains("got 3")));
    }

    #[test]
    fn empty_input_is_refused() {
        let err = normalize_five_field("   ").unwrap_err();
        assert!(matches!(err, Error::InvalidCron(_, ref detail) if detail.contains("got 0")));
    }

    /// The normalized string must actually parse under `adi-schedules`' own cron dialect — the
    /// real end-to-end check, not just a string shape assertion.
    #[test]
    fn the_normalized_expression_parses_as_a_valid_schedule_rule() {
        let normalized = normalize_five_field("0 5 * * MON").unwrap();
        let rule = adi_schedules::ScheduleRule::new("wf", normalized, "UTC");
        let now = chrono::Utc::now();
        assert!(adi_schedules::due(&rule, None, now).is_ok());
    }
}
