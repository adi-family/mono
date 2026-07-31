//! adi-hive (library) — the front door's routing brain, usable without the daemon around it.
//!
//! The binary beside this file is a thin shell: it binds sockets, supervises runners, and reloads
//! the config on a tick. Everything that *decides* — how a `hive.yaml` becomes a set of routes
//! ([`config`]), and which upstream a `(Host, path)` pair belongs to ([`proxy::Router`]) — lives
//! here, because the front door is no longer the only thing that needs to know.
//!
//! The mesh gateway is the reason. When a request arrives from a remote node, something on this
//! machine has to resolve a service label against the same table the local front door serves from.
//! A second implementation of that lookup would be a second set of rules to keep in step, and the
//! first time they drifted a host would resolve differently depending on which door you came in.
//! So the table is a library type both of them construct from the same config.
//!
//! [`notfound`] and [`tls`] come along for the same reason: a gateway that renders a different
//! "unreachable" page, or mints a certificate covering a different set of names, is the same
//! divergence in a different coat.

pub mod config;
pub mod notfound;
pub mod proxy;
pub mod runner;
pub mod status;
pub mod tls;
