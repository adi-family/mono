//! The session as a runner sees it: an identity, whether it has run before, a private scratch slot,
//! and somewhere to put output.
//!
//! This is a **borrowed view over the store**, not an object with a life of its own. It must own no
//! cached state — the CLI, the daemon, and triggers all launch runs independently (see
//! [`crate::run`]'s note on why PID files exist), so anything held in memory here would be a second
//! truth that another process can invalidate between two calls.

use std::path::Path;

use crate::error::Result;

pub trait Session {
    /// This session's id — durable, and the key everything else about it is filed under.
    fn id(&self) -> &str;

    /// The agent this session belongs to.
    fn agent(&self) -> &str;

    /// Whether at least one turn has already run here.
    ///
    /// This is what makes "start" and "resume" the same verb: a runner reads it to decide whether to
    /// establish its engine session or continue the established one, so no caller ever has to hold
    /// that distinction.
    fn has_started(&self) -> bool;

    /// The runner's own scratch space for this session — runner-written, store-opaque, and never
    /// interpreted by anything but the runner that wrote it.
    ///
    /// A local-process runner keeps its pid here; a pty runner its session name; a runner whose
    /// engine lives behind an API keeps nothing at all, because the session id is already the key it
    /// needs. That last case is the whole reason this is a slot rather than a typed handle threaded
    /// through every method.
    fn state(&self) -> Option<serde_json::Value>;

    /// Replace the runner's scratch space.
    ///
    /// # Errors
    /// Returns store write errors.
    fn set_state(&self, value: serde_json::Value) -> Result<()>;

    /// Where this session's raw output goes.
    ///
    /// A real path, not a sink object: a spawned child needs a file descriptor to redirect stdout
    /// and stderr into, and no amount of storage abstraction changes that. A store that keeps its
    /// records elsewhere still has to spool through a local file here.
    fn log_path(&self) -> &Path;
}
