//! `GET /api/mcp` and its mutations — MCP server definitions under `~/.adi/mono/mcp`, the thing
//! `agents save --mcp <id>` attaches to an agent.
//!
//! A definition never carries a secret's value, only the name it needs (see
//! [`McpValueDto::Secret`](crate::types::McpValueDto::Secret)) — `required_secrets`/
//! `missing_secrets` on [`McpServerDto`] are this surface's whole answer to "which secrets does
//! this server still need", read fresh off this machine's secrets store on every list/show, never
//! stored.

use adi_mcp::{Error as McpStoreError, Manifest, McpServers, Transport, Value};
use adi_secrets::Secrets;

use crate::types::{
    McpServerDto, McpServerRef, McpServersState, McpTransportDto, McpValueDto, SaveMcpServer,
};

use super::response::{FromBody, Response, error, mutate, ok_json};

/// `GET /api/mcp` — every registered server, with its own secret requirements read fresh.
#[must_use]
pub fn mcp_servers(store: &McpServers) -> Response {
    let secrets = Secrets::with_config(store.config().clone());
    match store.list() {
        Ok(list) => ok_json(&McpServersState {
            servers: list.into_iter().map(|s| server_dto(s, &secrets)).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/mcp/save` — create or replace a server definition, then report the fresh list.
#[must_use]
pub fn save_mcp_server(store: &McpServers, body: &[u8]) -> Response {
    mutate(
        body,
        |req: SaveMcpServer| {
            let mut manifest = Manifest::new(req.name, from_dto(req.transport));
            if let Some(description) = req.description {
                manifest = manifest.with_description(description);
            }
            if let Some(project) = req.project {
                manifest = manifest.with_project(project);
            }
            store.save(req.id.trim(), manifest)
        },
        || mcp_servers(store),
    )
}

/// `POST /api/mcp/remove` — permanently delete a server definition. An agent that still names it
/// in its own `mcp` list is not followed here — the same dangling-id tolerance a deleted tool
/// already gets.
#[must_use]
pub fn remove_mcp_server(store: &McpServers, body: &[u8]) -> Response {
    mutate(
        body,
        |req: McpServerRef| store.delete(req.id.trim()),
        || mcp_servers(store),
    )
}

/// One server's definition, with its secret requirements resolved against this machine's own
/// store — global plus this server's own project scope, the same precedence
/// `adi_secrets::Secrets::resolve` already gives a run.
fn server_dto(server: adi_mcp::McpServer, secrets: &Secrets) -> McpServerDto {
    let required = server.manifest.transport.required_secrets();
    let set = secrets
        .resolve(server.manifest.project.as_deref())
        .unwrap_or_default();
    let missing = required
        .iter()
        .filter(|name| !set.contains_key(name.as_str()))
        .cloned()
        .collect();
    McpServerDto {
        id: server.id,
        name: server.manifest.name,
        description: server.manifest.description,
        project: server.manifest.project,
        transport: to_dto(server.manifest.transport),
        required_secrets: required.into_iter().collect(),
        missing_secrets: missing,
        created_at: server.manifest.created_at,
        updated_at: server.manifest.updated_at,
    }
}

fn to_dto(transport: Transport) -> McpTransportDto {
    match transport {
        Transport::Http { url, headers } => McpTransportDto::Http {
            url,
            headers: headers.into_iter().map(|(k, v)| (k, value_to_dto(v))).collect(),
        },
        Transport::Stdio { command, args, env } => McpTransportDto::Stdio {
            command,
            args,
            env: env.into_iter().map(|(k, v)| (k, value_to_dto(v))).collect(),
        },
    }
}

fn value_to_dto(value: Value) -> McpValueDto {
    match value {
        Value::Literal(text) => McpValueDto::Literal(text),
        Value::Secret { secret, prefix } => McpValueDto::Secret { secret, prefix },
    }
}

fn from_dto(transport: McpTransportDto) -> Transport {
    match transport {
        McpTransportDto::Http { url, headers } => Transport::Http {
            url,
            headers: headers.into_iter().map(|(k, v)| (k, value_from_dto(v))).collect(),
        },
        McpTransportDto::Stdio { command, args, env } => Transport::Stdio {
            command,
            args,
            env: env.into_iter().map(|(k, v)| (k, value_from_dto(v))).collect(),
        },
    }
}

fn value_from_dto(value: McpValueDto) -> Value {
    match value {
        McpValueDto::Literal(text) => Value::Literal(text),
        McpValueDto::Secret { secret, prefix } => Value::Secret { secret, prefix },
    }
}

// Map an mcp-store error to an HTTP status: bad id → 400, missing definition → 404, anything else
// (a store I/O or TOML failure) → 500 — the same three-way split `ToolStoreError` already gets.
impl From<&McpStoreError> for Response {
    fn from(e: &McpStoreError) -> Self {
        let status = match e {
            McpStoreError::InvalidId(_) => 400,
            McpStoreError::NotFound(_) => 404,
            McpStoreError::Config(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for SaveMcpServer {
    const EXPECTED: &'static str =
        "expected JSON body { \"id\": \"…\", \"transport\": { \"kind\": \"http\"|\"stdio\", … } }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for McpServerRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;
    use std::collections::BTreeMap;

    fn scratch(tag: &str) -> McpServers {
        let dir = std::env::temp_dir().join(format!(
            "adi-webapp-api-mcp-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        McpServers::with_config(Config::with_root(dir))
    }

    fn http_body(id: &str, url: &str, secret: Option<&str>) -> Vec<u8> {
        let headers: BTreeMap<String, McpValueDto> = match secret {
            Some(name) => BTreeMap::from([(
                "Authorization".to_string(),
                McpValueDto::Secret { secret: name.to_string(), prefix: String::new() },
            )]),
            None => BTreeMap::new(),
        };
        serde_json::to_vec(&SaveMcpServer {
            id: id.to_string(),
            name: String::new(),
            description: None,
            project: None,
            transport: McpTransportDto::Http { url: url.to_string(), headers },
        })
        .expect("serialize")
    }

    #[test]
    fn save_then_list_round_trips_and_a_blank_name_falls_back_to_the_id() {
        let store = scratch("round-trip");
        let saved = save_mcp_server(&store, &http_body("n8n", "http://n8n.adi/mcp/", None));
        assert_eq!(saved.status, 200, "{}", saved.body);
        let state: McpServersState = serde_json::from_str(&saved.body).expect("state");
        assert_eq!(state.servers.len(), 1);
        assert_eq!(state.servers[0].id, "n8n");
        assert_eq!(state.servers[0].name, "n8n");
    }

    #[test]
    fn a_bearer_prefix_round_trips_through_save_and_list() {
        let store = scratch("bearer-prefix");
        let headers = BTreeMap::from([(
            "Authorization".to_string(),
            McpValueDto::Secret { secret: "N8N_MCP_ACCESS_TOKEN".to_string(), prefix: "Bearer ".to_string() },
        )]);
        let body = serde_json::to_vec(&SaveMcpServer {
            id: "n8n".to_string(),
            name: String::new(),
            description: None,
            project: None,
            transport: McpTransportDto::Http { url: "http://n8n.adi/mcp-server/http".to_string(), headers },
        })
        .expect("serialize");
        let saved = save_mcp_server(&store, &body);
        assert_eq!(saved.status, 200, "{}", saved.body);

        let res = mcp_servers(&store);
        let state: McpServersState = serde_json::from_str(&res.body).expect("state");
        let McpTransportDto::Http { headers, .. } = &state.servers[0].transport else {
            panic!("http");
        };
        assert_eq!(
            headers.get("Authorization"),
            Some(&McpValueDto::Secret {
                secret: "N8N_MCP_ACCESS_TOKEN".to_string(),
                prefix: "Bearer ".to_string()
            })
        );
        assert_eq!(state.servers[0].required_secrets, vec!["N8N_MCP_ACCESS_TOKEN".to_string()]);
    }

    #[test]
    fn required_and_missing_secrets_are_read_fresh_not_stored() {
        let store = scratch("missing");
        let _ = save_mcp_server(&store, &http_body("n8n", "http://n8n.adi/mcp/", Some("N8N_TOKEN")));

        let res = mcp_servers(&store);
        let state: McpServersState = serde_json::from_str(&res.body).expect("state");
        assert_eq!(state.servers[0].required_secrets, vec!["N8N_TOKEN".to_string()]);
        assert_eq!(state.servers[0].missing_secrets, vec!["N8N_TOKEN".to_string()]);

        Secrets::with_config(store.config().clone())
            .set(None, "N8N_TOKEN", "sekret", None)
            .expect("set secret");
        let res = mcp_servers(&store);
        let state: McpServersState = serde_json::from_str(&res.body).expect("state");
        assert!(state.servers[0].missing_secrets.is_empty());
    }

    #[test]
    fn remove_deletes_it_and_an_unknown_id_404s() {
        let store = scratch("remove");
        let _ = save_mcp_server(&store, &http_body("n8n", "http://n8n.adi/mcp/", None));

        let body = serde_json::to_vec(&McpServerRef { id: "n8n".to_string() }).expect("ser");
        let res = remove_mcp_server(&store, &body);
        assert_eq!(res.status, 200, "{}", res.body);
        let state: McpServersState = serde_json::from_str(&res.body).expect("state");
        assert!(state.servers.is_empty());

        let res = Response::from(&McpStoreError::NotFound("nope".to_string()));
        assert_eq!(res.status, 404);
    }

    #[test]
    fn a_blank_id_is_refused_with_400_before_touching_the_store() {
        let store = scratch("blank-id");
        let body = http_body("  ", "http://x", None);
        let res = save_mcp_server(&store, &body);
        assert_eq!(res.status, 400, "{}", res.body);
    }
}
