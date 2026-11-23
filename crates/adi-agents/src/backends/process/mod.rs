//! The `process` executor's engine commands: a vendor CLI (`claude --print` / `codex` / `agy -p`)
//! run headless as a detached subprocess.
//!
//! Only the argv builders live here now. Everything that *ran* them — the launch, the run history,
//! the stop and delete verbs, and the `Backend` match that picked between the engines — was a
//! wrapper around [`super::detached`], and is now [`crate::runner::detached::DetachedRunner`], which
//! decodes its own arguments and calls these directly.

pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod codex_mcp;
