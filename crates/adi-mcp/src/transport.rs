//! How an MCP server is reached, and the one grammar every field it carries a secret in shares.
//!
//! A definition never stores a secret's value, only the secret's own name — the same allowlist
//! rule an agent's attached secrets already follow (`adi_agents::SecretAttachment`). [`Value`] is
//! the one place that distinction lives: a plain string is taken literally, and `{ secret = "…" }`
//! names a secret to resolve at launch, against whichever agent is being launched.
//!
//! A secret reference may also carry a `prefix`, glued onto the resolved value with nothing in
//! between: `{ secret = "N8N_MCP_ACCESS_TOKEN", prefix = "Bearer " }` sends `Authorization:
//! Bearer <token>` while the stored secret is the raw token n8n handed out — exactly what
//! `bearer_token_env_var` on the Codex side also expects (see
//! `adi_agents::backends::process::codex_mcp`). Without a `prefix` a secret resolves exactly as
//! it always has: verbatim, so an existing definition that already baked the word `Bearer` into
//! its secret's value keeps working unchanged.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One header or environment value: either written out in full, or a secret's name to resolve at
/// launch time, optionally with a fixed prefix glued onto the front of it. `#[serde(untagged)]`
/// tells the two apart by shape alone — a bare TOML/JSON string is [`Value::Literal`], a table
/// naming `secret` is [`Value::Secret`] — so a definition reads exactly as a publisher would
/// write it by hand, no tag to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Literal(String),
    Secret {
        /// The secret's key name — matched against whichever agent's launch is resolving this
        /// server, the same way `adi_agents::SecretAttachment::name` is.
        secret: String,
        /// Glued onto the front of the secret's resolved value with nothing in between —
        /// `"Bearer "` being the usual reason one is needed at all. Absent (the default) resolves
        /// the secret verbatim, exactly as before this field existed.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        prefix: String,
    },
}

/// The prefix `Value::bearer` glues onto a secret's resolved value — shared with
/// `adi_agents::backends::process::codex_mcp`, which has to recognize this exact scheme to hand
/// Codex the raw token underneath it.
pub const BEARER_PREFIX: &str = "Bearer ";

impl Value {
    /// A plain literal value, for building a definition in code (tests, the marketplace lander).
    #[must_use]
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    /// A reference to a secret by name, resolved verbatim.
    #[must_use]
    pub fn secret(name: impl Into<String>) -> Self {
        Self::Secret { secret: name.into(), prefix: String::new() }
    }

    /// A reference to a secret by name, with `prefix` glued onto its resolved value.
    #[must_use]
    pub fn secret_with_prefix(name: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self::Secret { secret: name.into(), prefix: prefix.into() }
    }

    /// A reference to a secret by name, rendered as `Bearer <token>` — the shape almost every
    /// bearer-auth API (n8n's own MCP server included) wants in its `Authorization` header, from
    /// a secret that stores only the raw token.
    #[must_use]
    pub fn bearer(name: impl Into<String>) -> Self {
        Self::secret_with_prefix(name, BEARER_PREFIX)
    }

    /// The secret name this value names, or `None` for a literal.
    #[must_use]
    pub fn secret_name(&self) -> Option<&str> {
        match self {
            Self::Secret { secret, .. } => Some(secret),
            Self::Literal(_) => None,
        }
    }

    /// The prefix a secret reference glues onto its resolved value — empty for a literal, and
    /// empty by default for a secret reference that names none.
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::Secret { prefix, .. } => prefix,
            Self::Literal(_) => "",
        }
    }
}

/// How an MCP server is reached. Stored internally tagged (`kind = "http" | "stdio"`) so a
/// manifest reads as one table rather than a choice buried in a nested one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Transport {
    /// Streamable HTTP — the shape most hosted MCP servers (n8n's own included) speak.
    Http {
        url: String,
        /// Extra request headers, each either written out, naming a secret, or naming a secret
        /// with a prefix — `Authorization: Bearer <token>` is the usual reason one is needed at
        /// all (see [`Value::bearer`]).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, Value>,
    },
    /// A server launched as a child process speaking MCP over its own stdio.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, Value>,
    },
}

impl Transport {
    /// Every secret name this transport declares it needs — a header or an env var naming one.
    /// What a bundle installer and `mcp show` both read to tell an operator what to set.
    #[must_use]
    pub fn required_secrets(&self) -> BTreeSet<String> {
        let values: Box<dyn Iterator<Item = &Value>> = match self {
            Self::Http { headers, .. } => Box::new(headers.values()),
            Self::Stdio { env, .. } => Box::new(env.values()),
        };
        values.filter_map(Value::secret_name).map(str::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_round_trips_as_a_bare_string() {
        let value = Value::literal("application/json");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "\"application/json\"");
        let back: Value = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value);
    }

    #[test]
    fn a_secret_reference_round_trips_as_a_table_with_no_prefix_field() {
        let value = Value::secret("N8N_TOKEN");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "{\"secret\":\"N8N_TOKEN\"}", "an empty prefix is never written out");
        let back: Value = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value);
        assert_eq!(value.prefix(), "");
    }

    #[test]
    fn a_bearer_secret_round_trips_carrying_its_prefix() {
        let value = Value::bearer("N8N_MCP_ACCESS_TOKEN");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "{\"secret\":\"N8N_MCP_ACCESS_TOKEN\",\"prefix\":\"Bearer \"}");
        let back: Value = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value);
        assert_eq!(value.prefix(), BEARER_PREFIX);
        assert_eq!(value.secret_name(), Some("N8N_MCP_ACCESS_TOKEN"));
    }

    #[test]
    fn an_old_definition_with_no_prefix_field_at_all_still_deserializes() {
        let old = r#"{"secret":"N8N_TOKEN"}"#;
        let value: Value = serde_json::from_str(old).expect("deserialize");
        assert_eq!(value, Value::secret("N8N_TOKEN"));
        assert_eq!(value.prefix(), "");
    }

    #[test]
    fn required_secrets_collects_only_the_secret_valued_entries() {
        let transport = Transport::Http {
            url: "http://n8n.adi/mcp/".into(),
            headers: BTreeMap::from([
                ("Content-Type".to_string(), Value::literal("application/json")),
                ("Authorization".to_string(), Value::bearer("N8N_TOKEN")),
            ]),
        };
        assert_eq!(
            transport.required_secrets(),
            BTreeSet::from(["N8N_TOKEN".to_string()])
        );

        let transport = Transport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "n8n-mcp".into()],
            env: BTreeMap::new(),
        };
        assert!(transport.required_secrets().is_empty());
    }

    #[test]
    fn a_transport_round_trips_through_toml() {
        let transport = Transport::Http {
            url: "http://n8n.adi/mcp/".into(),
            headers: BTreeMap::from([("Authorization".to_string(), Value::bearer("N8N_TOKEN"))]),
        };
        let text = toml::to_string(&transport).expect("serialize");
        assert!(text.contains("prefix = \"Bearer \""), "{text}");
        let back: Transport = toml::from_str(&text).expect("deserialize");
        assert_eq!(back, transport);

        let transport = Transport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "n8n-mcp".into()],
            env: BTreeMap::from([("N8N_API_KEY".to_string(), Value::secret("N8N_TOKEN"))]),
        };
        let text = toml::to_string(&transport).expect("serialize");
        let back: Transport = toml::from_str(&text).expect("deserialize");
        assert_eq!(back, transport);
    }

    #[test]
    fn an_old_toml_manifest_with_no_prefix_field_still_parses() {
        let text = "kind = \"http\"\nurl = \"http://n8n.adi/mcp/\"\n[headers]\nAuthorization = { secret = \"N8N_TOKEN\" }\n";
        let transport: Transport = toml::from_str(text).expect("deserialize");
        let Transport::Http { headers, .. } = &transport else { panic!("http") };
        assert_eq!(headers.get("Authorization"), Some(&Value::secret("N8N_TOKEN")));
    }
}
