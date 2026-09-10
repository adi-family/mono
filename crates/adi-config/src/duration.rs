//! The one way a duration is written in a config file across the platform: `4h`, `30m`, `90s`, or
//! a bare number of seconds.
//!
//! It lives here because two subsystems that cannot see each other already read one: an LLM
//! backend's cooldown (`adi-agents`) and an on-demand service's idle window (`adi-hive`). Both
//! values are typed by the same person into the same kind of file, so `30m` had better mean the
//! same thing in both — and a second parser is exactly how it stops meaning it.

/// Read `"4h"` / `"30m"` / `"90s"` / `"3600"` as seconds. `None` when it is not a duration at all,
/// which callers read as "use the default" rather than as an error: a typo in a cooldown must
/// not be the reason a limit goes unrecorded, nor a typo in an idle window the reason a service
/// never stops.
#[must_use]
pub fn parse_duration(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (digits, multiplier) = match value.chars().last() {
        Some('h' | 'H') => (&value[..value.len() - 1], 3600),
        Some('m' | 'M') => (&value[..value.len() - 1], 60),
        Some('s' | 'S') => (&value[..value.len() - 1], 1),
        _ => (value, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn reads_the_suffixes_and_a_bare_number_of_seconds() {
        assert_eq!(parse_duration("4h"), Some(14_400));
        assert_eq!(parse_duration("30m"), Some(1_800));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration(" 45 "), Some(45));
        assert_eq!(parse_duration("1H"), Some(3_600));
    }

    #[test]
    fn anything_that_is_not_a_duration_is_none_rather_than_a_guess() {
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("-5m"), None);
        assert_eq!(parse_duration("1.5h"), None);
    }
}
