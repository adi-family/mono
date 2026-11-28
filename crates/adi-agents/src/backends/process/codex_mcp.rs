//! Translating this run's declared MCP servers into Codex's own `-c mcp_servers.<id>.…` config
//! overrides, and the one thing it cannot express.
//!
//! Codex's `mcp_servers` table takes a stdio server's `command`/`args`/`env` as literal TOML, the
//! same shape `codex mcp add -- …` itself writes (confirmed against a real run of it, not guessed
//! from the schema docs alone). A remote server is narrower: `url` plus, for auth,
//! `bearer_token_env_var` — the *name* of an environment variable Codex reads the token from at
//! launch, never a literal token in the config (confirmed against `codex mcp add --url …
//! --bearer-token-env-var …`). So an http server whose only header is an `Authorization: Bearer
//! <token>` maps onto that one knob, with the raw token carried over as that very env var (see
//! [`bearer_env`]); anything else — a second header, a differently-named one, no `Bearer ` prefix
//! — Codex simply cannot be told to send, and [`validate`] refuses before a launch pretends
//! otherwise.
//!
//! The raw token comes from [`ResolvedHeader::Prefixed`] when the definition names its secret
//! with `{ secret = "…", prefix = "Bearer " }` (or [`adi_mcp::Value::bearer`]) — the preferred
//! shape, since Codex's own `bearer_token_env_var` reads a bare token and never wants `Bearer `
//! glued onto it. An older definition that instead baked the word `Bearer` into the secret's own
//! value resolves to [`ResolvedHeader::Plain`] and is still recognized by stripping the prefix
//! back off the rendered string — kept for exactly that backward compatibility, not as a second
//! way to write a new definition.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::mcp_servers::{ResolvedHeader, ResolvedMcpServer, ResolvedTransport};

const BEARER_PREFIX: &str = adi_mcp::BEARER_PREFIX;

/// The env var name a server's bearer token travels under — shared between [`config_flags`]
/// (which names it to Codex) and [`bearer_env`] (which sets it), so the two can never disagree.
fn bearer_env_var(server_id: &str) -> String {
    let safe: String = server_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ADI_MCP_{safe}_BEARER")
}

/// The single header a bearer-token server carries, as the *raw* token Codex's own knob wants —
/// `Some` only when there is exactly one header, it is named `Authorization`, and it resolves to
/// a bearer scheme. Codex has no way to express anything broader than this one case.
///
/// A [`ResolvedHeader::Prefixed`] header whose prefix is exactly `Bearer ` hands back its `raw`
/// field directly — the preferred path, since it already *is* the token, however it is spelled.
/// A [`ResolvedHeader::Plain`] one — a literal, or a secret resolved with no `prefix` at all — is
/// still checked the old way, by stripping the literal `Bearer ` text off the front: the back
/// compatibility a definition written before this field existed still needs.
fn bearer_header(headers: &BTreeMap<String, ResolvedHeader>) -> Option<&str> {
    let mut iter = headers.iter();
    let (name, value) = iter.next()?;
    if iter.next().is_some() || name != "Authorization" {
        return None;
    }
    match value {
        ResolvedHeader::Prefixed { prefix, raw } if prefix == BEARER_PREFIX => Some(raw.as_str()),
        ResolvedHeader::Prefixed { .. } => None,
        ResolvedHeader::Plain(rendered) => rendered.strip_prefix(BEARER_PREFIX),
    }
}

/// Refuse before launch if any declared server is something Codex's config cannot express — see
/// the module docs. Stdio is always expressible; http is, exactly when [`bearer_header`] matches.
///
/// # Errors
/// [`Error::Launch`] naming the server and why, so the operator reads a reason rather than a
/// server that silently never loaded.
pub(crate) fn validate(servers: &[ResolvedMcpServer]) -> Result<()> {
    for server in servers {
        if let ResolvedTransport::Http { headers, .. } = &server.transport
            && bearer_header(headers).is_none()
        {
            return Err(Error::Launch(format!(
                "process:codex cannot reach the MCP server {:?}: Codex's own config only sends a \
                 single bearer token (one \"Authorization: Bearer …\" header), and this server's \
                 headers are not exactly that. Use harness:claude-sdk for it instead, or narrow \
                 its headers to one Authorization header naming a secret with `prefix = \"Bearer \
                 \"` (or `adi_mcp::Value::bearer`)",
                server.id
            )));
        }
    }
    Ok(())
}

/// The `-c key=value` pairs that configure these servers — appended to Codex's own command line
/// alongside its other `-c` overrides (`reasoning_effort`, `web_search`, …). Callers are expected
/// to have run [`validate`] first; an http server `validate` would have refused is simply skipped
/// here rather than mis-rendered.
pub(crate) fn config_flags(servers: &[ResolvedMcpServer]) -> Vec<String> {
    let mut flags = Vec::new();
    for server in servers {
        let prefix = format!("mcp_servers.{}", server.id);
        match &server.transport {
            ResolvedTransport::Stdio { command, args, env } => {
                flags.push(format!("{prefix}.command={}", toml_string(command)));
                if !args.is_empty() {
                    flags.push(format!("{prefix}.args={}", toml_array(args)));
                }
                for (key, value) in env {
                    flags.push(format!("{prefix}.env.{key}={}", toml_string(value)));
                }
            }
            ResolvedTransport::Http { url, headers } => {
                flags.push(format!("{prefix}.url={}", toml_string(url)));
                if bearer_header(headers).is_some() {
                    flags.push(format!(
                        "{prefix}.bearer_token_env_var={}",
                        toml_string(&bearer_env_var(&server.id))
                    ));
                }
            }
        }
    }
    flags
}

/// The env vars [`config_flags`] named by [`bearer_env_var`] — set on the spawned process so the
/// token Codex was told to read by name is actually there to read, raw, with no `Bearer ` on it.
pub(crate) fn bearer_env(servers: &[ResolvedMcpServer]) -> Vec<(String, String)> {
    servers
        .iter()
        .filter_map(|server| {
            let ResolvedTransport::Http { headers, .. } = &server.transport else {
                return None;
            };
            let token = bearer_header(headers)?;
            Some((bearer_env_var(&server.id), token.to_string()))
        })
        .collect()
}

/// A TOML-literal string, the same way `argv`'s own `add_dir` override already builds one: JSON's
/// string escaping is a strict subset of TOML's.
fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

/// A TOML-literal array of strings, same reasoning as [`toml_string`].
fn toml_array(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(id: &str, env: &[(&str, &str)]) -> ResolvedMcpServer {
        ResolvedMcpServer {
            id: id.to_string(),
            transport: ResolvedTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "n8n-mcp".to_string()],
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
        }
    }

    /// Builds the http fixture from a bearer secret already split into prefix/raw — the shape
    /// [`crate::mcp_servers::resolve`] produces for `{ secret = "…", prefix = "Bearer " }`.
    fn http_bearer(id: &str, raw: &str) -> ResolvedMcpServer {
        ResolvedMcpServer {
            id: id.to_string(),
            transport: ResolvedTransport::Http {
                url: "http://n8n.adi/mcp/".to_string(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    ResolvedHeader::Prefixed {
                        prefix: BEARER_PREFIX.to_string(),
                        raw: raw.to_string(),
                    },
                )]),
            },
        }
    }

    /// Builds the http fixture from plain, already-rendered header strings — a literal, or an old
    /// definition whose secret's own value already carried the word `Bearer`.
    fn http_plain(id: &str, headers: &[(&str, &str)]) -> ResolvedMcpServer {
        ResolvedMcpServer {
            id: id.to_string(),
            transport: ResolvedTransport::Http {
                url: "http://n8n.adi/mcp/".to_string(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), ResolvedHeader::Plain(v.to_string())))
                    .collect(),
            },
        }
    }

    #[test]
    fn a_stdio_server_always_validates_and_renders_its_command_args_and_env() {
        let servers = [stdio("n8n", &[("N8N_API_KEY", "sekret")])];
        assert!(validate(&servers).is_ok());
        let flags = config_flags(&servers);
        assert_eq!(
            flags,
            vec![
                "mcp_servers.n8n.command=\"npx\"".to_string(),
                "mcp_servers.n8n.args=[\"-y\",\"n8n-mcp\"]".to_string(),
                "mcp_servers.n8n.env.N8N_API_KEY=\"sekret\"".to_string(),
            ]
        );
        assert!(bearer_env(&servers).is_empty());
    }

    #[test]
    fn a_bearer_secret_header_validates_and_hands_codex_the_raw_token() {
        let servers = [http_bearer("n8n", "tok-123")];
        assert!(validate(&servers).is_ok());
        let flags = config_flags(&servers);
        assert_eq!(
            flags,
            vec![
                "mcp_servers.n8n.url=\"http://n8n.adi/mcp/\"".to_string(),
                "mcp_servers.n8n.bearer_token_env_var=\"ADI_MCP_N8N_BEARER\"".to_string(),
            ]
        );
        assert_eq!(
            bearer_env(&servers),
            vec![("ADI_MCP_N8N_BEARER".to_string(), "tok-123".to_string())],
            "the env var carries the bare token, never 'Bearer ' glued onto it"
        );
    }

    #[test]
    fn an_old_plain_bearer_header_still_validates_the_back_compatible_way() {
        let servers = [http_plain("n8n", &[("Authorization", "Bearer tok-123")])];
        assert!(validate(&servers).is_ok());
        assert_eq!(
            bearer_env(&servers),
            vec![("ADI_MCP_N8N_BEARER".to_string(), "tok-123".to_string())]
        );
    }

    #[test]
    fn a_prefix_other_than_bearer_is_not_mistaken_for_one() {
        let servers = [ResolvedMcpServer {
            id: "x".to_string(),
            transport: ResolvedTransport::Http {
                url: "http://x".to_string(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    ResolvedHeader::Prefixed {
                        prefix: "Token ".to_string(),
                        raw: "tok".to_string(),
                    },
                )]),
            },
        }];
        assert!(validate(&servers).is_err());
    }

    #[test]
    fn an_http_server_with_anything_else_refuses_clearly_rather_than_mis_sending() {
        let bad = [
            http_plain("none", &[]),
            http_plain(
                "extra",
                &[("Authorization", "Bearer tok"), ("X-Other", "y")],
            ),
            http_plain("wrong-name", &[("X-Api-Key", "tok")]),
            http_plain("no-bearer", &[("Authorization", "tok")]),
        ];
        for server in bad {
            let err = validate(std::slice::from_ref(&server)).expect_err(&server.id);
            assert!(format!("{err}").contains(&server.id), "{err}");
        }
    }

    #[test]
    fn server_ids_with_hyphens_become_safe_env_var_names() {
        let servers = [http_bearer("my-n8n.server", "x")];
        assert_eq!(
            bearer_env(&servers),
            vec![("ADI_MCP_MY_N8N_SERVER_BEARER".to_string(), "x".to_string())]
        );
    }
}
