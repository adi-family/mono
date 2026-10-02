//! The node's own small local endpoint behind the `channel-reply` tool
//! (`docs/channels.md` §5): look up the connection a run id maps to, then `POST /send` on the
//! router. Mirrors `Report`'s shape — announce something mid-turn — without the harness's
//! suspend/resume machinery, because a channel post, unlike a question, never blocks the turn.

use crate::connection::Connections;
use crate::error::{Error, Result};
use crate::router_api::RouterApi;
use crate::token;

/// Post `text` back through whichever connection `run_id`'s thread map names.
///
/// `agent` is accepted (it's what the tool already has in its environment, from the same
/// `ADI_AGENT`/`ADI_RUN_ID` pair `adi_agents::launcher::by_caller` reads) but not required for the
/// lookup — a run id is unique platform-wide, so [`Connections::find_by_run`] needs nothing else.
/// It is kept as a parameter rather than dropped, in case a future caller wants to validate the
/// two agree before this does anything.
///
/// # Errors
/// [`Error::NotFound`] if `run_id` maps to no connection this node holds (the ordinary case for
/// every run that didn't open from a channel — [`crate::finished`] treats the same situation as
/// "nothing to do"; this endpoint is reached by a tool that believes it's in a channel
/// conversation, so a caller here gets told rather than silently dropped); otherwise whatever the
/// store, the secrets store, or the router call itself returns.
pub fn handle(
    connections: &Connections,
    secrets: &adi_secrets::Secrets,
    router_url: &str,
    _agent: &str,
    run_id: &str,
    text: &str,
) -> Result<()> {
    let (connection, thread) = connections
        .find_by_run(run_id)?
        .ok_or_else(|| Error::NotFound(run_id.to_string()))?;
    if connection.manifest.paused {
        return Ok(());
    }
    let node_token = token::load(secrets, &connection.manifest.provider)?.ok_or_else(|| {
        Error::Router(format!(
            "no node token on file for provider {}",
            connection.manifest.provider
        ))
    })?;
    RouterApi::new(router_url).send(&node_token, &connection.id, Some(&thread), text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Target;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-reply-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    #[test]
    fn a_run_id_with_no_connection_is_not_found() {
        let cfg = scratch("notfound");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        let err = handle(
            &connections,
            &secrets,
            "http://127.0.0.1:1",
            "solver",
            "no-such-run",
            "hi",
        )
        .expect_err("no connection maps to this run");
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn a_paused_connection_drops_the_post_without_erroring() {
        let cfg = scratch("paused");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "solver".into(),
                },
            )
            .unwrap();
        connections.bind_thread(&created.id, "chat-1", "run-1").unwrap();
        connections.set_paused(&created.id, true).unwrap();

        handle(
            &connections,
            &secrets,
            "http://127.0.0.1:1",
            "solver",
            "run-1",
            "hi",
        )
        .expect("paused connections drop the post, not error");
    }

    #[test]
    fn a_missing_node_token_is_a_router_error() {
        let cfg = scratch("no-token");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        let created = connections
            .create(
                "telegram",
                Target::Agent {
                    agent: "solver".into(),
                },
            )
            .unwrap();
        connections.bind_thread(&created.id, "chat-1", "run-1").unwrap();

        let err = handle(
            &connections,
            &secrets,
            "http://127.0.0.1:1",
            "solver",
            "run-1",
            "hi",
        )
        .expect_err("no token means no send");
        assert!(matches!(err, Error::Router(_)));
    }
}
