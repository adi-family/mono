//! `/api/channels/*` (`docs/channels.md` §7) — the node's own small API over
//! [`adi_channels::Connections`], which both `/settings/channels` and `adi-mono channels` (both
//! ADI-MONO-122, not this crate) call. Connecting and disconnecting also talk to the router
//! (`POST /register`, `POST /disconnect`), so those two take the extra context
//! (`adi_secrets::Secrets`, the admin secret or node token, this node's `adi_config::Config`, and
//! the router's base URL) the others don't need.

use adi_channels::connection::{Allowlist, Connections, Target};
use adi_channels::error::Error as ChannelsError;
use adi_secrets::Secrets;

use crate::types::{
    AllowChannel, ChannelAllowlistDto, ChannelConnected, ChannelConnectionDto,
    ChannelProviderStatus, ChannelRef, ChannelReplyRequest, ChannelTargetDto, ChannelsState,
    ConnectChannel, PauseChannel, RouteChannel,
};

use super::response::{FromBody, Response, error, mutate, ok_json};

/// `GET /api/channels` — every connection this node holds. Every mutation below returns a fresh
/// one, so the client refreshes from one round-trip.
#[must_use]
pub fn channels(store: &Connections) -> Response {
    match store.list() {
        Ok(list) => ok_json(&ChannelsState {
            connections: list.into_iter().map(connection_dto).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `GET /api/channels/status` — the router-connection pill's own data: whether each provider's
/// socket is actually open right now. `connected` is computed by `adi-channelsd` from its
/// live clients and handed in already built.
#[must_use]
pub fn channel_status(connected: std::collections::BTreeMap<String, bool>) -> Response {
    ok_json(&ChannelProviderStatus { connected })
}

/// `GET /api/channels/<id>` — one connection, for the connect flow's own poll on `linked`
/// (`docs/channels.md` §7: "the node is just waiting on its own WebSocket for a `linked` frame").
#[must_use]
pub fn channel(store: &Connections, id: &str) -> Response {
    match store.get(id) {
        Ok(Some(connection)) => ok_json(&connection_dto(connection)),
        Ok(None) => error(404, &format!("no such connection: {id}")),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/channels/connect` — register this node for the provider (minting/reusing its node
/// token) and mirror the fresh, unlinked connection the router created in the same call.
/// `router_url`/`admin_secret` name where and how to reach the router — `admin_secret` is `None`
/// for the ordinary node (ADI-MONO-125: registration is open, not admin-gated, any more). The
/// allowlist starts at the router's own default (owner-only) — `docs/channels.md` §7 has "who may
/// talk" as a separate step (`adi-mono channels allow`), not part of connect's own request.
#[must_use]
pub fn connect_channel(
    connections: &Connections,
    secrets: &Secrets,
    config: &adi_config::Config,
    router_url: &str,
    admin_secret: Option<&str>,
    body: &[u8],
) -> Response {
    let req = require!(body, ConnectChannel);
    match adi_channels::connect::connect(
        connections,
        secrets,
        config,
        adi_channels::connect::Router {
            url: router_url,
            admin_secret,
        },
        req.provider.trim(),
        target_from_dto(req.target),
        None,
    ) {
        Ok(connected) => ok_json(&ChannelConnected {
            connection: connection_dto(connected.connection),
            install_url: connected.install_url.unwrap_or_default(),
            install_url_group: connected.install_url_group.unwrap_or_default(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/channels/route` — change a connection's target.
#[must_use]
pub fn route_channel(store: &Connections, body: &[u8]) -> Response {
    mutate(
        body,
        |req: RouteChannel| store.set_target(&req.id, target_from_dto(req.target)),
        || channels(store),
    )
}

/// `POST /api/channels/pause` — stop or resume delivery without disconnecting.
#[must_use]
pub fn pause_channel(store: &Connections, body: &[u8]) -> Response {
    mutate(
        body,
        |req: PauseChannel| store.set_paused(&req.id, req.paused),
        || channels(store),
    )
}

/// `POST /api/channels/allow` — change who may talk.
#[must_use]
pub fn allow_channel(store: &Connections, body: &[u8]) -> Response {
    mutate(
        body,
        |req: AllowChannel| store.set_allowlist(&req.id, allowlist_from_dto(req.allowlist)),
        || channels(store),
    )
}

/// `POST /api/channels/disconnect` — tell the router to drop the connection, then drop it here
/// too, and the provider's node token once nothing else on this node still uses it.
#[must_use]
pub fn disconnect_channel(
    connections: &Connections,
    secrets: &Secrets,
    router_url: &str,
    body: &[u8],
) -> Response {
    mutate(
        body,
        |req: ChannelRef| {
            adi_channels::connect::disconnect(connections, secrets, router_url, &req.id)
        },
        || channels(connections),
    )
}

/// `POST /api/channels/reply` — the `channel-reply` tool's own local endpoint: look up the
/// connection `run_id`'s thread map names, and post `text` back through it.
#[must_use]
pub fn reply_channel(
    connections: &Connections,
    secrets: &Secrets,
    router_url: &str,
    body: &[u8],
) -> Response {
    let req = require!(body, ChannelReplyRequest);
    match adi_channels::reply::handle(
        connections,
        secrets,
        router_url,
        &req.agent,
        &req.run_id,
        &req.text,
    ) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err(e) => Response::from(&e),
    }
}

fn connection_dto(connection: adi_channels::Connection) -> ChannelConnectionDto {
    ChannelConnectionDto {
        id: connection.id,
        provider: connection.manifest.provider,
        routing_key: connection.manifest.routing_key,
        target: target_dto(connection.manifest.target),
        allowlist: allowlist_dto(connection.manifest.allowlist),
        paused: connection.manifest.paused,
        linked: connection.manifest.linked,
        created_at: connection.manifest.created_at,
        updated_at: connection.manifest.updated_at,
    }
}

fn target_dto(target: Target) -> ChannelTargetDto {
    match target {
        Target::Agent { agent } => ChannelTargetDto::Agent { agent },
        Target::Trigger { trigger } => ChannelTargetDto::Trigger { trigger },
        Target::AppRoute { app, route } => ChannelTargetDto::AppRoute { app, route },
        // An older build reading a store a newer one wrote — see `Target::Unknown`'s own doc.
        // There is no "unknown" wire shape to answer with, so this reads as the tightest target
        // rather than inventing one; `adi_channels::dispatch` already skips it either way.
        Target::Unknown => ChannelTargetDto::Agent {
            agent: String::new(),
        },
    }
}

fn target_from_dto(target: ChannelTargetDto) -> Target {
    match target {
        ChannelTargetDto::Agent { agent } => Target::Agent { agent },
        ChannelTargetDto::Trigger { trigger } => Target::Trigger { trigger },
        ChannelTargetDto::AppRoute { app, route } => Target::AppRoute { app, route },
    }
}

fn allowlist_dto(allowlist: Allowlist) -> ChannelAllowlistDto {
    match allowlist {
        Allowlist::OwnerOnly => ChannelAllowlistDto::OwnerOnly,
        Allowlist::List { sender_ids } => ChannelAllowlistDto::List { sender_ids },
        Allowlist::Open => ChannelAllowlistDto::Open,
    }
}

fn allowlist_from_dto(allowlist: ChannelAllowlistDto) -> Allowlist {
    match allowlist {
        ChannelAllowlistDto::OwnerOnly => Allowlist::OwnerOnly,
        ChannelAllowlistDto::List { sender_ids } => Allowlist::List { sender_ids },
        ChannelAllowlistDto::Open => Allowlist::Open,
    }
}

impl FromBody for ConnectChannel {
    const EXPECTED: &'static str = "expected JSON body { \"provider\": \"…\", \"target\": {…} }";

    fn is_complete(&self) -> bool {
        !self.provider.trim().is_empty()
    }
}

impl FromBody for ChannelRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for RouteChannel {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\", \"target\": {…} }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for PauseChannel {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\", \"paused\": bool }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for AllowChannel {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\", \"allowlist\": {…} }";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for ChannelReplyRequest {
    const EXPECTED: &'static str = "expected JSON body { \"run_id\": \"…\", \"text\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.run_id.trim().is_empty() && !self.text.trim().is_empty()
    }
}

// Map a channels-store error to an HTTP status: a bad/missing id is 404, everything else this
// crate's own `Error` carries is a local store or network failure → 500.
impl From<&ChannelsError> for Response {
    fn from(e: &ChannelsError) -> Self {
        let status = match e {
            ChannelsError::NotFound(_) => 404,
            ChannelsError::InvalidId(_) => 400,
            ChannelsError::Config(_)
            | ChannelsError::Agents(_)
            | ChannelsError::Events(_)
            | ChannelsError::Secrets(_)
            | ChannelsError::Tools(_)
            | ChannelsError::Router(_)
            | ChannelsError::Http(_)
            | ChannelsError::Protocol(_) => 500,
        };
        error(status, &e.to_string())
    }
}
