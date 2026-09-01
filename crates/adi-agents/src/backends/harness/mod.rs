//! The `harness` executor: an agentic loop ADI drives itself, rather than a vendor CLI that owns
//! its own loop.
//!
//! A harness run is a *conversation* you can answer, not a fire-and-forget `--print` run: the first
//! turn is the initial task, and each reply spawns another detached child that continues the same
//! thread and prints its answer. One turn runs at a time, so a message sent mid-answer waits in the
//! conversation's queue — until the turn lands for `claude-sdk`, whose CLI owns its own loop and
//! cannot be told anything once it has started, and only until the next round for `adi`, which
//! drives its loop here and takes what is waiting between rounds. The conversation machinery
//! (transcript, queue, turn spawning, settling) lives in [`conversation`]; each engine only supplies
//! the per-turn command and how it continues.
//!
//! - `harness:claude-sdk` runs the `claude` CLI headless (a turn-capped, adi-scoped `--print` turn),
//!   establishing a session id on the first turn (`--session-id`) and resuming it on each reply
//!   (`--resume`) so the CLI reconstructs the full history. Spawned detached through the shared
//!   [`super::detached`] machinery, under its own `harness/` runtime subdir.
//! - `harness:adi` is ADI's own loop over a chosen model provider (see [`adi_loop`]): each turn
//!   re-enters `adi-mono harness-turn`, replays the transcript, calls the provider's chat API, and
//!   runs the [`tools`] it asks for until it answers. Runnable as soon as a provider is named.

//! Only the `adi` loop's turn entry point stays in this module. Everything else here was the
//! conversation *lifecycle* — launch, reply, advance, stop, delete, transcript — wrapped around
//! [`super::detached`] and dispatched by a `Backend` match per verb. That is now the session store
//! plus [`crate::runner::detached::DetachedRunner`]. What could not move is this: the `adi` loop is
//! not a child a runner spawns and watches, it *is* that child, so it keeps a direct call.

pub(crate) mod adi_loop;
pub(crate) mod claude_sdk;
// `pub(crate)` for one helper: an await's check is a shell command with a deadline, which is the
// same thing `Bash` already runs, so [`crate::awaits`] borrows the timed wait rather than keeping a
// second copy of it.
pub(crate) mod tools;

use std::path::Path;

use crate::StoredAgent;
use crate::error::Result;

/// Run one `adi` conversation turn: read the transcript, drive the provider and its tools, and
/// return the answer text. Used by the `adi-mono harness-turn` child that an `adi` turn spawns — a
/// plain sync process, since the blocking provider client must not run inside an async runtime.
///
/// `sink` receives the turn's events as they happen (that child's stdout in production), which is
/// what puts a running tool on screen before the turn settles.
///
/// # Errors
/// Returns argument, provider-configuration, or HTTP/decoding errors.
pub fn run_adi_turn(
    agent: &StoredAgent,
    sessions_dir: &Path,
    conv_id: &str,
    sink: crate::backends::adi_events::Sink<'_>,
) -> Result<String> {
    adi_loop::run_turn(agent, sessions_dir, conv_id, sink)
}
