//! Basic authentication — the node-side HTTP gate (`docs/fleet.md` §5, checklist C3).
//!
//! The mesh grant is **machine**-scoped: once your laptop is paired, every process on it can
//! reach the node through your front door, and so can anyone else sitting at that laptop. The
//! password is the second, **human**-scoped layer, and it is enforced *on the node* — the side
//! that owns the data — never on the caller, which an attacker already controls.
//!
//! The gate is deliberately dumb about what it is gating. [`is_authorized`] looks at exactly
//! one thing: does this request head carry credentials that verify? It never inspects the
//! method, the path, or `Upgrade`, because "a gate that only covers plain requests is not a
//! gate" — a WebSocket handshake is an ordinary HTTP request until the `101`, so gating it is
//! a matter of *not* adding an exception, and there is a test pinning that.
//!
//! Three properties are worth stating, because each is a way this could have been wrong:
//!
//! - **Default-deny.** No credentials configured means nobody gets in, not everybody.
//! - **No plaintext at rest.** A [`Credential`] stores a random per-credential salt and
//!   `SHA-256(salt ‖ password)`. Salted SHA-256 is what the contract specifies; it is *not* a
//!   password-stretching KDF, so see the note on [`Credential::from_password`].
//! - **No early exit on a mismatch.** Both the username and the digest are compared in
//!   constant time and folded together, so response timing cannot be used to enumerate which
//!   usernames exist on a node.
//!
//! The head handed to [`parse_basic_credentials`] is the raw request bytes from the request
//! line (`GET / HTTP/1.1`) **up to and including** the blank line — the first line is skipped,
//! since a header can never be it. Parsing stops at that blank line even if the buffer runs on
//! into the body, or a request could smuggle its own `Authorization:` line in as body content.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::{Choice, ConstantTimeEq as _};

/// Bytes of salt minted per credential. Fixed-width on purpose: it makes `salt ‖ password`
/// unambiguous without a separator, and a stored salt of any other length is rejected as
/// corrupt rather than silently hashed.
const SALT_LEN: usize = 16;

/// Bytes in a SHA-256 digest.
const DIGEST_LEN: usize = 32;

/// The realm used when the caller's own is empty or unusable, so the `401` is always
/// well-formed.
const FALLBACK_REALM: &str = "adi";

/// The header this machine's stored mesh credential travels in.
///
/// Deliberately **not** `Authorization`: that one belongs to whatever the node is fronting. An
/// app behind the mesh that authenticates its own callers — every SPA sending
/// `Authorization: Bearer <jwt>` from its `fetch` — would otherwise have its token read by this
/// gate, fail `Basic` parsing, and draw a `401` challenge that pops the browser's password
/// prompt on an ordinary API call. One header, two owners, and the app is not the one holding
/// the password. So the mesh takes a header of its own and strips it before the request reaches
/// the service.
pub const MESH_AUTH_HEADER: &str = "X-Adi-Authorization";

/// [`MESH_AUTH_HEADER`] lowercased, for [`parse_basic_credentials_in`].
pub const MESH_AUTH_HEADER_LOWER: &[u8] = b"x-adi-authorization";

/// The body of the challenge response. Short and plain: a browser shows its own password
/// prompt, and only a non-browser client ever reads this.
const CHALLENGE_BODY: &str = "401 Unauthorized: this adi node requires a username and password.\n";

/// One stored username/password pair for a node, as it sits in a config file.
///
/// Serde-serializable so another module can embed it (`docs/fleet.md` B2 stores these per
/// registry entry). `salt` and `digest` are base64 so the whole thing survives a TOML round
/// trip as plain strings:
///
/// ```toml
/// user = "igor"
/// salt = "5Wm1V7cQ0Yy1Yx8Q1a2b3w=="
/// digest = "n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg="
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Credential {
    /// The username this credential authenticates. Stored in the clear — it is not a secret,
    /// but it is still compared in constant time so a node's user list cannot be probed.
    pub user: String,
    /// The per-credential random salt, base64. Random per credential, so two nodes (or two
    /// users) sharing a password do not share a digest.
    pub salt: String,
    /// `SHA-256(salt ‖ password)`, base64.
    pub digest: String,
}

impl Credential {
    /// Mint a credential for `user` with a fresh random salt.
    ///
    /// # Panics
    /// Never in practice: it panics only if the OS random source is unavailable, which is not
    /// a condition a node can meaningfully continue past.
    ///
    /// Note on the primitive: this is a salted SHA-256, as `docs/fleet.md` §5 specifies. It
    /// defends the *stored* form against rainbow tables and cross-node reuse; it is fast by
    /// design and therefore does not stretch a weak password against an offline cracker who
    /// has stolen the config. Argon2/scrypt is the upgrade path if that threat is ever in
    /// scope — the on-disk shape already carries a salt, so it is a field, not a redesign.
    #[must_use]
    pub fn from_password(user: &str, password: &str) -> Self {
        use rand::TryRng as _;
        let mut salt = [0u8; SALT_LEN];
        rand::rngs::SysRng
            .try_fill_bytes(&mut salt)
            .expect("the OS random source is unavailable");
        let digest = hash(&salt, password);
        Self {
            user: user.to_string(),
            salt: B64.encode(salt),
            digest: B64.encode(digest),
        }
    }

    /// Has a password ever been set?
    ///
    /// The default credential is empty, and [`verify`](Self::verify) already denies it — but a
    /// node also has to *know* it is unconfigured, because §5's fail-closed rule says it must
    /// refuse to bind a non-loopback address rather than serve unauthenticated. That is a
    /// startup decision, not a per-request one, so it needs its own question.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !self.digest.is_empty()
    }

    /// Does this credential authenticate `(user, password)`?
    ///
    /// Both halves are compared in constant time and combined with a bitwise `&`, so neither
    /// a wrong username nor a wrong password can be told apart from the other by timing.
    #[must_use]
    pub fn verify(&self, user: &str, password: &str) -> bool {
        self.verify_choice(user, password).into()
    }

    fn verify_choice(&self, user: &str, password: &str) -> Choice {
        // A corrupt or hand-edited entry authenticates nobody. This is the one early exit,
        // and it depends on the stored config only — never on the attacker's input.
        let (Ok(salt), Ok(stored)) = (B64.decode(&self.salt), B64.decode(&self.digest)) else {
            return Choice::from(0u8);
        };
        if salt.len() != SALT_LEN || stored.len() != DIGEST_LEN {
            return Choice::from(0u8);
        }
        // Usernames are variable-length, and a length check on them is itself a leak, so
        // compare their digests instead: equal-length, and equal exactly when the names are.
        let offered = Sha256::digest(user.as_bytes());
        let known = Sha256::digest(self.user.as_bytes());
        let user_ok = offered.as_slice().ct_eq(known.as_slice());
        let digest_ok = hash(&salt, password).as_slice().ct_eq(stored.as_slice());
        user_ok & digest_ok
    }
}

/// Pull the credentials out of an `Authorization: Basic <base64>` header in a buffered request
/// head, returning the decoded `(user, password)`.
///
/// Lenient where HTTP is (the header name and the `Basic` scheme token are case-insensitive,
/// and whitespace around the value or between scheme and token is ignored), strict where
/// leniency would be a hole: the head ends at the first blank line, a header name may not
/// carry whitespace of its own, and a request bearing *more than one* `Authorization` header
/// is rejected outright rather than resolved by picking one — which of two disagreeing
/// headers a proxy honoured is exactly the ambiguity request smuggling lives in.
///
/// Returns `None` when the header is absent, duplicated, not `Basic`, not valid base64, not
/// UTF-8, or carries no `:` separating user from password.
#[must_use]
pub fn parse_basic_credentials(head: &[u8]) -> Option<(String, String)> {
    parse_basic_credentials_in(head, b"authorization")
}

/// [`parse_basic_credentials`], but reading a header of your choosing — [`MESH_AUTH_HEADER`]
/// for the mesh's own credential.
///
/// `name` is matched case-insensitively and must be given in lowercase.
#[must_use]
pub fn parse_basic_credentials_in(head: &[u8], name: &[u8]) -> Option<(String, String)> {
    let mut found: Option<&[u8]> = None;
    // `skip(1)` drops the request line; a header can never be the first line.
    for line in head.split(|&b| b == b'\n').skip(1) {
        let line = strip_cr(line);
        if line.is_empty() {
            break; // End of the head — anything after this is body, not headers.
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue; // Not a header line at all.
        };
        let (header_name, value) = line.split_at(colon);
        if header_name.iter().any(u8::is_ascii_whitespace) {
            continue; // Not a well-formed header line (obs-fold, or a padded name).
        }
        if header_name.eq_ignore_ascii_case(name) {
            if found.is_some() {
                return None; // Two of them: ambiguous, so refuse both.
            }
            found = Some(&value[1..]); // Past the ':'.
        }
    }

    let value = std::str::from_utf8(trim_ascii(found?)).ok()?;
    let (scheme, token) = value.split_once(|c: char| c.is_ascii_whitespace())?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = B64.decode(token.trim()).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    // The username may not contain a colon, so the *first* one separates the two halves; a
    // password may contain any number.
    let (user, password) = decoded.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// The one gate every request passes through — plain `GET`, `POST`, and WebSocket upgrade
/// alike.
///
/// Default-deny in both directions: an empty `credentials` list authorizes nobody (a node
/// with no password configured is not an open node), and every credential is checked without
/// an early exit, so the time this takes does not reveal which entry matched.
#[must_use]
pub fn is_authorized(head: &[u8], credentials: &[Credential]) -> bool {
    // Both headers, not the first that parses: [`MESH_AUTH_HEADER`] holds the caller machine's
    // stored password and `Authorization` whatever a person typed at the browser's prompt, and a
    // stale stored one is exactly when the typed one has to be reachable.
    [
        parse_basic_credentials_in(head, MESH_AUTH_HEADER_LOWER),
        parse_basic_credentials(head),
    ]
    .into_iter()
    .flatten()
    .fold(Choice::from(0u8), |seen, (user, password)| {
        credentials
            .iter()
            .fold(seen, |seen, c| seen | c.verify_choice(&user, &password))
    })
    .into()
}

/// The `401` challenge to send when [`is_authorized`] says no.
///
/// `Connection: close` because the credentials the client retries with belong to a fresh
/// request, and a half-read body on a kept-alive connection is how request smuggling starts.
/// `realm` is sanitised to what a quoted-string may hold — a node's petname never needs more,
/// and a `"` or a newline in it would otherwise let a caller forge headers of its own.
#[must_use]
pub fn unauthorized_response(realm: &str) -> String {
    let realm = sanitize_realm(realm);
    let body = CHALLENGE_BODY;
    format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         WWW-Authenticate: Basic realm=\"{realm}\", charset=\"UTF-8\"\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    )
}

/// `SHA-256(salt ‖ password)`.
fn hash(salt: &[u8], password: &str) -> [u8; DIGEST_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// Keep only what RFC 9110's `quoted-string` allows unescaped — printable ASCII minus `"` and
/// `\` — and never return an empty realm.
fn sanitize_realm(realm: &str) -> String {
    let kept: String = realm
        .chars()
        .filter(|&c| c.is_ascii_graphic() || c == ' ')
        .filter(|&c| c != '"' && c != '\\')
        .collect();
    let kept = kept.trim();
    if kept.is_empty() {
        FALLBACK_REALM.to_string()
    } else {
        kept.to_string()
    }
}

/// The head with every `name` header line removed, and everything else byte for byte.
///
/// Used to take [`MESH_AUTH_HEADER`] back out once it has done its job, so the service behind the
/// node never sees this machine's password. Stops at the blank line for the reason
/// [`parse_basic_credentials`] does: past it is body, where a line that looks like a header is
/// just bytes.
///
/// `name` is matched case-insensitively and must be given in lowercase.
#[must_use]
pub fn strip_header(head: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(head.len());
    let mut rest = head;
    let mut past_request_line = false;
    let mut in_headers = true;
    while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
        let (line, tail) = rest.split_at(nl + 1);
        rest = tail;
        if in_headers && past_request_line {
            let trimmed = strip_cr(&line[..line.len() - 1]);
            if trimmed.is_empty() {
                in_headers = false; // End of the head — the body is copied untouched.
            } else if is_header_named(trimmed, name) {
                continue;
            }
        }
        past_request_line = true;
        out.extend_from_slice(line);
    }
    out.extend_from_slice(rest);
    out
}

/// Whether a header line names `name`, by the same rules [`parse_basic_credentials_in`] matches
/// by — so a line it would have read is a line this removes.
fn is_header_named(line: &[u8], name: &[u8]) -> bool {
    let Some(colon) = line.iter().position(|&b| b == b':') else {
        return false;
    };
    let header_name = &line[..colon];
    !header_name.iter().any(u8::is_ascii_whitespace) && header_name.eq_ignore_ascii_case(name)
}

fn strip_cr(line: &[u8]) -> &[u8] {
    match line {
        [rest @ .., b'\r'] => rest,
        _ => line,
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request head with the given header lines, terminated like a real one.
    /// The header split, end to end: the mesh's own credential admits a request whose
    /// `Authorization` belongs to the app behind the node — the case that used to draw a `401`
    /// challenge and pop the browser's password prompt on an ordinary `fetch`.
    #[test]
    fn a_mesh_credential_admits_a_request_carrying_an_apps_own_token() {
        let creds = [Credential::from_password("igor", "hunter2")];
        let head = head(&[
            &mesh_basic("igor", "hunter2"),
            "Authorization: Bearer app.jwt.token",
        ]);
        assert!(is_authorized(&head, &creds));
    }

    /// The healing path, which reading only the first header that parses would have broken: the
    /// front door keeps attaching a stored password that has gone stale, so the one a person typed
    /// at the prompt must still be reached — or the prompt returns forever.
    #[test]
    fn a_typed_password_still_gets_in_past_a_stale_stored_one() {
        let creds = [Credential::from_password("igor", "hunter2")];
        let healed = head(&[
            &mesh_basic("igor", "the-old-one"),
            &basic("igor", "hunter2"),
        ]);
        assert!(is_authorized(&healed, &creds), "the typed one is tried too");

        let neither = head(&[
            &mesh_basic("igor", "the-old-one"),
            &basic("igor", "also-wrong"),
        ]);
        assert!(
            !is_authorized(&neither, &creds),
            "two wrong ones are still wrong"
        );
    }

    /// The mesh credential is the node's business and no one else's: it never reaches the service.
    #[test]
    fn the_mesh_header_is_stripped_and_nothing_else_is() {
        let head = head(&[
            &mesh_basic("igor", "hunter2"),
            "Authorization: Bearer app.jwt.token",
            "Accept: */*",
        ]);
        let out = strip_header(&head, MESH_AUTH_HEADER_LOWER);
        let text = String::from_utf8(out).expect("still text");
        assert!(!text.contains("X-Adi-Authorization"), "gone: {text}");
        assert!(
            text.contains("Authorization: Bearer app.jwt.token"),
            "kept: {text}"
        );
        assert!(text.contains("Accept: */*"), "kept: {text}");
        assert!(
            text.starts_with("GET /ws HTTP/1.1\r\n"),
            "request line intact: {text}"
        );
        assert!(text.ends_with("\r\n\r\n"), "head still terminated: {text}");
    }

    fn head(lines: &[&str]) -> Vec<u8> {
        let mut out = String::from("GET /ws HTTP/1.1\r\nHost: nosh.laptop-b.n.adi\r\n");
        for line in lines {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        out.into_bytes()
    }

    fn basic(user: &str, password: &str) -> String {
        format!(
            "Authorization: Basic {}",
            B64.encode(format!("{user}:{password}"))
        )
    }

    /// [`basic`], but in the mesh's own header.
    fn mesh_basic(user: &str, password: &str) -> String {
        format!(
            "{MESH_AUTH_HEADER}: Basic {}",
            B64.encode(format!("{user}:{password}"))
        )
    }

    // --- header parsing ------------------------------------------------------------------

    #[test]
    fn parses_a_basic_header() {
        let bytes = head(&[basic("igor", "hunter2").as_str()]);
        let parsed = parse_basic_credentials(&bytes);
        assert_eq!(parsed, Some(("igor".into(), "hunter2".into())));
    }

    #[test]
    fn parsing_is_case_and_whitespace_insensitive() {
        let token = B64.encode("igor:hunter2");
        for line in [
            format!("authorization: Basic {token}"),
            format!("AUTHORIZATION: basic {token}"),
            format!("Authorization:Basic {token}"),
            format!("Authorization:   BaSiC    {token}   "),
            format!("Authorization: bASIC\t{token}"),
        ] {
            assert_eq!(
                parse_basic_credentials(&head(&[line.as_str()])),
                Some(("igor".into(), "hunter2".into())),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_password_may_contain_colons() {
        let bytes = head(&[basic("igor", "a:b:c").as_str()]);
        assert_eq!(
            parse_basic_credentials(&bytes),
            Some(("igor".into(), "a:b:c".into()))
        );
    }

    #[test]
    fn parsing_rejects_absent_malformed_and_wrong_scheme() {
        let token = B64.encode("igor:hunter2");
        let no_colon = B64.encode("no-colon-here");
        let cases: Vec<Vec<u8>> = vec![
            head(&[]),                                                    // absent
            head(&["Authorization: Basic !!!not-base64!!!"]),             // bad base64
            head(&[format!("Authorization: Bearer {token}").as_str()]),   // wrong scheme
            head(&[format!("Authorization: {token}").as_str()]),          // no scheme
            head(&["Authorization: Basic"]),                              // no token
            head(&[format!("Authorization: Basic {no_colon}").as_str()]), // no user:pass split
            head(&[format!("Authorization : Basic {token}").as_str()]),   // padded header name
            head(&[format!("X-Authorization: Basic {token}").as_str()]),  // a different header
        ];
        for bytes in cases {
            let shown = String::from_utf8_lossy(&bytes).replace("\r\n", " | ");
            assert_eq!(parse_basic_credentials(&bytes), None, "{shown}");
        }
    }

    #[test]
    fn duplicate_authorization_headers_are_refused() {
        let bytes = head(&[
            basic("igor", "hunter2").as_str(),
            basic("mallory", "guess").as_str(),
        ]);
        assert_eq!(parse_basic_credentials(&bytes), None);
    }

    #[test]
    fn a_header_shaped_body_line_is_not_a_header() {
        // The blank line ends the head; what follows is the body, and must not be parsed.
        let mut bytes = head(&[]);
        bytes.extend_from_slice(basic("igor", "hunter2").as_bytes());
        bytes.extend_from_slice(b"\r\n");
        assert_eq!(parse_basic_credentials(&bytes), None);
    }

    // --- verification --------------------------------------------------------------------

    #[test]
    fn verify_accepts_the_right_password_only() {
        let cred = Credential::from_password("igor", "hunter2");
        assert!(cred.verify("igor", "hunter2"));
        assert!(!cred.verify("igor", "hunter3"), "wrong password");
        assert!(!cred.verify("igor", ""), "empty password");
        assert!(!cred.verify("igor", "hunter2 "), "trailing space matters");
        assert!(!cred.verify("mallory", "hunter2"), "wrong username");
        assert!(
            !cred.verify("Igor", "hunter2"),
            "usernames are case-sensitive"
        );
        assert!(!cred.verify("", ""), "neither half");
    }

    #[test]
    fn each_credential_gets_its_own_salt() {
        let a = Credential::from_password("igor", "hunter2");
        let b = Credential::from_password("igor", "hunter2");
        assert_ne!(a.salt, b.salt, "salts are random per credential");
        assert_ne!(
            a.digest, b.digest,
            "so the same password stores differently"
        );
        assert!(a.verify("igor", "hunter2") && b.verify("igor", "hunter2"));
        assert_eq!(B64.decode(&a.salt).expect("base64").len(), SALT_LEN);
        assert_eq!(B64.decode(&a.digest).expect("base64").len(), DIGEST_LEN);
        assert!(!a.digest.contains("hunter2"), "no plaintext at rest");
    }

    #[test]
    fn a_corrupt_credential_authenticates_nobody() {
        let mut cred = Credential::from_password("igor", "hunter2");
        cred.salt = "not base64 at all!!".to_string();
        assert!(!cred.verify("igor", "hunter2"));

        let mut cred = Credential::from_password("igor", "hunter2");
        cred.digest = B64.encode([0u8; 8]); // right encoding, wrong length
        assert!(!cred.verify("igor", "hunter2"));
    }

    #[test]
    fn a_credential_round_trips_through_toml() {
        let cred = Credential::from_password("igor", "hunter2");
        let text = toml::to_string(&cred).expect("serialize");
        let back: Credential = toml::from_str(&text).expect("deserialize");
        assert!(back.verify("igor", "hunter2"));
        assert_eq!(
            (back.user, back.salt, back.digest),
            (cred.user, cred.salt, cred.digest)
        );
    }

    // --- the gate ------------------------------------------------------------------------

    #[test]
    fn the_gate_admits_a_configured_user() {
        let creds = vec![
            Credential::from_password("igor", "hunter2"),
            Credential::from_password("ada", "difference"),
        ];
        for (user, password, expected) in [
            ("igor", "hunter2", true),
            ("ada", "difference", true),
            ("ada", "hunter2", false),
            ("igor", "difference", false),
            ("mallory", "hunter2", false),
        ] {
            let bytes = head(&[basic(user, password).as_str()]);
            assert_eq!(is_authorized(&bytes, &creds), expected, "{user}/{password}");
        }
        assert!(!is_authorized(&head(&[]), &creds), "no header at all");
    }

    #[test]
    fn no_credentials_configured_means_default_deny() {
        let bytes = head(&[basic("igor", "hunter2").as_str()]);
        assert!(!is_authorized(&bytes, &[]));
        assert!(!is_authorized(&head(&[]), &[]));
    }

    #[test]
    fn a_websocket_upgrade_is_gated_like_any_other_request() {
        let creds = vec![Credential::from_password("igor", "hunter2")];
        let upgrade = [
            "Upgrade: websocket",
            "Connection: Upgrade",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
            "Sec-WebSocket-Version: 13",
        ];

        // No credentials: the handshake is refused exactly like a plain GET would be, and the
        // node answers with the challenge rather than letting the upgrade through.
        let bare = head(&upgrade);
        assert!(!is_authorized(&bare, &creds), "an upgrade is not exempt");
        let challenge = unauthorized_response("laptop-b");
        assert!(challenge.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(!challenge.contains("101"), "nothing is switching protocols");

        // With credentials it passes, so the gate is closing on the credentials and not on
        // the upgrade headers.
        let auth = basic("igor", "hunter2");
        let mut with_creds = upgrade.to_vec();
        with_creds.push(auth.as_str());
        assert!(is_authorized(&head(&with_creds), &creds));
    }

    // --- the challenge -------------------------------------------------------------------

    #[test]
    fn the_challenge_is_a_well_formed_401() {
        let response = unauthorized_response("laptop-b");
        let (head_part, body) = response.split_once("\r\n\r\n").expect("blank line");

        assert!(head_part.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(
            head_part.contains("\r\nWWW-Authenticate: Basic realm=\"laptop-b\", charset=\"UTF-8\""),
            "{head_part}"
        );
        assert!(head_part.contains("\r\nConnection: close"));

        let declared: usize = head_part
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .expect("a Content-Length")
            .parse()
            .expect("a number");
        assert_eq!(declared, body.len(), "Content-Length matches the body");
        assert!(!body.is_empty());
    }

    #[test]
    fn a_hostile_realm_cannot_forge_headers() {
        let response = unauthorized_response("lap\"top\r\nX-Injected: yes\r\n\r\nowned");
        let (head_part, body) = response.split_once("\r\n\r\n").expect("blank line");

        // The quote and the CRLFs are gone, so the payload stays *inside* one quoted-string
        // instead of becoming a header of its own — and the body is still ours.
        assert!(
            head_part.contains("realm=\"laptopX-Injected: yesowned\""),
            "{head_part}"
        );
        assert_eq!(
            head_part.matches('"').count(),
            4,
            "two quoted values, nothing loose"
        );
        assert_eq!(
            head_part.lines().count(),
            5,
            "no extra header line: {head_part}"
        );
        assert!(
            !head_part.lines().any(|l| l.starts_with("X-Injected")),
            "{head_part}"
        );
        assert_eq!(body, CHALLENGE_BODY);

        // An empty or wholly unusable realm still yields a valid header.
        for realm in ["", "\"", "\\", "\r\n"] {
            let response = unauthorized_response(realm);
            assert!(
                response.contains(&format!("realm=\"{FALLBACK_REALM}\"")),
                "{realm:?} falls back"
            );
        }
    }
}
