//! adi-webapp-api — the contract between the adi webapp and its host.
//!
//! [`types`] holds the serde DTOs for every `/api/*` payload. It has no platform
//! dependencies and compiles for `wasm32-unknown-unknown`, so the Leptos frontend
//! ([`adi-webapp`](../adi-webapp)) and the server ([`adi-app`](../adi-app)) share exactly
//! the same structs — the wire format can't drift between them.
//!
//! [`handlers`] (behind the `server` feature) is the native-only backend: the `/api/*`
//! logic over the live [`adi_ports_manager`] port registry. The frontend never enables
//! that feature, so it links none of the server code.

pub mod types;

#[cfg(feature = "server")]
pub mod handlers;
