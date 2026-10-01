//! adi-channels — the node side of Channels (`docs/channels.md`): the outbound WebSocket to the
//! channel-router, the `adi.channels.message` event, the connection store, dispatch to an agent
//! with thread->conversation mapping, the automatic post-back of a run's answer, and the
//! `channel-reply` tool.
//!
//! One ADI agent, reachable from Telegram/Slack/…, on the node side of the split the design
//! document draws: the router holds bot credentials and brokers the standing relationship between
//! a service's chat/workspace and this node; this crate is everything past that boundary — the
//! socket, the connection a link created, and getting a message to (and an answer back from) the
//! agent it names.

pub mod client;
pub mod connect;
pub mod connection;
pub mod dispatch;
pub mod error;
pub mod events;
pub mod finished;
pub mod message;
pub mod node_id;
pub mod node_port;
pub mod protocol;
pub mod reply;
pub mod router_api;
pub mod token;
pub mod tool;
mod ws;

pub use client::RouterClient;
pub use connect::Connected;
pub use connection::{Allowlist, Connection, ConnectionManifest, Connections, Target};
pub use error::{Error, Result};
pub use events::{CHANNEL_MESSAGE, event_catalog, event_types};
pub use message::{Attachment, AttachmentKind, ChannelMessage, Sender, VERSION};
pub use router_api::{Registered, RouterApi};
