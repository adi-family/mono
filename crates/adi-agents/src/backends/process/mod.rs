//! The `process` executor's engine commands: a vendor CLI (`claude --print` / `codex exec`) run
//! headless as a detached subprocess.
//!
//! Only the argv builders live here now. Everything that *ran* them — the launch, the run history,
//! the stop and delete verbs, and the `Backend` match that picked between the two engines — was a
//! wrapper around [`super::detached`], and is now [`crate::runner::detached::DetachedRunner`], which
//! decodes its own arguments and calls these directly.

pub(crate) mod claude;
pub(crate) mod codex;
