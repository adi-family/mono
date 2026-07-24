//! Project hooks and workspaces.
//!
//! A project can own working copies of its source — **workspaces** — and defines how they
//! come to be with **hooks**: plain shell scripts stored git-hooks style inside the project
//! directory at `.adi/hooks/<name>`. Creating the first workspace runs the `init` hook
//! (typically `git clone`); each additional one runs the `workspace` hook (typically
//! `git worktree add`); a *local* workspace just registers an existing directory. Extra
//! hook files can sit alongside and be run manually.
//!
//! Everything is rooted at a project directory passed in by the caller (mirroring
//! `adi_fs::Jail`), so the crate stays free of registry/config dependencies:
//!
//! ```text
//! <project>/
//! ├── .adi/hooks/<name>          # the hook scripts (browsable, editable as plain files)
//! ├── .adi/hooks/logs/<name>.log # one log per hook, truncated per run
//! ├── .adi/workspaces.toml       # the workspace registry
//! └── workspaces/<name>/         # default location for created workspaces
//! ```
//!
//! Runs are detached (`sh -c`, own process group, output to the log) — the same execution
//! shape as adi-triggers' fire — with an `[adi:hook-exit <code>]` marker appended so a
//! finished run's status is readable from the log alone.
//!
//! Each workspace can also host an interactive pty terminal rooted at its directory (the
//! [`terminal`] module), observed and driven like agent sessions.

mod error;
mod hook;
pub mod terminal;
mod workspace;

pub use error::{Error, Result};
pub use hook::{
    EXIT_MARKER_PREFIX, HOOK_INIT, HOOK_WORKSPACE, Hook, HookRun, HookRunStatus, Hooks,
    hook_template, is_lifecycle,
};
pub use workspace::{WORKSPACES_DIR, WorkspaceEntry, WorkspaceKind, WorkspaceStatus, Workspaces};
