//! How an MCP server is reached, and the one grammar every field it carries a secret in shares.
//!
//! A definition never stores a secret's value, only the secret's own name — the same allowlist
//! rule an agent's attached secrets already follow (`adi_agents::SecretAttachment`). [`Value`] is
//! the one place that distinction lives: a plain string is taken literally, and `{ secret = "…" }`
//! names a secret to resolve at launch, against whichever agent is being launched.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One header or environment value: either written out in full, or a secret's name to resolve at
/// launch time. `#[serde(untagged)]` tells the two apart by shape alone — a bare TOML/JSON string
/// is [`Value::Literal`], a one-field table naming `secret` is [`Value::Secret`] — so a definition
/// reads exactly as a publisher would write it by hand, no tag to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Literal(String),
    Secret {
        /// The secret's key name — matched against whichever agent's launch is resolving this
        /// server, the same way `adi_agents::SecretAttachment::name` is.
        secret: String,
    },
}

impl Value {
    /// A plain literal value, for building a definition in code (tests, the marketplace lander).
    #[must_use]
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    /// A reference to a secret by name.
    #[must_use]
    pub fn secret(name: impl Into<String>) -> Self {
        Self::Secret { secret: name.into() }
    }

    /// The secret name this value names, or `None` for a literal.
    #[must_use]
    pub fn secret_name(&self) -> Option<&str> {
        match self {
            Self::Secret { secret } => Some(secret),
            Self::Literal(_) => None,
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
        /// Extra request headers, each either written out or naming a secret — `Authorization:
        /// Bearer <token>` is the usual reason one is needed at all.
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
    fn a_secret_reference_round_trips_as_a_table() {
        let value = Value::secret("N8N_TOKEN");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "{\"secret\":\"N8N_TOKEN\"}");
        let back: Value = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value);
    }

    #[test]
    fn required_secrets_collects_only_the_secret_valued_entries() {
        let transport = Transport::Http {
            url: "http://n8n.adi/mcp/".into(),
            headers: BTreeMap::from([
                ("Content-Type".to_string(), Value::literal("application/json")),
                ("Authorization".to_string(), Value::secret("N8N_TOKEN")),
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
            headers: BTreeMap::from([("Authorization".to_string(), Value::secret("N8N_TOKEN"))]),
        };
        let text = toml::to_string(&transport).expect("serialize");
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
}
