use crate::loop_run_context::LoopRunContext;

/// JSON-encoded trigger event payload.
///
/// The wire format is a JSON string whose shape matches the trigger's
/// declared `@llm.trigger.field` schema in `plugin.adi.tsp`. The host
/// forwards this string to the WASM dispatcher which hands it to the
/// user's trigger handler as `JSON.parse(data)` — so emitting anything
/// other than valid JSON will crash the handler.
///
/// Use [`event`] to build one safely from a serde type.
pub type TriggerMessage = String;
pub type TriggerSend<'a> = &'a dyn Fn(TriggerMessage);

/// Serialize a structured event to the wire format expected by [`TriggerSend`].
///
/// Panics only if the value contains a non-string map key, which
/// `serde_json::json!({...})` never produces.
pub fn event<T: serde::Serialize>(value: &T) -> TriggerMessage {
    serde_json::to_string(value).expect("trigger event serialization")
}

pub trait Trigger: Send + Sync {
    fn name(&self) -> String;

    /// Blocking. The runtime wraps this in a managed thread.
    /// Restarts automatically on panic or return.
    ///
    /// Call `send` with a JSON string matching the trigger's declared
    /// field schema. Prefer [`event`] to build the payload from a
    /// serde type rather than hand-rolling JSON.
    fn watch(&self, ctx: &LoopRunContext, send: TriggerSend<'_>);
}
