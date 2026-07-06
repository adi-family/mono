//! adi-core — the command surface for the adi platform, shared by every frontend.
//! Clients trigger platform commands through this crate instead of owning
//! launchd/config/route logic. The `adi-mono` CLI is a thin argv adapter over this API.

mod commands;
pub mod dns;
pub mod launchd;
pub mod paths;
mod proc;
pub mod service;
pub mod status;

pub use commands::{Adi, Report};
pub use dns::Dns;
pub use service::{Action, Service, ServiceReport};

/// The CLI binary name — the single Rust-side source of truth for user-facing messages.
pub const BIN_NAME: &str = "adi-mono";
