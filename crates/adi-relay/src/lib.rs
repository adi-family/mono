//! Client half of the webhook relay's wire contract (ADI-MONO-220).
//!
//! Four things live here, used by both sides of the relay despite the crate's name: a node signs
//! with [`keys`]/[`sign`] and seals nothing (it only *opens*, via [`seal::open`]); the relay
//! verifies with [`sign::verify`] and seals with [`seal::seal`], and never calls
//! [`keys::load_or_create`] — the one function in this crate that touches `~/.adi/mono/`, so
//! depending on this crate never makes the self-hostable `adi-relay-server` assume that layout
//! exists.
//!
//! * [`keys`] — a node's own ed25519 signing key and X25519 sealing key, `load_or_create`d as two
//!   raw files, the same shape `adi-mesh::identity`/`devicekey` already use.
//! * [`openssh`] — rendering an ed25519 public key as the one-line `ssh-ed25519 AAAA…` format
//!   (and parsing it back, and an `authorized_keys` file of them), with no new dependency: the
//!   format is a 4-byte-length-prefixed pair of strings, not worth a crate for.
//! * [`sign`] — the canonical string every node→relay request signs, and both directions of
//!   it: `sign` (the node) and `verify` (the relay).
//! * [`seal`] — the sealed-box envelope at rest: `seal` (the relay, to a node's long-lived
//!   public key) and `open` (the node, with its own secret).

pub mod keys;
pub mod openssh;
pub mod seal;
pub mod sign;

pub use keys::RelayIdentity;
pub use openssh::Fingerprint;
pub use seal::Sealed;
pub use sign::{SignedRequest, VerifyError};
