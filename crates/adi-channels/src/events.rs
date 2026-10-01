//! `adi.channels.message` — a [`ChannelMessage`] (§3 of `docs/channels.md`), emitted by
//! [`crate::client::RouterClient`] the moment an `event` frame off the router is acked.
//!
//! **Why the whole-platform catalog is assembled here and not in `adi-agents` any more.**
//! `adi_agents::events`'s own doc explains why *it* assembles the task+agent catalog: it is the
//! lowest crate that can see both producers' payload types. This crate depends on `adi-agents` (to
//! dispatch a message to one), so it now sits one layer above — and is, in turn, the lowest crate
//! that can see every producer, task + agent + channel. [`event_catalog`] here is what `adi-core`
//! and the webapp API read; `adi_agents::event_catalog` still exists and is still correct, it is
//! simply no longer the *whole* catalog.

use adi_events::EventType;
use schemars::JsonSchema;
use serde_json::Value;

use crate::message::{Attachment, AttachmentKind, ChannelMessage, Sender, VERSION};

/// The event name — also [`crate::message::ChannelMessage`]'s home on the bus.
pub const CHANNEL_MESSAGE: &str = "adi.channels.message";

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

fn example() -> ChannelMessage {
    ChannelMessage {
        v: VERSION,
        id: "telegram:7001234567890".into(),
        provider: "telegram".into(),
        connection: "1750000000000-0001".into(),
        thread: "394857201".into(),
        sender: Sender {
            id: "394857201".into(),
            name: "Igor".into(),
        },
        text: "status?".into(),
        attachments: vec![Attachment {
            kind: AttachmentKind::Image,
            url: "https://api.telegram.org/file/bot<token>/photos/file_1.jpg".into(),
            name: None,
        }],
        reply_to: None,
        raw_kind: "message".into(),
        received_at: 1_750_000_000_000,
    }
}

/// This crate's slice of the catalog: just [`CHANNEL_MESSAGE`].
#[must_use]
pub fn event_types() -> Vec<EventType> {
    vec![EventType::of(
        CHANNEL_MESSAGE,
        "An inbound Telegram/Slack message was normalized and forwarded by the channel-router.",
        schema::<ChannelMessage>(),
        &example(),
    )]
}

/// The whole platform event catalog: task events, then agent events, then this one — see the
/// module doc for why the assembly moved here.
#[must_use]
pub fn event_catalog() -> Vec<EventType> {
    let mut all = adi_agents::event_catalog();
    all.extend(event_types());
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_includes_channel_message_alongside_everything_below_it() {
        let catalog = event_catalog();
        let names: Vec<&str> = catalog.iter().map(|e| e.name).collect();
        assert!(names.contains(&CHANNEL_MESSAGE));
        assert!(names.contains(&"adi.tasks.created"));
        assert!(names.contains(&"adi.agents.run.finished"));

        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "event names must be unique");
    }

    #[test]
    fn channel_message_entry_has_a_valid_name_schema_and_example() {
        let entry = event_types().into_iter().next().unwrap();
        assert_eq!(entry.name, CHANNEL_MESSAGE);
        assert!(adi_events::validate_name(entry.name).is_ok());
        assert!(entry.schema.is_object());
        assert!(entry.example.is_object());
        assert_eq!(entry.example["provider"], "telegram");
    }
}
