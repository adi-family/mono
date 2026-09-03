//! Getting an `adi-invite:` token out of whatever a person actually pasted.
//!
//! A token is one 900-character word, and it reaches the person who spends it by whatever route
//! the operator had — a chat message, an email, an App Store review note. Every one of those
//! routes mangles it in a way the *sender* never sees, because the sender is looking at the copy
//! that still works:
//!
//! - a phone's word selection ends at `:` and `-`, so a double-tap on the token puts the payload
//!   on the pasteboard and leaves `adi-invite:` behind on the screen;
//! - a mail client wraps the one long word, and what is copied back out carries the wrapping;
//! - a word processor swaps the hyphen for an en dash, and a chat client leaves a zero-width
//!   character in front of the `a`.
//!
//! Each of those arrives as a string that is not `adi-invite:…` by `strip_prefix`, and the person
//! holding it is then told that the thing they pasted is not the thing they pasted — a refusal
//! they can neither see nor debug, on the one screen where nothing else can happen until it
//! works. (It cost this project an App Store review on 2026-09-02: the reviewer could not pair,
//! and the app's own dialog was the only evidence of why.)
//!
//! So the prefix test happens here, over a normalised string and against several readings of it,
//! rather than by `strip_prefix` over whatever arrived. Nothing here validates anything — it only
//! proposes readings, cheaply and in order of confidence. [`crate::join::decode_invite`] is still
//! the one that decides, and a reading that is not a real invite dies there as before.

/// The prefix a canonical token carries. The one spelling everything else is normalised *to*.
pub const PREFIX: &str = "adi-invite:";

/// The name in front of the payload, without its separator — matched case-insensitively, since a
/// keyboard that autocapitalises has already changed it once.
const MARKER: &str = "adi-invite";

/// Hex of `{`. Every payload in this token family is a JSON object, which is what makes a bare
/// payload recognisable as one rather than as some other hex string.
const JSON_OBJECT: &str = "7b";

/// Every canonical token worth trying, most likely first.
///
/// Usually one, occasionally two, empty when the string names no invite at all. The caller tries
/// them in order and keeps the first that decodes, so a reading that is wrong costs one failed
/// parse and no I/O.
#[must_use]
pub fn candidates(input: &str) -> Vec<String> {
    let cleaned = clean(input);
    let lower = ascii_lower(&cleaned);
    let mut out = Vec::new();

    let mut from = 0;
    while let Some(start) = payload_start(&lower, from) {
        // Cut the region at the next marker, so a paste carrying *several* tokens reads as
        // several tokens rather than as the first one with the second's leading digits glued on:
        // `a` and `d` are hex digits, so `adi-invite:` continues a run it ought to end.
        let end = payload_start(&lower, start).map_or(cleaned.len(), |next| {
            lower[..next].rfind(MARKER).unwrap_or(cleaned.len())
        });
        let region = &cleaned[start..end];
        push(&mut out, contiguous_hex(region));
        push(&mut out, unwrapped_hex(region));
        from = start;
    }

    push(&mut out, bare_payload(&cleaned));
    out
}

/// A sentence naming what arrived *instead of* a token, for the error that says so.
///
/// It describes the shape and never the content. A string that failed to parse is a string that
/// might still be a live credential for somebody's machine — the failure could be ours — and an
/// error message is the one place a credential is certain to be screenshotted.
#[must_use]
pub fn describe(input: &str) -> String {
    let cleaned = clean(input);
    let trimmed = cleaned.trim();
    let lower = ascii_lower(trimmed);

    if trimmed.is_empty() {
        return "nothing was pasted".into();
    }
    if lower.starts_with("adimesh:") {
        return "that is an `adimesh:` ticket — a machine's address, not an invite to it (mint \
                one on that machine with `adi-mono mesh invite`)"
            .into();
    }
    if lower.contains(MARKER) {
        return "the name is there but what follows it is not a payload — a token is one long \
                word, so it has to be copied whole"
            .into();
    }
    format!(
        "what arrived is {} characters long and carries no `adi-invite:` anywhere in it",
        trimmed.chars().count()
    )
}

/// Fold away the characters that survive a copy without being visible in one.
///
/// None of these is whitespace, so nothing downstream would have removed them, and exactly one of
/// them in front of the `a` is the whole difference between a token and a refusal.
fn clean(input: &str) -> String {
    input
        .chars()
        .filter_map(|c| match c {
            // Zero-width spaces and joiners, the directional marks, the word joiner, a BOM that
            // came along with a file, and a soft hyphen a renderer inserted at a line break.
            '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{00ad}' => None,
            // Spaces that are not `' '`, folded so "whitespace" downstream is one thing.
            '\u{00a0}' | '\u{2007}' | '\u{202f}' => Some(' '),
            // Every dash a word processor or a chat client substitutes for the one in the name.
            '\u{2010}'..='\u{2015}' | '\u{2212}' => Some('-'),
            c => Some(c),
        })
        .collect()
}

/// Lowercase the ASCII and leave everything else alone — which keeps byte offsets identical to
/// the string it was made from, so an index found here is an index there. `to_lowercase` does not
/// promise that.
fn ascii_lower(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// Where the payload begins after the next marker at or past `from`, if there is one.
///
/// Accepts the separators a token picks up in transit as well as the one it is minted with: a
/// plain `:`, the `%3A` of a URL it was carried inside, and the `//` of a scheme a client
/// linkified it into.
fn payload_start(lower: &str, from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        let found = at + lower.get(at..)?.find(MARKER)?;
        let after = found + MARKER.len();
        let rest = lower.get(after..)?;
        for separator in ["://", "%3a", ":"] {
            if rest.starts_with(separator) {
                return Some(after + separator.len());
            }
        }
        at = after;
    }
}

/// The ordinary reading: the run of hex that starts right where the payload does.
fn contiguous_hex(region: &str) -> Option<String> {
    let hex: String = region.chars().take_while(char::is_ascii_hexdigit).collect();
    (!hex.is_empty()).then(|| format!("{PREFIX}{hex}"))
}

/// The same payload with the line breaks taken back out of it.
///
/// What a mail client does to a 900-character word is wrap it; what a person copies back out is
/// the wrapping. Tried only after [`contiguous_hex`], so on an unwrapped token this reading is
/// never reached — and where it *is* reached, a stray word after the token makes it a candidate
/// that fails to decode rather than one that decodes wrongly.
fn unwrapped_hex(region: &str) -> Option<String> {
    let mut hex = String::new();
    for c in region.chars() {
        if c.is_ascii_hexdigit() {
            hex.push(c);
        } else if !c.is_whitespace() {
            break;
        }
    }
    (!hex.is_empty()).then(|| format!("{PREFIX}{hex}"))
}

/// The payload on its own, with the prefix the person never saw put back in front of it.
///
/// This is the double-tap: on iOS a word ends at `:` and `-`, so selecting "the token" selects
/// the hex. It is safe to accept because it is not ambiguous — the payload is hex of a JSON
/// object, so a bare payload is an even number of hex digits beginning `7b`, and a hex string
/// that is something else entirely does not look like that.
fn bare_payload(cleaned: &str) -> Option<String> {
    let hex: String = cleaned.chars().filter(|c| !c.is_whitespace()).collect();
    let looks_like_one = !hex.is_empty()
        && hex.len().is_multiple_of(2)
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && ascii_lower(&hex).starts_with(JSON_OBJECT);
    looks_like_one.then_some(format!("{PREFIX}{hex}"))
}

/// Add a reading unless it is already there — the readings agree far more often than not.
fn push(out: &mut Vec<String>, candidate: Option<String>) {
    if let Some(candidate) = candidate
        && !out.contains(&candidate)
    {
        out.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload shaped like a real one: hex of a JSON object.
    const PAYLOAD: &str = "7b2276223a317d";

    fn token() -> String {
        format!("{PREFIX}{PAYLOAD}")
    }

    fn first(input: &str) -> Option<String> {
        candidates(input).into_iter().next()
    }

    #[test]
    fn a_clean_token_reads_as_itself() {
        assert_eq!(first(&token()).as_deref(), Some(token().as_str()));
    }

    #[test]
    fn the_shapes_a_paste_arrives_in_all_read_as_the_token() {
        let cases = [
            // What the review note looked like, and what a terminal copy looks like.
            format!("1. {}", token()),
            format!("  {}  \n", token()),
            format!("adi-mono mesh join {}", token()),
            // Punctuation the token was wrapped in by whoever passed it on.
            format!("`{}`", token()),
            format!("\"{}\"", token()),
            format!("<{}>", token()),
            format!("{} /", token()),
            // A keyboard that autocapitalises, and a word processor that restyles the hyphen.
            format!("Adi-Invite:{PAYLOAD}"),
            format!("adi\u{2011}invite:{PAYLOAD}"),
            // A chat client's zero-width character, right where it does the most damage.
            format!("\u{200b}{}", token()),
            // Carried inside a URL, and percent-encoded on the way.
            format!("https://withadi.dev/pair?t={}", token()),
            format!("https://withadi.dev/pair?t=adi-invite%3A{PAYLOAD}"),
            // The double-tap: the payload, with the prefix left behind on screen.
            PAYLOAD.to_string(),
            // A mail client's wrapping, copied back out.
            format!("adi-invite:{}\n{}", &PAYLOAD[..6], &PAYLOAD[6..]),
        ];
        for case in cases {
            assert!(
                candidates(&case).contains(&token()),
                "{case:?} -> {:?}",
                candidates(&case)
            );
        }
    }

    #[test]
    fn two_tokens_in_one_paste_read_as_two_tokens() {
        let other = format!("{PREFIX}7b2276223a327d");
        let both = candidates(&format!("{} / {other}", token()));
        assert_eq!(both.first().map(String::as_str), Some(token().as_str()));
        assert!(both.contains(&other), "{both:?}");
    }

    #[test]
    fn a_string_that_names_no_invite_proposes_nothing() {
        for input in [
            "",
            "   ",
            "hello",
            "adi-invite",
            "adimesh:7b2276223a317d",
            "zzzz",
        ] {
            assert!(candidates(input).is_empty(), "{input:?}");
        }
    }

    #[test]
    fn describing_a_failure_names_its_shape_and_not_its_content() {
        assert!(describe("").contains("nothing"));
        assert!(describe("adimesh:7b2276223a317d").contains("address"));
        assert!(describe("adi-invite").contains("one long word"));

        let secret = "hunter2-and-then-some";
        let described = describe(secret);
        assert!(described.contains("21 characters"), "{described}");
        assert!(!described.contains(secret), "{described}");
    }
}
