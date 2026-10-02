//! `ChannelMessage` — the provider-neutral shape every adapter on the router normalizes *into*
//! (`docs/channels.md` §3), and the only shape anything past the adapter boundary (this crate,
//! and whatever a user's own trigger reads off `adi.channels.message`) ever reads.
//!
//! Versioned from day one (`v: 1`): the router, this crate, and a trigger a user wrote against
//! the shape deploy on three different cadences, so "add a field" is the only change v1 ever
//! promises not to break. Every reader here ignores fields it doesn't recognize (`serde`'s
//! default, since nothing below is `deny_unknown_fields`) and the version is read but not yet
//! branched on — there is only one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only value [`ChannelMessage::v`] takes today.
pub const VERSION: u8 = 1;

/// Who said it, in the provider's own vocabulary — stable across redelivery and across that
/// sender's other messages, but never anything past an id and a display name (`docs/channels.md`
/// §8: user PII beyond this never flows through the router at all).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Sender {
    /// The provider's own user id.
    pub id: String,
    /// A display name or handle, for the [`Marker::From`](adi_agents::Marker::From) a dispatched
    /// message is stamped with.
    pub name: String,
}

/// What kind of thing an [`Attachment`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
}

/// A file or image riding alongside a message. `url` is a short-lived link the node fetches
/// itself — never proxied through the router or riding the WebSocket as bytes (`docs/channels.md`
/// §2: this socket carries control and event traffic only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One inbound message, normalized by a router adapter out of whatever shape the service sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelMessage {
    /// Schema version — see the module doc.
    pub v: u8,
    /// Dedup key: stable across redelivery, unique per `(provider, id)`. This crate's
    /// [`client`](crate::client) keeps the last delivered ids and drops a repeat before it ever
    /// reaches [`dispatch`](crate::dispatch).
    pub id: String,
    /// `"telegram"`, `"slack"`, …
    pub provider: String,
    /// The connection this message belongs to, minted at link time (`docs/channels.md` §5) — which
    /// agent/trigger/app-route target it dispatches to.
    pub connection: String,
    /// The provider's own thread/chat identity, opaque past the adapter: a Telegram chat id, or a
    /// Slack `channel` / `channel:thread_ts`. [`connection::Connection::threads`] maps this to an
    /// ADI conversation id.
    pub thread: String,
    pub sender: Sender,
    /// Already extracted: captions, slash-command args, and the like are folded in by the adapter.
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// This message's id, if it quoted/replied to an earlier one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// What the provider actually called it (`"message"`, `"edited_message"`, `"app_mention"`, …)
    /// — for an adapter-specific trigger that needs more than the normalized shape; never parsed
    /// past the adapter itself.
    pub raw_kind: String,
    /// Epoch ms, the router's own clock.
    pub received_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChannelMessage {
        ChannelMessage {
            v: VERSION,
            id: "evt_1".into(),
            provider: "telegram".into(),
            connection: "conn_1".into(),
            thread: "12345".into(),
            sender: Sender {
                id: "u1".into(),
                name: "Igor".into(),
            },
            text: "hello".into(),
            attachments: Vec::new(),
            reply_to: None,
            raw_kind: "message".into(),
            received_at: 1_700_000_000_000,
        }
    }

    /// A field this version doesn't know about must not break deserialization — the whole point
    /// of versioning from day one (see the module doc).
    #[test]
    fn an_unknown_field_is_ignored_not_refused() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value["from_the_future"] = serde_json::json!("whatever a later v1 adds");
        let round_tripped: ChannelMessage = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, sample());
    }

    #[test]
    fn empty_attachments_and_absent_reply_to_are_omitted() {
        let json = serde_json::to_value(sample()).unwrap();
        assert!(json.get("attachments").is_none());
        assert!(json.get("reply_to").is_none());
    }
}
