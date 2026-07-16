//! Running `wasm:*` agents — the adi-workforce engine slice.
//!
//! A wasm agent's manifest points at a compiled WASM component (TS →
//! esbuild → jco) via `extra.wasm`; running it is a synchronous one-shot
//! dispatch: the engine installs the component under the `workforce`
//! module dir, instantiates it, runs `main()` to collect subscriptions,
//! and delivers one message into the chosen handler. Loops the agent
//! starts (`sdk.loop(...).run()`) execute inside the dispatch with the
//! engine's bundled tools/runners.

use crate::agent::Agent;
use crate::error::{Error, Result};

/// What a completed wasm dispatch did — re-exported shape from the engine.
pub use adi_workforce::DispatchOutcome;

/// Whether this agent runs through the wasm engine (backend `wasm:*`).
#[must_use]
pub fn is_wasm(agent: &Agent) -> bool {
    agent.manifest.executor() == "wasm"
}

/// Dispatch `message` into `agent`'s wasm component.
///
/// `workforce_dir` is where employees are installed and their logs live
/// (the `workforce` config module dir). `handler` picks the subscription;
/// `None` falls back to the agent's first subscription.
///
/// # Errors
/// [`Error::Launch`] when the manifest has no `extra.wasm` path or the
/// engine fails to load/dispatch.
pub fn dispatch(
    agent: &Agent,
    workforce_dir: &std::path::Path,
    handler: Option<&str>,
    message: &str,
) -> Result<DispatchOutcome> {
    let wasm_path = agent
        .manifest
        .extra
        .get("wasm")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            Error::Launch(format!(
                "agent {} has backend {:?} but no `extra.wasm` path to a compiled component \
                 (save it with --extra wasm=/path/to/agent.wasm)",
                agent.name, agent.manifest.backend
            ))
        })?;

    let core = adi_workforce::bundled::core();
    adi_workforce::dispatch_message(
        core,
        workforce_dir,
        std::path::Path::new(wasm_path),
        handler,
        message,
    )
    .map_err(|e| Error::Launch(e.message))
}
