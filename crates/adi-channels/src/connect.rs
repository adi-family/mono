//! The synchronous half of `docs/channels.md` §7's "Connect": call the router's `POST /register`
//! (which mints this `(node, provider)`'s token — once — and a fresh connection in the same
//! call), record the token, and mirror that connection locally under the id the router minted
//! for it.
//!
//! **Not included here: opening the `/subscribe` socket.** That's
//! [`crate::client::RouterClient::run`], an async task owned by the Hive-managed `adi-channelsd`
//! runtime. This module handles registration synchronously from its API's blocking pool.
//!
//! [`disconnect`] tells the router first (`POST /disconnect`, added by ADI-MONO-122 once
//! ADI-MONO-120/121 both flagged the gap — see `docs/channels.md` §1 step 4) and only then drops
//! this node's own state (the connection row, and the provider's node token once nothing else
//! needs it) — the router's own entry and the D1 routing row are the authoritative record of
//! whether a chat/workspace still reaches this node, so they go first.

use crate::connection::{Allowlist, Connection, Connections, Target};
use crate::error::{Error, Result};
use crate::node_id;
use crate::router_api::RouterApi;
use crate::token;

/// What a successful connect hands back: the new (unlinked) connection row, mirrored locally
/// under the id the router minted for it, and the install URL to show the operator — already the
/// whole `t.me/<bot>?start=<code>` for Telegram, `None` for a provider with no known way to build
/// one from a bare code. `install_url_group` (ADI-MONO-125) is Telegram's "add to a group" twin
/// of `install_url`, `None` for every other provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Connected {
    pub connection: Connection,
    pub install_url: Option<String>,
    pub install_url_group: Option<String>,
}

/// Where the router is, and how this node proves it may mint a token — the two facts
/// [`connect`] needs about the router itself, grouped so the function reads as "the node, the
/// target, the router" rather than five same-shaped strings in a row. `admin_secret` is `None`
/// for the ordinary case since ADI-MONO-125 (open self-registration — no admin secret to hold),
/// `Some` only for an operator's own node that still wants to skip the rate limit/pending cap.
#[derive(Debug, Clone, Copy)]
pub struct Router<'a> {
    pub url: &'a str,
    pub admin_secret: Option<&'a str>,
}

/// `adi-mono channels connect <svc> --agent <a>` / `POST /api/channels/connect`
/// (`docs/channels.md` §7): register this node for `provider` (minting or reusing its node
/// token) and mirror the fresh, unlinked connection the router created in the same call. The
/// local mirror's allowlist is set from `allowlist` when given, else left at
/// [`Allowlist::default`] (owner-only) regardless of what the router itself was told — the
/// router's own copy is never enforced (§5/§8: that's this node's job alone).
///
/// # Errors
/// Whatever [`RouterApi::register`], [`token::save`], [`node_id::get_or_create`], or
/// [`Connections::create_with_id`] return.
pub fn connect(
    connections: &Connections,
    secrets: &adi_secrets::Secrets,
    config: &adi_config::Config,
    router: Router<'_>,
    provider: &str,
    target: Target,
    allowlist: Option<&Allowlist>,
) -> Result<Connected> {
    let node = node_id::get_or_create(config)?;
    let registered = RouterApi::new(router.url).register(
        &node,
        provider,
        &target,
        allowlist,
        router.admin_secret,
    )?;
    token::save(secrets, provider, &registered.token)?;
    let mut connection = connections.create_with_id(&registered.connection, provider, target)?;
    if let Some(allowlist) = allowlist {
        connection = connections.set_allowlist(&connection.id, allowlist.clone())?;
    }
    Ok(Connected {
        connection,
        install_url: registered.install_url,
        install_url_group: registered.install_url_group,
    })
}

/// `adi-mono channels disconnect <id>` / `POST /api/channels/disconnect` — tell the router to
/// drop the connection (its Durable Object entry and the D1 routing row), then drop it here too,
/// and the provider's node token once nothing else on this node still uses that provider.
///
/// # Errors
/// [`Error::NotFound`] if `id` names no connection; otherwise whatever
/// [`RouterApi::disconnect`], [`Connections::remove`]/[`list`](Connections::list), or
/// [`token::remove`] return. A router failure aborts before any local state is touched — the
/// alternative (dropping it locally regardless) would desync this node's mirror from the
/// router's own record of whether the chat/workspace still reaches here.
pub fn disconnect(
    connections: &Connections,
    secrets: &adi_secrets::Secrets,
    router_url: &str,
    id: &str,
) -> Result<()> {
    let connection = connections
        .get(id)?
        .ok_or_else(|| Error::NotFound(id.to_string()))?;
    if let Some(token) = token::load(secrets, &connection.manifest.provider)? {
        RouterApi::new(router_url).disconnect(&token, id)?;
    }
    connections.remove(id)?;
    let provider_still_used = connections
        .list()?
        .iter()
        .any(|c| c.manifest.provider == connection.manifest.provider);
    if !provider_still_used {
        token::remove(secrets, &connection.manifest.provider)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::TcpListener;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-connect-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    /// A fake router that answers exactly one `/register`, same shape as `router_api`'s own test
    /// helper.
    fn fake_router(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.try_clone().unwrap().write_all(response.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    /// A fake router that answers every request it gets with a bare `200 {}` — for a test that
    /// calls `disconnect` (and therefore `POST /disconnect`) more than once against the same
    /// `router_url`.
    fn fake_router_ok_forever() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h == "\r\n" {
                        break;
                    }
                }
                stream
                    .try_clone()
                    .unwrap()
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn connect_mirrors_the_routers_connection_id_and_saves_the_token() {
        let cfg = scratch("connect");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg.clone());
        let base = fake_router(
            "{\"token\":\"tok_1\",\"connection\":\"conn_abc\",\"link_code\":\"code_1\",\
             \"install_url\":\"https://t.me/AdiBot?start=code_1\"}",
        );

        let connected = connect(
            &connections,
            &secrets,
            &cfg,
            Router {
                url: &base,
                admin_secret: None,
            },
            "telegram",
            Target::Agent {
                agent: "adi-agent".into(),
            },
            None,
        )
        .expect("connect");

        assert_eq!(connected.connection.id, "conn_abc", "the router's own id, verbatim");
        assert!(!connected.connection.manifest.linked);
        assert_eq!(
            connected.install_url,
            Some("https://t.me/AdiBot?start=code_1".to_string())
        );
        assert_eq!(
            token::load(&secrets, "telegram").unwrap(),
            Some("tok_1".to_string())
        );
        assert_eq!(
            connections.require("conn_abc").unwrap().id,
            "conn_abc",
            "readable back under the same id"
        );
    }

    /// The local mirror's allowlist comes from what `connect` itself was told, not the router's
    /// own stored copy — which it never reads back to apply (the router's copy is never enforced
    /// either; see §5/§8).
    #[test]
    fn connect_applies_the_given_allowlist_to_the_local_mirror() {
        let cfg = scratch("connect-allow");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg.clone());
        let base = fake_router("{\"token\":\"tok_1\",\"connection\":\"conn_open\",\"link_code\":\"code_1\"}");

        let connected = connect(
            &connections,
            &secrets,
            &cfg,
            Router {
                url: &base,
                admin_secret: None,
            },
            "telegram",
            Target::Agent {
                agent: "adi-agent".into(),
            },
            Some(&Allowlist::Open),
        )
        .expect("connect");

        assert_eq!(connected.connection.manifest.allowlist, Allowlist::Open);
        assert_eq!(
            connections.require("conn_open").unwrap().manifest.allowlist,
            Allowlist::Open
        );
    }

    #[test]
    fn disconnect_removes_the_connection_and_the_token_once_the_provider_is_unused() {
        let cfg = scratch("disconnect");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        let router_url = fake_router_ok_forever();
        token::save(&secrets, "telegram", "tok_1").unwrap();
        let a = connections
            .create("telegram", Target::Agent { agent: "a".into() })
            .unwrap();
        let b = connections
            .create("telegram", Target::Agent { agent: "b".into() })
            .unwrap();

        // Two connections share the provider — removing one leaves the token in place.
        disconnect(&connections, &secrets, &router_url, &a.id).unwrap();
        assert_eq!(token::load(&secrets, "telegram").unwrap(), Some("tok_1".into()));

        // Removing the last one drops the token too.
        disconnect(&connections, &secrets, &router_url, &b.id).unwrap();
        assert_eq!(token::load(&secrets, "telegram").unwrap(), None);

        assert!(
            matches!(
                disconnect(&connections, &secrets, &router_url, &a.id),
                Err(Error::NotFound(_))
            ),
            "already gone"
        );
    }

    /// A connection created without ever being `connect`ed through the router (no saved token
    /// for its provider) still disconnects cleanly — nothing to tell the router, so `disconnect`
    /// never calls it.
    #[test]
    fn disconnect_with_no_saved_token_skips_the_router_call() {
        let cfg = scratch("disconnect-no-token");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        let a = connections
            .create("telegram", Target::Agent { agent: "a".into() })
            .unwrap();

        disconnect(&connections, &secrets, "http://127.0.0.1:1", &a.id).unwrap();
        assert!(connections.get(&a.id).unwrap().is_none());
    }

    #[test]
    fn disconnect_aborts_and_keeps_local_state_when_the_router_call_fails() {
        let cfg = scratch("disconnect-router-fails");
        let connections = Connections::with_config(cfg.clone());
        let secrets = adi_secrets::Secrets::with_config(cfg);
        token::save(&secrets, "telegram", "tok_1").unwrap();
        let a = connections
            .create("telegram", Target::Agent { agent: "a".into() })
            .unwrap();

        // Nothing listening on this port -- the request can't even be sent.
        let err = disconnect(&connections, &secrets, "http://127.0.0.1:1", &a.id).unwrap_err();
        assert!(matches!(err, Error::Router(_) | Error::Http(_)));
        assert!(connections.get(&a.id).unwrap().is_some(), "local state untouched");
        assert_eq!(token::load(&secrets, "telegram").unwrap(), Some("tok_1".into()));
    }
}
