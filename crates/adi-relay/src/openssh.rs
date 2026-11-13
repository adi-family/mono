//! OpenSSH rendering and parsing of the node's ed25519 *public* signing key (`docs/webhook-relay.md`
//! §2, §6) — the one-line `ssh-ed25519 AAAA… [comment]` format an operator already knows from
//! `~/.ssh/authorized_keys`, and nothing more of it: no other key type, no private-key format, no
//! comment-aware editing. Hand-rolled rather than a `ssh-key` dependency: the wire format a
//! public-key line encodes is two length-prefixed strings (`"ssh-ed25519"`, then the 32 raw
//! bytes), and this is the only direction either side of the relay ever needs.

use std::fmt;

use base64::Engine as _;
use ed25519_dalek::{PUBLIC_KEY_LENGTH, SignatureError, VerifyingKey};
use sha2::{Digest as _, Sha256};

const KEY_TYPE: &str = "ssh-ed25519";

/// Render `key` as the one-line OpenSSH public-key format: `ssh-ed25519 AAAA…`, with no trailing
/// comment — the line every `X-Relay-Key` header and every `register` response's echo carries.
#[must_use]
pub fn encode(key: &VerifyingKey) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(blob(key.as_bytes()));
    format!("{KEY_TYPE} {b64}")
}

/// The wire blob inside the base64: `string("ssh-ed25519") || string(32 raw bytes)`, each
/// `string` a 4-byte big-endian length prefix followed by its bytes — the standard OpenSSH
/// public-key wire format (RFC 4253 §6.6), restricted to the one key type this relay ever uses.
fn blob(public: &[u8; PUBLIC_KEY_LENGTH]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + KEY_TYPE.len() + 4 + PUBLIC_KEY_LENGTH);
    push_ssh_string(&mut out, KEY_TYPE.as_bytes());
    push_ssh_string(&mut out, public);
    out
}

fn push_ssh_string(out: &mut Vec<u8>, value: &[u8]) {
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

/// Everything that can go wrong turning a line of text back into a key.
#[derive(Debug)]
pub enum ParseError {
    /// The line has no `ssh-ed25519 <base64>` pair to read at all.
    Empty,
    /// The first field names a key type other than `ssh-ed25519` — this relay accepts no other.
    WrongType(String),
    /// The second field isn't valid base64.
    BadBase64,
    /// The decoded blob ends before a length-prefixed string it names finishes.
    Truncated,
    /// The key-bytes string inside the blob isn't exactly 32 bytes.
    BadLength(usize),
    /// The 32 bytes don't decode to a valid ed25519 point.
    BadPoint(SignatureError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty or missing ssh-ed25519 public-key line"),
            Self::WrongType(got) => write!(f, "expected key type ssh-ed25519, got {got:?}"),
            Self::BadBase64 => write!(f, "the key field is not valid base64"),
            Self::Truncated => write!(f, "the decoded key blob is truncated"),
            Self::BadLength(n) => write!(f, "expected a 32-byte ed25519 key, got {n} bytes"),
            Self::BadPoint(e) => write!(f, "not a valid ed25519 public key: {e}"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BadPoint(e) => Some(e),
            _ => None,
        }
    }
}

/// Parse one `ssh-ed25519 AAAA… [comment]` line (the `X-Relay-Key` header value, or one line of
/// an `authorized_keys` file) back into its key. Only the first two whitespace-separated fields
/// matter; a trailing comment — or its absence — is ignored either way.
///
/// # Errors
/// See [`ParseError`]'s variants.
pub fn parse(line: &str) -> Result<VerifyingKey, ParseError> {
    let mut fields = line.split_whitespace();
    let kind = fields.next().ok_or(ParseError::Empty)?;
    if kind != KEY_TYPE {
        return Err(ParseError::WrongType(kind.to_string()));
    }
    let b64 = fields.next().ok_or(ParseError::Empty)?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| ParseError::BadBase64)?;
    let public = parse_blob(&blob)?;
    VerifyingKey::from_bytes(&public).map_err(ParseError::BadPoint)
}

fn parse_blob(blob: &[u8]) -> Result<[u8; PUBLIC_KEY_LENGTH], ParseError> {
    let mut pos = 0;
    let type_bytes = read_ssh_string(blob, &mut pos)?;
    if type_bytes != KEY_TYPE.as_bytes() {
        return Err(ParseError::WrongType(
            String::from_utf8_lossy(type_bytes).into_owned(),
        ));
    }
    let key_bytes = read_ssh_string(blob, &mut pos)?;
    key_bytes
        .try_into()
        .map_err(|_| ParseError::BadLength(key_bytes.len()))
}

fn read_ssh_string<'a>(buf: &'a [u8], pos: &mut usize) -> Result<&'a [u8], ParseError> {
    let len_bytes = buf.get(*pos..*pos + 4).ok_or(ParseError::Truncated)?;
    let len = u32::from_be_bytes(len_bytes.try_into().expect("checked 4 bytes")) as usize;
    *pos += 4;
    let value = buf.get(*pos..*pos + len).ok_or(ParseError::Truncated)?;
    *pos += len;
    Ok(value)
}

/// Every `ssh-ed25519` line in `text` that parses — one per non-blank, non-`#`-comment line, the
/// same tolerance real `sshd` has for an `authorized_keys` file (§6's self-host mode): a line that
/// fails to parse is skipped, not fatal to the rest of the file, because an operator's typo on one
/// line must not lock every other key out too.
#[must_use]
pub fn parse_authorized_keys(text: &str) -> Vec<VerifyingKey> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| parse(line).ok())
        .collect()
}

/// A stable identifier for a public key, independent of how many times it's transmitted: sha256
/// of the OpenSSH line's `type base64` prefix (never a trailing comment, which isn't part of the
/// key), hex-encoded. This is `endpoints.owner_fingerprint` (`docs/webhook-relay.md` §8's schema)
/// and the value every nonce row is scoped by (§3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Fingerprint an already-rendered `ssh-ed25519 AAAA… [comment]` line.
    #[must_use]
    pub fn of_line(line: &str) -> Self {
        let mut fields = line.split_whitespace();
        let normalized = match (fields.next(), fields.next()) {
            (Some(kind), Some(b64)) => format!("{kind} {b64}"),
            _ => line.trim().to_string(),
        };
        Self::of_bytes(normalized.as_bytes())
    }

    /// Fingerprint a key directly, equal to [`Self::of_line`] on [`encode`]'s own output for it.
    #[must_use]
    pub fn of_key(key: &VerifyingKey) -> Self {
        Self::of_bytes(encode(key).as_bytes())
    }

    fn of_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> VerifyingKey {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        signing.verifying_key()
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let k = key(1);
        let line = encode(&k);
        assert!(line.starts_with("ssh-ed25519 "));
        assert_eq!(parse(&line).expect("parses"), k);
    }

    #[test]
    fn a_trailing_comment_is_ignored() {
        let k = key(2);
        let line = format!("{} user@host", encode(&k));
        assert_eq!(parse(&line).expect("parses"), k);
    }

    #[test]
    fn wrong_key_type_is_refused() {
        assert!(matches!(
            parse("ssh-rsa AAAA"),
            Err(ParseError::WrongType(t)) if t == "ssh-rsa"
        ));
    }

    #[test]
    fn bad_base64_is_refused() {
        assert!(matches!(
            parse("ssh-ed25519 not-base64!!"),
            Err(ParseError::BadBase64)
        ));
    }

    #[test]
    fn empty_line_is_refused() {
        assert!(matches!(parse(""), Err(ParseError::Empty)));
        assert!(matches!(parse("ssh-ed25519"), Err(ParseError::Empty)));
    }

    #[test]
    fn a_blob_naming_the_wrong_type_inside_is_refused() {
        // Valid outer field says ssh-ed25519, but the wire blob inside lies about its own type.
        let mut blob = Vec::new();
        push_ssh_string(&mut blob, b"ssh-rsa");
        push_ssh_string(&mut blob, &[7u8; 32]);
        let line = format!(
            "ssh-ed25519 {}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        );
        assert!(matches!(parse(&line), Err(ParseError::WrongType(_))));
    }

    #[test]
    fn authorized_keys_skips_comments_blanks_and_bad_lines() {
        let k1 = key(3);
        let k2 = key(4);
        let text = format!(
            "# a comment\n\n{}\nssh-rsa AAAA bad-type\n{} op@laptop\n",
            encode(&k1),
            encode(&k2)
        );
        assert_eq!(parse_authorized_keys(&text), vec![k1, k2]);
    }

    #[test]
    fn fingerprint_ignores_a_trailing_comment_but_not_the_key() {
        let k1 = key(5);
        let k2 = key(6);
        let bare = encode(&k1);
        let commented = format!("{bare} someone@somewhere");
        assert_eq!(Fingerprint::of_line(&bare), Fingerprint::of_line(&commented));
        assert_eq!(Fingerprint::of_line(&bare), Fingerprint::of_key(&k1));
        assert_ne!(Fingerprint::of_key(&k1), Fingerprint::of_key(&k2));
    }
}
