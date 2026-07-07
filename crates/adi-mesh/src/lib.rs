//! adi-mesh (library) — the peer mesh, usable both as the `adi-mesh` binary and, more
//! importantly, **in-process**: [`Daemon::start`] brings the host + client roles up on
//! internal tasks so a host process (the control-panel [`adi-app`](../adi-app)) can own the
//! mesh's lifecycle — start/stop it on demand, nothing left running once the host exits.
//!
//! The reusable state pieces — the typed [`config`], the stable [`identity`], the shareable
//! [`ticket`] — are public so a caller can inspect/edit mesh state without starting it.

pub mod config;
pub mod identity;
pub mod ticket;

mod client;
mod daemon;
mod host;
mod protocol;
mod tunnel;

pub use daemon::{Daemon, current_ticket};
