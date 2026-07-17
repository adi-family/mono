//! Executor-specific agent backends.
//!
//! Each executor owns a directory so its shared lifecycle and individual engines can evolve
//! independently. The public dispatch surface remains in `crate::run`.

pub(crate) mod process;
pub(crate) mod tmux;
