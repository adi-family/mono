//! adi-mesh (library) — the peer mesh, usable both as the `adi-mesh` binary and, more
//! importantly, **in-process**: [`Daemon::start`] brings the host + client roles up on
//! internal tasks so a host process (the control-panel [`adi-app`](../adi-app)) can own the
//! mesh's lifecycle — start/stop it on demand, nothing left running once the host exits.
//!
//! The reusable state pieces — the typed [`config`], the stable [`identity`], this machine's own
//! [`node`] name, the shareable [`ticket`], and the [`join`] handshake that turns one into a
//! pairing — are public so a caller can inspect/edit mesh state, or enrol a machine, without
//! starting the daemon. `adi-mono mesh` is exactly that caller.

pub mod auth;
pub mod config;
pub mod fleet;
pub mod gateway;
pub mod identity;
pub mod join;
pub mod node;
pub mod protocol;
pub mod ticket;

mod client;
mod daemon;
mod host;
mod tunnel;

pub use daemon::{Daemon, current_ticket};
