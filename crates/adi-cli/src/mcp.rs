//! The `mcp` command group: register and read MCP server definitions, the thing `agents save
//! --mcp <id>` attaches to an agent.
//!
//! A definition never stores a secret's value, only its name — give a header or env value as
//! `secret:NAME` to name one, or anything else for a literal. `bearer:NAME` is shorthand for a
//! secret named with the `Bearer ` prefix most APIs' `Authorization` header wants
//! (`adi_mcp::Value::bearer`): the stored secret is the raw token, and this header renders as
//! `Bearer <token>` at launch — never write the word `Bearer` into the secret's own value.
//! `mcp show` names which secrets a server still needs and whether each is set anywhere on this
//! machine (not whether it is *attached* to any particular agent — that is `agents show`'s own
//! business).

use std::collections::BTreeMap;

use adi_core::{Adi, McpManifest, McpServer, McpTransport, McpValue};
use clap::Subcommand;

use crate::format::print_json;

// `Save` carries the whole definition's worth of flags, dwarfing the id-only variants — the same
// shape `LlmCommand::Save` is in, for the same reason (see its own comment).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum McpCommand {
    /// List the registered MCP server definitions.
    List {
        /// Only servers filed under this project id.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one server's definition in full, including which secrets it needs and whether each
    /// is set anywhere on this machine (global or any project).
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or replace a server definition. A save states the whole object — like `llm backends
    /// save`, a definition is a handful of fields and an omit-to-keep save is how a header quietly
    /// disappears.
    Save {
        /// The id `agents save --mcp` names. Also the filename, so it is a single path segment.
        id: String,
        /// What a human calls it, shown in the panel. Defaults to `id`.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// File this server under a project id; omit for a global server.
        #[arg(long)]
        project: Option<String>,
        /// `http` (streamable, the default when `--url` is given) or `stdio` (the default when
        /// `--command` is given). Only needed to disambiguate a save that gives neither.
        #[arg(long)]
        transport: Option<String>,
        /// The server's URL, for `--transport http`.
        #[arg(long)]
        url: Option<String>,
        /// A request header, as `NAME=VALUE` — give `secret:THE_NAME` to resolve a secret by name
        /// at launch (against whichever agent's run reads it) instead of writing one literally,
        /// or `bearer:THE_NAME` for the common `Authorization: Bearer <token>` case (the secret
        /// stores the raw token; `Bearer ` is glued on at launch, never stored). Repeatable;
        /// replaces the whole header set on every save.
        #[arg(long = "header")]
        headers: Vec<String>,
        /// The command to launch, for `--transport stdio`.
        #[arg(long)]
        command: Option<String>,
        /// One argument to the command, in order. Repeatable; replaces the whole argument list on
        /// every save.
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
        /// An environment variable for the command, as `KEY=VALUE` (`secret:THE_NAME` or
        /// `bearer:THE_NAME` resolve a secret, the same as `--header`). Repeatable; replaces the
        /// whole env on every save.
        #[arg(long = "env")]
        env: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Permanently delete a server definition. An agent that still names it in its own `mcp` list
    /// is not followed here — it is the same dangling-id tolerance a deleted tool already gets.
    Rm { id: String },
    /// Help written for a language model rather than for a person. Bare `llm` prints it too.
    Llm(crate::llm_help::LlmHelp),
}

/// Dispatch an `mcp` subcommand over the MCP server registry.
pub(crate) fn run_mcp(adi: Adi, command: McpCommand) -> Result<(), String> {
    let store = adi.mcp();
    match command {
        McpCommand::Llm(_) => unreachable!("`llm` is answered in main"),
        McpCommand::List { project, json } => {
            let mut servers = store.list().map_err(|e| e.to_string())?;
            if let Some(pid) = project.as_deref() {
                servers.retain(|s| s.manifest.project.as_deref() == Some(pid));
            }
            if json {
                print_json(&servers);
            } else if servers.is_empty() {
                println!("No MCP servers registered.");
            } else {
                for server in &servers {
                    print_server(server);
                }
            }
        }
        McpCommand::Show { id, json } => {
            let server = store
                .get(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no such mcp server: {id}"))?;
            if json {
                print_json(&server);
            } else {
                print_server(&server);
            }
        }
        McpCommand::Save {
            id,
            name,
            description,
            project,
            transport,
            url,
            headers,
            command,
            args,
            env,
            json,
        } => {
            let transport = build_transport(transport.as_deref(), url, headers, command, args, env)?;
            let mut manifest = McpManifest::new(name.unwrap_or_else(|| id.clone()), transport);
            if let Some(description) = description {
                manifest = manifest.with_description(description);
            }
            if let Some(project) = project {
                manifest = manifest.with_project(project);
            }
            let server = store.save(&id, manifest).map_err(|e| e.to_string())?;
            if json {
                print_json(&server);
            } else {
                println!("Saved mcp server {} ({}).", server.display_name(), server.id);
                print_server(&server);
            }
        }
        McpCommand::Rm { id } => {
            if store.delete(&id).map_err(|e| e.to_string())? {
                println!("Deleted mcp server {id}.");
            } else {
                println!("No such mcp server: {id}.");
            }
        }
    }
    Ok(())
}

/// Read `--transport`, or infer it from whichever of `--url`/`--command` was actually given, and
/// build the one the flags describe.
fn build_transport(
    transport: Option<&str>,
    url: Option<String>,
    headers: Vec<String>,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
) -> Result<McpTransport, String> {
    let kind = match transport.map(str::trim) {
        Some("http") => "http",
        Some("stdio") => "stdio",
        Some(other) => return Err(format!("--transport must be 'http' or 'stdio', got {other:?}")),
        None if url.is_some() => "http",
        None if command.is_some() => "stdio",
        None => {
            return Err(
                "give --url (http) or --command (stdio), or say --transport explicitly".to_string(),
            );
        }
    };
    match kind {
        "http" => Ok(McpTransport::Http {
            url: url.ok_or_else(|| "--transport http needs --url".to_string())?,
            headers: parse_values(headers)?,
        }),
        _ => Ok(McpTransport::Stdio {
            command: command.ok_or_else(|| "--transport stdio needs --command".to_string())?,
            args,
            env: parse_values(env)?,
        }),
    }
}

/// Parse repeated `KEY=VALUE` flags into the manifest's value map — `secret:NAME` becomes a
/// secret reference, anything else a literal. The value is everything after the first `=`, so it
/// may itself contain one (a URL-shaped literal, say).
fn parse_values(values: Vec<String>) -> Result<BTreeMap<String, McpValue>, String> {
    let mut out = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            return Err(format!("expected KEY=VALUE, got: {value}"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err("a header/env flag needs a name before the `=`".to_string());
        }
        out.insert(key.to_string(), parse_value(value));
    }
    Ok(out)
}

fn parse_value(raw: &str) -> McpValue {
    if let Some(name) = raw.strip_prefix("bearer:") {
        return McpValue::bearer(name);
    }
    match raw.strip_prefix("secret:") {
        Some(name) => McpValue::secret(name),
        None => McpValue::literal(raw),
    }
}

/// The reverse of [`parse_value`], for printing a value back the way it was given. A prefix other
/// than the `bearer:` shorthand recognizes (there is currently no flag syntax for one) still
/// prints something readable rather than silently dropping it.
fn render_value(value: &McpValue) -> String {
    match value {
        McpValue::Literal(text) => text.clone(),
        McpValue::Secret { secret, prefix } if prefix.is_empty() => format!("secret:{secret}"),
        McpValue::Secret { secret, prefix } if prefix == adi_core::MCP_BEARER_PREFIX => format!("bearer:{secret}"),
        McpValue::Secret { secret, prefix } => format!("secret:{secret} (prefix {prefix:?})"),
    }
}

fn print_server(server: &McpServer) {
    let project = server.manifest.project.as_deref().unwrap_or("global");
    println!("{} — {project} [{}]", server.id, transport_label(&server.manifest.transport));
    if let Some(description) = &server.manifest.description {
        println!("  {description}");
    }
    match &server.manifest.transport {
        McpTransport::Http { url, headers } => {
            println!("  url: {url}");
            for (name, value) in headers {
                println!("  header: {name}={}", render_value(value));
            }
        }
        McpTransport::Stdio { command, args, env } => {
            println!("  command: {command} {}", args.join(" "));
            for (name, value) in env {
                println!("  env: {name}={}", render_value(value));
            }
        }
    }
    let required = server.manifest.transport.required_secrets();
    if !required.is_empty() {
        let secrets = adi_core::Secrets::open();
        let missing: Vec<&String> = required
            .iter()
            .filter(|name| !secrets.resolve(server.manifest.project.as_deref()).is_ok_and(|m| m.contains_key(*name)))
            .collect();
        if missing.is_empty() {
            println!("  secrets: {} (all set)", required.into_iter().collect::<Vec<_>>().join(", "));
        } else {
            println!(
                "  secrets: {} — missing on this machine: {}",
                required.iter().cloned().collect::<Vec<_>>().join(", "),
                missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
}

fn transport_label(transport: &McpTransport) -> &'static str {
    match transport {
        McpTransport::Http { .. } => "http",
        McpTransport::Stdio { .. } => "stdio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A parser for this group alone — the same harness every other command-group test file uses,
    /// so a test states argv the way a user types it without carrying the top-level CLI's own
    /// required arguments.
    #[derive(Debug, Parser)]
    struct Harness {
        #[command(subcommand)]
        command: McpCommand,
    }

    fn parse(args: &[&str]) -> McpCommand {
        Harness::try_parse_from(std::iter::once("mcp").chain(args.iter().copied()))
            .expect("parses")
            .command
    }

    #[test]
    fn save_infers_http_from_url_and_stdio_from_command() {
        let McpCommand::Save { transport, url, .. } =
            parse(&["save", "n8n", "--url", "http://n8n.adi/mcp/"])
        else {
            panic!("expected save");
        };
        assert_eq!(transport, None);
        assert_eq!(url, Some("http://n8n.adi/mcp/".to_string()));

        let McpCommand::Save { command, .. } = parse(&["save", "n8n", "--command", "npx"]) else {
            panic!("expected save");
        };
        assert_eq!(command, Some("npx".to_string()));
    }

    #[test]
    fn stdio_args_may_start_with_a_hyphen() {
        let McpCommand::Save { args, .. } =
            parse(&["save", "n8n", "--command", "npx", "--arg", "-y", "--arg", "n8n-mcp"])
        else {
            panic!("expected save");
        };
        assert_eq!(args, vec!["-y".to_string(), "n8n-mcp".to_string()]);
    }

    #[test]
    fn a_header_value_prefixed_secret_colon_is_a_secret_reference() {
        assert_eq!(parse_value("secret:N8N_TOKEN"), McpValue::secret("N8N_TOKEN"));
        assert_eq!(parse_value("application/json"), McpValue::literal("application/json"));
        assert_eq!(render_value(&McpValue::secret("X")), "secret:X");
        assert_eq!(render_value(&McpValue::literal("y")), "y");
    }

    #[test]
    fn a_header_value_prefixed_bearer_colon_is_a_secret_with_the_bearer_prefix() {
        assert_eq!(parse_value("bearer:N8N_MCP_ACCESS_TOKEN"), McpValue::bearer("N8N_MCP_ACCESS_TOKEN"));
        assert_eq!(render_value(&McpValue::bearer("X")), "bearer:X");
        // The raw token is never stored with 'Bearer ' baked in — that's the whole point.
        assert_eq!(
            McpValue::bearer("X"),
            McpValue::secret_with_prefix("X", adi_core::MCP_BEARER_PREFIX)
        );
    }

    #[test]
    fn a_value_may_contain_its_own_equals_sign() {
        let values = parse_values(vec!["Authorization=Bearer a=b".to_string()]).expect("parse");
        assert_eq!(values.get("Authorization"), Some(&McpValue::literal("Bearer a=b")));
    }

    #[test]
    fn build_transport_refuses_without_enough_to_build_one() {
        assert!(build_transport(None, None, vec![], None, vec![], vec![]).is_err());
        assert!(build_transport(Some("http"), None, vec![], None, vec![], vec![]).is_err());
        assert!(build_transport(Some("stdio"), None, vec![], None, vec![], vec![]).is_err());
        assert!(build_transport(Some("carrier-pigeon"), None, vec![], None, vec![], vec![]).is_err());
    }

    #[test]
    fn build_transport_builds_the_kind_asked_for() {
        let transport = build_transport(
            None,
            Some("http://n8n.adi/mcp/".to_string()),
            vec!["Authorization=secret:N8N_TOKEN".to_string()],
            None,
            vec![],
            vec![],
        )
        .expect("http");
        assert!(matches!(transport, McpTransport::Http { .. }));

        let transport = build_transport(
            None,
            None,
            vec![],
            Some("npx".to_string()),
            vec!["-y".to_string()],
            vec!["KEY=VALUE".to_string()],
        )
        .expect("stdio");
        assert!(matches!(transport, McpTransport::Stdio { .. }));
    }
}
