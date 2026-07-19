//! Config-field port commands: a `bash`…`` preprocessor plus on-read execution.
//!
//! A config field (e.g. a hive.yaml `rollout.recreate.ports` value) may be written as a literal
//! integer or as a shell-style backtick command, unquoted in the YAML:
//!
//! ```yaml
//! a:
//!   b: bash`ports-manager.get('demo/app')`
//! ```
//!
//! The document is plain YAML *plus* these `bash`…`` commands, so it is run through
//! [`preprocess`] first: every command is replaced with a quoted `"datacommand:<hash>"`
//! placeholder — leaving 100% valid YAML — and the `hash → command source` pairs are collected
//! into a [`Commands`] table:
//!
//! ```yaml
//! a:
//!   b: "datacommand:9f3a…"
//! ```
//!
//! Parse the preprocessed text inside [`with_commands`] (which installs that table for the
//! current thread). When serde reads a port field carrying a `datacommand:<hash>` value, the
//! deserializer looks the hash back up, parses the command with a small hand-rolled parser, and
//! runs it — for `ports-manager.get('name')`, reserving a stable port from the live registry.
//! Because [`Ports`] holds no state beyond the on-disk registry, execution needs no external
//! context. The reservation is idempotent, so re-reading the same config returns the same port.
//!
//! Opt a field in with serde's `deserialize_with`:
//! ```ignore
//! #[serde(default, deserialize_with = "adi_ports_manager::ports_map")]
//! ports: std::collections::BTreeMap<String, u16>,   // ints or datacommand:<hash> placeholders
//!
//! #[serde(deserialize_with = "adi_ports_manager::port")]
//! http: u16,                                         // a single int-or-command
//! ```

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

use crate::Ports;

/// The tag that opens a backtick command in the source YAML, e.g. `` bash`ports-manager.get('x')` ``.
const COMMAND_TAG: &str = "bash`";

/// The scheme a preprocessed command carries as its (quoted) YAML value: `datacommand:<hash>`.
const SCHEME: &str = "datacommand:";

/// The registry key a single-argument `get('name')` reserves under; `get('name', 'key')`
/// overrides it.
const DEFAULT_KEY: &str = "port";

thread_local! {
    /// A per-thread [`Ports`] override for command execution. `None` uses [`Ports::new`].
    /// Set it around a parse to redirect reservations (e.g. tests pointing at a temp registry).
    static OVERRIDE: RefCell<Option<Ports>> = const { RefCell::new(None) };

    /// The `hash → command source` table for the config currently being parsed, installed by
    /// [`with_commands`] so the context-free deserializer can resolve `datacommand:<hash>`.
    static COMMANDS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// The `hash → command source` map produced by [`preprocess`], carried opaquely to
/// [`with_commands`]. Callers never inspect it, but [`Commands::restore`] can rewrite the
/// placeholders in re-serialized YAML back into their `` bash`…` `` source, so a config can
/// round-trip through preprocess → parse → patch → serialize without losing its commands.
#[derive(Debug, Clone, Default)]
pub struct Commands(HashMap<String, String>);

impl Commands {
    /// Register `command` and return its `datacommand:<hash>` placeholder value — for building
    /// *new* config values that [`restore`](Self::restore) will rewrite into `` bash`…` ``
    /// alongside the ones [`preprocess`] collected.
    pub fn placeholder(&mut self, command: &str) -> String {
        let hash = hash_command(command);
        self.0.insert(hash.clone(), command.to_string());
        format!("{SCHEME}{hash}")
    }

    /// The reverse of [`preprocess`]: rewrite every known `datacommand:<hash>` placeholder in
    /// `text` (quoted or plain, as a serializer may emit either) back to its `` bash`…` ``
    /// command source. Unknown placeholders are left verbatim.
    #[must_use]
    pub fn restore(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (hash, command) in &self.0 {
            let bash = format!("{COMMAND_TAG}{command}`");
            for needle in [
                format!("\"{SCHEME}{hash}\""),
                format!("'{SCHEME}{hash}'"),
                format!("{SCHEME}{hash}"),
            ] {
                out = out.replace(&needle, &bash);
            }
        }
        out
    }
}

/// Rewrite raw config text into plain YAML: every `` bash`…` `` command becomes a quoted
/// `"datacommand:<hash>"` placeholder, and the returned [`Commands`] maps each hash back to its
/// command source so a port field can resolve and run it on read.
///
/// An unterminated `` bash` `` (no closing backtick) is left verbatim.
#[must_use]
pub fn preprocess(raw: &str) -> (String, Commands) {
    let mut table = HashMap::new();
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(pos) = rest.find(COMMAND_TAG) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + COMMAND_TAG.len()..];
        let Some(end) = after.find('`') else {
            // Unterminated command — leave the remainder untouched.
            out.push_str(&rest[pos..]);
            return (out, Commands(table));
        };
        let command = &after[..end];
        let hash = hash_command(command);
        table.insert(hash.clone(), command.to_string());
        // The placeholder is quoted, so any command text stays valid inside the YAML scalar.
        out.push('"');
        out.push_str(SCHEME);
        out.push_str(&hash);
        out.push('"');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    (out, Commands(table))
}

/// A stable, YAML-safe hex digest of a command's source text.
fn hash_command(command: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    command.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Install `commands` (from [`preprocess`]) for the current thread while `f` parses the
/// preprocessed YAML, then restore the previous table. Port fields read during `f` resolve their
/// `datacommand:<hash>` values against it.
pub fn with_commands<R>(commands: Commands, f: impl FnOnce() -> R) -> R {
    let previous = COMMANDS.with(|slot| slot.replace(commands.0));
    let result = f();
    COMMANDS.with(|slot| *slot.borrow_mut() = previous);
    result
}

/// Execute commands on this thread against `ports` for the duration of `f`, then restore the
/// previous manager. Primarily a test/embedding seam so parsing does not touch the real registry.
pub fn with_ports<R>(ports: Ports, f: impl FnOnce() -> R) -> R {
    let previous = OVERRIDE.with(|slot| slot.borrow_mut().replace(ports));
    let result = f();
    OVERRIDE.with(|slot| *slot.borrow_mut() = previous);
    result
}

/// The manager command execution should use on this thread.
fn current_ports() -> Ports {
    OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_default()
}

/// A port field: a literal integer or a preprocessed `datacommand:<hash>` placeholder (originally
/// a `` bash`…` `` command), resolved to a concrete port on deserialize.
#[derive(Debug, Clone, Copy)]
pub struct Port(pub u16);

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(PortVisitor).map(Port)
    }
}

/// `deserialize_with` helper for a single `u16` port field that may be a literal or a command.
///
/// # Errors
/// Fails if the value is neither an integer nor a resolvable `datacommand:<hash>` placeholder, or
/// if the command fails to run.
pub fn port<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    Port::deserialize(deserializer).map(|p| p.0)
}

/// `deserialize_with` helper for a `BTreeMap<String, u16>` whose values may each be a literal or a
/// command placeholder (see [`port`]).
///
/// # Errors
/// Fails if any value fails to deserialize per [`port`].
pub fn ports_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, u16>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = BTreeMap::<String, Port>::deserialize(deserializer)?;
    Ok(raw.into_iter().map(|(key, value)| (key, value.0)).collect())
}

struct PortVisitor;

impl Visitor<'_> for PortVisitor {
    type Value = u16;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a port number or a preprocessed datacommand:<hash> command")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u16, E> {
        u16::try_from(v).map_err(|_| E::custom(format!("port {v} is out of range")))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u16, E> {
        u16::try_from(v).map_err(|_| E::custom(format!("port {v} is out of range")))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<u16, E> {
        resolve(v).map_err(E::custom)
    }
}

/// Resolve a port field's scalar form: a bare integer, or a `datacommand:<hash>` placeholder whose
/// command is looked up in the current [`with_commands`] table and executed.
fn resolve(raw: &str) -> Result<u16, String> {
    let trimmed = raw.trim();
    if let Ok(port) = trimmed.parse::<u16>() {
        return Ok(port);
    }
    let hash = trimmed.strip_prefix(SCHEME).ok_or_else(|| {
        format!("expected a port number or a preprocessed {SCHEME}<hash> value, got `{raw}`")
    })?;
    let command = COMMANDS
        .with(|slot| slot.borrow().get(hash).cloned())
        .ok_or_else(|| {
            format!(
                "no command registered for hash `{hash}` — was the config run through preprocess()?"
            )
        })?;
    Call::parse(&command)?.eval()
}

/// A parsed `namespace.function('arg', …)` call.
struct Call {
    namespace: String,
    function: String,
    args: Vec<String>,
}

impl Call {
    fn parse(expr: &str) -> Result<Self, String> {
        let expr = expr.trim();
        let open = expr
            .find('(')
            .ok_or_else(|| format!("expected `(` in command `{expr}`"))?;
        let close = expr
            .rfind(')')
            .ok_or_else(|| format!("expected `)` in command `{expr}`"))?;
        if close < open {
            return Err(format!("misplaced parentheses in command `{expr}`"));
        }
        let (namespace, function) = expr[..open]
            .trim()
            .rsplit_once('.')
            .ok_or_else(|| format!("expected `namespace.function(…)` in command `{expr}`"))?;
        let args = parse_args(&expr[open + 1..close])?;
        Ok(Self {
            namespace: namespace.trim().to_string(),
            function: function.trim().to_string(),
            args,
        })
    }

    fn eval(&self) -> Result<u16, String> {
        match (self.namespace.as_str(), self.function.as_str()) {
            // `ports-manager.get('name'[, 'key'])` — reserve a stable port for `name`.
            ("ports-manager" | "ports_manager", "get") => {
                let name = self
                    .args
                    .first()
                    .ok_or("ports-manager.get(name) needs a name argument")?;
                let key = self.args.get(1).map_or(DEFAULT_KEY, String::as_str);
                current_ports()
                    .reserve(name, key)
                    .map_err(|e| format!("reserving a port for `{name}`: {e}"))
            }
            (namespace, function) => {
                Err(format!("unknown command function `{namespace}.{function}`"))
            }
        }
    }
}

/// Split a call's argument list into quoted string literals; empty for no args.
fn parse_args(raw: &str) -> Result<Vec<String>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|arg| parse_literal(arg.trim()))
        .collect()
}

/// Parse a single `'quoted'` or `"quoted"` string literal.
fn parse_literal(token: &str) -> Result<String, String> {
    let bytes = token.as_bytes();
    let quoted = token.len() >= 2
        && (bytes[0] == b'\'' || bytes[0] == b'"')
        && bytes[bytes.len() - 1] == bytes[0];
    if quoted {
        Ok(token[1..token.len() - 1].to_string())
    } else {
        Err(format!("expected a quoted string argument, got `{token}`"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::Config;

    /// A manager backed by a unique throwaway registry, so tests never share state.
    fn temp_manager() -> (Ports, PathBuf) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "adi-ports-template-{}-{n}/registry.json",
            std::process::id()
        ));
        (
            Ports::with_config(Config {
                registry_path: path.clone(),
                ..Config::default()
            }),
            path,
        )
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    /// Resolve a bare scalar as a port field would, after preprocessing raw text that may hold a
    /// `` bash`…` `` command.
    fn run(raw: &str) -> Result<u16, String> {
        let (rewritten, commands) = preprocess(raw);
        // preprocess() quotes the placeholder; strip the quotes to feed resolve() a bare scalar.
        let scalar = rewritten.trim().trim_matches('"').to_string();
        with_commands(commands, || resolve(&scalar))
    }

    #[test]
    fn preprocess_rewrites_commands_to_quoted_placeholders() {
        let (yaml, commands) = preprocess("a:\n  b: bash`command 123123`\n");
        assert!(
            yaml.contains(r#"b: "datacommand:"#),
            "command becomes a quoted datacommand placeholder, got: {yaml}"
        );
        assert!(!yaml.contains("bash`"), "no raw command tag survives");
        assert_eq!(commands.0.len(), 1);
        assert!(commands.0.values().any(|c| c == "command 123123"));
    }

    #[test]
    fn preprocess_leaves_plain_yaml_untouched() {
        let (yaml, commands) = preprocess("a:\n  b: 8090\n");
        assert_eq!(yaml, "a:\n  b: 8090\n");
        assert!(commands.0.is_empty());
    }

    #[test]
    fn restore_reverses_preprocess() {
        let raw = "a:\n  b: bash`ports-manager.get('x', 'http')`\n  c: 8090\n";
        let (yaml, commands) = preprocess(raw);
        assert_eq!(commands.restore(&yaml), raw);
    }

    #[test]
    fn restore_handles_quoted_and_plain_placeholders() {
        let (_, mut commands) = preprocess("");
        let placeholder = commands.placeholder("ports-manager.get('svc')");
        let quoted = format!("a: \"{placeholder}\"\n");
        let single = format!("a: '{placeholder}'\n");
        let plain = format!("a: {placeholder}\n");
        let want = "a: bash`ports-manager.get('svc')`\n";
        assert_eq!(commands.restore(&quoted), want);
        assert_eq!(commands.restore(&single), want);
        assert_eq!(commands.restore(&plain), want);
    }

    #[test]
    fn placeholder_registers_the_command_for_parsing() {
        let (manager, path) = temp_manager();
        let (_, mut commands) = preprocess("");
        let placeholder = commands.placeholder("ports-manager.get('fresh/app', 'http')");
        let port = with_ports(manager.clone(), || {
            with_commands(commands, || resolve(&placeholder)).expect("resolves")
        });
        assert_eq!(
            manager.get("fresh/app", "http").expect("lookup"),
            Some(port)
        );
        cleanup(&path);
    }

    #[test]
    fn literal_integer_passes_through() {
        assert_eq!(run("48090").expect("literal"), 48090);
    }

    #[test]
    fn command_reserves_a_port_and_is_stable() {
        let (manager, path) = temp_manager();
        let (first, second) = with_ports(manager.clone(), || {
            let first = run("bash`ports-manager.get('demo/app')`").expect("first");
            let second = run("bash`ports-manager.get('demo/app')`").expect("second");
            (first, second)
        });
        assert_eq!(first, second);
        assert_eq!(
            manager.get("demo/app", "port").expect("lookup"),
            Some(first),
            "the reservation is persisted under the (name, default-key) pair"
        );
        cleanup(&path);
    }

    #[test]
    fn distinct_names_get_distinct_ports() {
        let (manager, path) = temp_manager();
        with_ports(manager, || {
            let a = run("bash`ports-manager.get('one')`").expect("one");
            let b = run("bash`ports-manager.get('two')`").expect("two");
            assert_ne!(a, b);
        });
        cleanup(&path);
    }

    #[test]
    fn explicit_key_argument_is_honored() {
        let (manager, path) = temp_manager();
        let port = with_ports(manager.clone(), || {
            run("bash`ports-manager.get('svc', 'grpc')`").expect("keyed")
        });
        assert_eq!(manager.get("svc", "grpc").expect("lookup"), Some(port));
        cleanup(&path);
    }

    #[test]
    fn ports_map_helper_resolves_preprocessed_placeholders() {
        let (manager, path) = temp_manager();
        // A preprocessed document: the http value is a datacommand placeholder whose hash maps to
        // a ports-manager command; db is a plain literal. (JSON stands in for parsed YAML — it
        // can't carry an unquoted command, so we feed ports_map what preprocess() would produce.)
        let command = "ports-manager.get('demo/app')";
        let hash = hash_command(command);
        let mut commands = Commands::default();
        commands.0.insert(hash.clone(), command.to_string());
        let json = format!(r#"{{"http": "datacommand:{hash}", "db": 5432}}"#);
        let map = with_ports(manager, || {
            with_commands(commands, || {
                let mut de = serde_json::Deserializer::from_str(&json);
                ports_map(&mut de).expect("map")
            })
        });
        assert_eq!(map.get("db"), Some(&5432));
        assert!(map.get("http").is_some_and(|&p| p != 5432));
        cleanup(&path);
    }

    #[test]
    fn rejects_unknown_function_and_bad_syntax() {
        assert!(run("bash`ports-manager.take('x')`").is_err());
        assert!(run("bash`mystery.get('x')`").is_err());
        assert!(run("bash`ports-manager.get(unquoted)`").is_err());
        assert!(resolve("datacommand:deadbeef").is_err());
        assert!(resolve("nonsense").is_err());
    }
}
