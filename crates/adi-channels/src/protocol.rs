//! The wire protocol on the node<->router WebSocket (`docs/channels.md` §2). Every frame is a
//! flat JSON object with a `type` tag, kept small on purpose: this socket carries control and
//! event traffic only, never attachment bytes.

use serde::{Deserialize, Serialize};

use crate::message::ChannelMessage;

/// A frame the router sends to the node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RouterFrame {
    /// A normalized message, at-least-once until acked by `id`. Boxed: `Linked`/`Ping` are tiny,
    /// and clippy is right that leaving this unboxed would size every `RouterFrame` to the
    /// largest variant.
    Event {
        id: String,
        message: Box<ChannelMessage>,
    },
    /// A pending connection was just bound to a real routing key.
    Linked {
        connection: String,
        provider: String,
        routing_key: String,
    },
    /// Idle-socket heartbeat; answer with [`NodeFrame::Pong`].
    Ping,
}

/// A frame the node sends to the router.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeFrame {
    /// Acknowledge delivery of the [`RouterFrame::Event`] with this `id`, so the router's queue
    /// drops it.
    Ack { id: String },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Sender, VERSION};

    fn message() -> ChannelMessage {
        ChannelMessage {
            v: VERSION,
            id: "evt_1".into(),
            provider: "telegram".into(),
            connection: "c1".into(),
            thread: "t1".into(),
            sender: Sender {
                id: "u1".into(),
                name: "Igor".into(),
            },
            text: "hi".into(),
            attachments: Vec::new(),
            reply_to: None,
            raw_kind: "message".into(),
            received_at: 1,
        }
    }

    #[test]
    #[allow(clippy::similar_names)] // ping/pong/ack name exactly the frames being tested
    fn frames_tag_and_round_trip_the_way_the_spec_shows_them() {
        let event = RouterFrame::Event {
            id: "evt_1".into(),
            message: Box::new(message()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["id"], "evt_1");
        assert_eq!(json["message"]["provider"], "telegram");
        let back: RouterFrame = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);

        let linked = RouterFrame::Linked {
            connection: "c1".into(),
            provider: "telegram".into(),
            routing_key: "123".into(),
        };
        let json = serde_json::to_value(&linked).unwrap();
        assert_eq!(json["type"], "linked");

        let ping = serde_json::to_value(RouterFrame::Ping).unwrap();
        assert_eq!(ping, serde_json::json!({"type": "ping"}));

        let ack = serde_json::to_value(NodeFrame::Ack { id: "evt_1".into() }).unwrap();
        assert_eq!(ack, serde_json::json!({"type": "ack", "id": "evt_1"}));

        let pong = serde_json::to_value(NodeFrame::Pong).unwrap();
        assert_eq!(pong, serde_json::json!({"type": "pong"}));
    }
}
