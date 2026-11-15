//! Resolving an agent's declared MCP servers ([`AgentManifest::mcp`]) into what a backend can
//! actually hand its engine — every secret reference already substituted for a value.
//!
//! Resolution happens once, in [`crate::Agents::spec_in`], against *that agent's own* attached
//! secrets ([`SecretAttachment`]) — the identical allowlist [`crate::attached_secret_env`] already
//! applies to the run's plain environment, so an MCP server reaches no secret an agent's shell
//! could not already read for itself.
//!
//! A server naming a secret the agent never attached, or this machine has no value for, is
//! dropped **whole** rather than launched half-configured: the same best-effort tone
//! `attached_secret_env` already carries ("a missing/undecryptable secret is skipped"), but here a
//! half-authenticated MCP server is worse than none — a request that reaches the far end missing
//! its bearer token is not a failure the model can read and correct itself from, the way a failed
//! tool call is; it is a confusing 401 from a tool the model was told it could trust.

use std::collections::BTreeMap;

use adi_config::Config;
use adi_mcp::{McpServers, Transport, Value};

use crate::agent::SecretAttachment;

/// One declared server, resolved: every header/env value is a plain string now, because the
/// value never has to outlive this run's own config — unlike the definition it came from, which
/// never stores one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcpServer {
    /// The server's own id — also the `mcp__<id>` tool-grant prefix a Claude engine sees, and the
    /// key this id's config is written under in `mcpServers`.
    pub id: String,
    pub transport: ResolvedTransport,
}

/// [`adi_mcp::Transport`], with every [`Value`] resolved to the string it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTransport {
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
}

/// Resolve `ids` into runnable servers, in the order named, silently skipping any id this store
/// has no definition for (the same tolerance `bin_tools` and `secrets` already give a dangling
/// reference) and any whose secrets do not all resolve against `secrets` (see the module docs).
#[must_use]
pub fn resolve(config: &Config, ids: &[String], secrets: &[SecretAttachment]) -> Vec<ResolvedMcpServer> {
    if ids.is_empty() {
        return Vec::new();
    }
    let store = McpServers::with_config(config.clone());
    let secrets_store = adi_secrets::Secrets::with_config(config.clone());
    let lookup = |name: &str| -> Option<String> {
        let attachment = secrets.iter().find(|a| a.name == name)?;
        secrets_store.reveal(attachment.project.as_deref(), name).ok().flatten()
    };
    ids.iter()
        .filter_map(|id| {
            let server = store.get(id).ok().flatten()?;
            let transport = resolve_transport(&server.manifest.transport, &lookup)?;
            Some(ResolvedMcpServer { id: server.id, transport })
        })
        .collect()
}

fn resolve_transport(transport: &Transport, lookup: &impl Fn(&str) -> Option<String>) -> Option<ResolvedTransport> {
    Some(match transport {
        Transport::Http { url, headers } => ResolvedTransport::Http {
            url: url.clone(),
            headers: resolve_map(headers, lookup)?,
        },
        Transport::Stdio { command, args, env } => ResolvedTransport::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: resolve_map(env, lookup)?,
        },
    })
}

/// Resolve every value in one header/env map, or `None` the moment one names a secret `lookup`
/// cannot answer — the whole server is dropped by its caller, never half-resolved.
fn resolve_map(
    map: &BTreeMap<String, Value>,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (key, value) in map {
        let resolved = match value {
            Value::Literal(text) => text.clone(),
            Value::Secret { secret } => lookup(secret)?,
        };
        out.insert(key.clone(), resolved);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "adi-agents-mcp-servers-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Config::with_root(dir)
    }

    fn attachment(name: &str) -> SecretAttachment {
        SecretAttachment { project: None, name: name.to_string() }
    }

    #[test]
    fn a_server_with_no_secrets_resolves_with_no_attachments_needed() {
        let config = scratch("no-secrets");
        McpServers::with_config(config.clone())
            .save(
                "n8n",
                adi_mcp::Manifest::new(
                    "n8n",
                    Transport::Stdio { command: "npx".into(), args: vec!["-y".into(), "n8n-mcp".into()], env: BTreeMap::new() },
                ),
            )
            .expect("save");

        let resolved = resolve(&config, &["n8n".to_string()], &[]);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "n8n");
    }

    #[test]
    fn a_secret_header_resolves_only_against_the_agents_own_attachment() {
        let config = scratch("attached-secret");
        adi_secrets::Secrets::with_config(config.clone())
            .set(None, "N8N_TOKEN", "sekret", None)
            .expect("set secret");
        McpServers::with_config(config.clone())
            .save(
                "n8n",
                adi_mcp::Manifest::new(
                    "n8n",
                    Transport::Http {
                        url: "http://n8n.adi/mcp/".into(),
                        headers: BTreeMap::from([("Authorization".to_string(), Value::secret("N8N_TOKEN"))]),
                    },
                ),
            )
            .expect("save");

        // Not attached: dropped whole, even though the secret exists on this machine.
        let resolved = resolve(&config, &["n8n".to_string()], &[]);
        assert!(resolved.is_empty());

        // Attached: resolves, and the header carries the real value, never the name.
        let resolved = resolve(&config, &["n8n".to_string()], &[attachment("N8N_TOKEN")]);
        assert_eq!(resolved.len(), 1);
        let ResolvedTransport::Http { headers, .. } = &resolved[0].transport else {
            panic!("http");
        };
        assert_eq!(headers.get("Authorization"), Some(&"sekret".to_string()));
    }

    #[test]
    fn an_unresolvable_secret_drops_the_whole_server_not_just_the_header() {
        let config = scratch("unresolvable");
        McpServers::with_config(config.clone())
            .save(
                "n8n",
                adi_mcp::Manifest::new(
                    "n8n",
                    Transport::Http {
                        url: "http://n8n.adi/mcp/".into(),
                        headers: BTreeMap::from([("Authorization".to_string(), Value::secret("N8N_TOKEN"))]),
                    },
                ),
            )
            .expect("save");

        // Attached, but never set anywhere — still drops whole, never a header with no value.
        let resolved = resolve(&config, &["n8n".to_string()], &[attachment("N8N_TOKEN")]);
        assert!(resolved.is_empty());
    }

    #[test]
    fn a_dangling_id_is_skipped_like_a_dangling_bin_tool() {
        let config = scratch("dangling");
        let resolved = resolve(&config, &["no-such-server".to_string()], &[]);
        assert!(resolved.is_empty());
    }

    #[test]
    fn ids_resolve_in_the_order_named() {
        let config = scratch("order");
        let store = McpServers::with_config(config.clone());
        for id in ["b", "a"] {
            store
                .save(id, adi_mcp::Manifest::new(id, Transport::Stdio { command: "x".into(), args: vec![], env: BTreeMap::new() }))
                .expect("save");
        }
        let resolved = resolve(&config, &["b".to_string(), "a".to_string()], &[]);
        assert_eq!(
            resolved.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
}
