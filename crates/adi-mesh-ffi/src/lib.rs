//! The C surface Swift links against. See [`viewer`] for the runtime and the design behind it.
//!
//! **Every function returns one JSON string**, allocated here and freed by [`adi_mesh_free`]. That
//! is a deliberate choice over a struct-per-call ABI: the bridging header stays four lines, adding
//! a field to a reply never breaks the Swift side's layout, and there is exactly one ownership rule
//! to get right instead of one per type. The shapes are:
//!
//! ```json
//! {"ok": true,  "value": <anything>}
//! {"ok": false, "error": "a sentence for a human"}
//! ```
//!
//! Errors are sentences, not codes, for the same reason a join refusal is (`docs/fleet.md` §8):
//! the things that go wrong here — not paired, no relay yet, node asleep — have different fixes and
//! the person reading them is holding the phone.
//!
//! ## Threading
//!
//! [`adi_mesh_start`] must be called once before anything else and stores a process-global
//! [`Viewer`]. Every other entry point is safe to call from any thread; they take a read lock on
//! that global and, where they must touch the network, block on the viewer's own runtime. None of
//! them blocks for longer than a store read except [`adi_mesh_open`], which binds a socket.

// The workspace warns on `unsafe_code` and every other crate earns that by allowing it at the one
// line that needs it. This crate is the exception in kind, not in discipline: it *is* the FFI
// boundary, so `no_mangle`, `extern "C"`, and raw-pointer arguments are its entire subject matter,
// and twenty site-level allows would say less than this paragraph. Everything unsafe here is
// confined to the four shapes below — borrowing a C string, handing one out, taking one back, and
// setting `$HOME` before the runtime starts — each documented at its definition with the condition
// the caller must hold.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char};
use std::sync::{OnceLock, RwLock, PoisonError};

use serde::Serialize;

pub mod viewer;

use viewer::Viewer;

/// The one viewer per process. `RwLock<Option<_>>` rather than `OnceLock<_>` so a shutdown can put
/// it back — an iOS app that is terminated and relaunched in the same process image would otherwise
/// find a viewer holding a closed endpoint.
fn slot() -> &'static RwLock<Option<Viewer>> {
    static SLOT: OnceLock<RwLock<Option<Viewer>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// A reply, before it becomes a C string.
#[derive(Serialize)]
#[serde(untagged)]
enum Reply<T> {
    Ok { ok: bool, value: T },
    Err { ok: bool, error: String },
}

/// Turn a result into the JSON string the caller owns.
fn reply<T: Serialize>(result: anyhow::Result<T>) -> *mut c_char {
    let json = match result {
        Ok(value) => serde_json::to_string(&Reply::Ok { ok: true, value }),
        // `{:#}` prints the whole `anyhow` chain on one line — the context a caller added ("no node
        // called…", "binding a local port for…") is the half that tells a human what to do.
        Err(e) => serde_json::to_string(&Reply::<T>::Err {
            ok: false,
            error: format!("{e:#}"),
        }),
    };
    let json = json.unwrap_or_else(|e| format!(r#"{{"ok":false,"error":"encoding failed: {e}"}}"#));
    // `unwrap_or_default` covers the impossible interior NUL: serde_json never emits one.
    CString::new(json).unwrap_or_default().into_raw()
}

/// Run `f` against the started viewer, or fail with a sentence saying it was not started.
fn with_viewer<T: Serialize>(f: impl FnOnce(&Viewer) -> anyhow::Result<T>) -> *mut c_char {
    let guard = slot().read().unwrap_or_else(PoisonError::into_inner);
    match guard.as_ref() {
        Some(viewer) => reply(f(viewer)),
        None => reply::<T>(Err(anyhow::anyhow!(
            "the mesh has not been started — call adi_mesh_start first"
        ))),
    }
}

/// Borrow a C string argument as `&str`, or `None` for null / non-UTF-8.
///
/// # Safety
/// `raw` must be null or a valid NUL-terminated C string that stays alive for the call.
unsafe fn borrow<'a>(raw: *const c_char) -> Option<&'a str> {
    if raw.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(raw) }.to_str().ok()
}

/// Start the mesh. `home` may be null to use the process's own `$HOME`.
///
/// Returns `{"key": "<this phone's endpoint id>"}`. Calling it twice is not an error: the second
/// call returns the running viewer's key, because a UI that retries on a slow launch should not
/// end up with two endpoints on one identity racing for the same relay session.
///
/// # Safety
/// `home` must be null or a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn adi_mesh_start(home: *const c_char) -> *mut c_char {
    let home = unsafe { borrow(home) };
    let mut guard = slot().write().unwrap_or_else(PoisonError::into_inner);
    if let Some(running) = guard.as_ref() {
        return reply(Ok(serde_json::json!({ "key": running.key() })));
    }
    match Viewer::start(home) {
        Ok(viewer) => {
            let key = viewer.key();
            *guard = Some(viewer);
            reply(Ok(serde_json::json!({ "key": key })))
        }
        Err(e) => reply::<()>(Err(e)),
    }
}

/// Stop the mesh and release the endpoint. Safe to call when it was never started.
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_stop() -> *mut c_char {
    let taken = slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(viewer) = taken {
        viewer.stop();
    }
    reply(Ok(serde_json::json!({ "stopped": true })))
}

/// This phone's identity and whether a node could reach it yet.
///
/// Returns `{"key": "…", "ready": <bool>}`. `ready` is false until the relay session is up, which
/// is the honest answer to "can I pair right now?" — an invite minted before then names an endpoint
/// that only the local network can dial.
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_status() -> *mut c_char {
    with_viewer(|viewer| {
        Ok(serde_json::json!({
            "key": viewer.key(),
            "ready": viewer.ticket().is_some(),
        }))
    })
}

/// Mint a single-use invite for a node to spend with `adi-mono mesh join <token>`.
///
/// Returns `{"token": "adi-invite:…"}`.
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_invite() -> *mut c_char {
    with_viewer(|viewer| Ok(serde_json::json!({ "token": viewer.invite()? })))
}

/// Every paired node, as an array of [`viewer::NodeInfo`].
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_nodes() -> *mut c_char {
    with_viewer(Viewer::nodes)
}

/// The loopback port serving `service` on `node`, binding a listener on first ask.
///
/// Returns `{"port": <u16>}` — the port a `WKWebView` then loads `http://127.0.0.1:<port>/` from.
///
/// # Safety
/// `node` and `service` must be valid NUL-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn adi_mesh_open(
    node: *const c_char,
    service: *const c_char,
) -> *mut c_char {
    let (node, service) = unsafe { (borrow(node), borrow(service)) };
    with_viewer(|viewer| {
        let node = node.context_missing("node")?;
        let service = service.context_missing("service")?;
        Ok(serde_json::json!({ "port": viewer.open(node, service)? }))
    })
}

/// Drain the pairings that completed since the last call, as an array of [`viewer::Paired`].
///
/// **Each pairing is returned exactly once**, and it carries the plaintext password. The caller is
/// expected to put it in the Keychain immediately; nothing here writes it to disk (§8).
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_take_pairings() -> *mut c_char {
    with_viewer(|viewer| Ok(viewer.take_pairings()))
}

/// Unpair `node`. Returns `{"removed": <bool>}`.
///
/// # Safety
/// `node` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn adi_mesh_forget(node: *const c_char) -> *mut c_char {
    let node = unsafe { borrow(node) };
    with_viewer(|viewer| {
        let node = node.context_missing("node")?;
        Ok(serde_json::json!({ "removed": viewer.forget(node)? }))
    })
}

/// Retire pooled connections after the app returns to the foreground. See [`Viewer::resume`].
#[unsafe(no_mangle)]
pub extern "C" fn adi_mesh_resume() -> *mut c_char {
    with_viewer(|viewer| {
        viewer.resume();
        Ok(serde_json::json!({ "resumed": true }))
    })
}

/// Free a string returned by any function here. Passing null is a no-op.
///
/// # Safety
/// `raw` must be null or a pointer this library returned and that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn adi_mesh_free(raw: *mut c_char) {
    if !raw.is_null() {
        drop(unsafe { CString::from_raw(raw) });
    }
}

/// Turn a missing/invalid string argument into the same kind of sentence every other failure is.
trait MissingArg<T> {
    fn context_missing(self, what: &str) -> anyhow::Result<T>;
}

impl<T> MissingArg<T> for Option<T> {
    fn context_missing(self, what: &str) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("the {what} argument is missing or not valid UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading a reply back the way Swift does, so the shape stays what the header promises.
    fn read(raw: *mut c_char) -> serde_json::Value {
        let json = unsafe { CStr::from_ptr(raw) }.to_str().expect("utf-8").to_string();
        unsafe { adi_mesh_free(raw) };
        serde_json::from_str(&json).expect("valid json")
    }

    #[test]
    fn every_entry_point_answers_before_the_mesh_is_started() {
        // The contract the Swift side leans on: a call made too early is a readable failure, not a
        // crash and not a hang. `start` is excluded — it is the one that changes the state.
        for reply in [
            adi_mesh_status(),
            adi_mesh_invite(),
            adi_mesh_nodes(),
            adi_mesh_take_pairings(),
            adi_mesh_resume(),
        ] {
            let value = read(reply);
            assert_eq!(value["ok"], false);
            assert!(
                value["error"].as_str().expect("an error sentence").contains("adi_mesh_start"),
                "the error should name the fix, got {value}"
            );
        }
    }

    #[test]
    fn a_null_argument_is_an_error_and_not_a_crash() {
        let value = read(unsafe { adi_mesh_open(std::ptr::null(), std::ptr::null()) });
        assert_eq!(value["ok"], false);
    }

    #[test]
    fn freeing_null_is_a_no_op() {
        unsafe { adi_mesh_free(std::ptr::null_mut()) };
    }

    #[test]
    fn stopping_a_mesh_that_never_started_succeeds() {
        let value = read(adi_mesh_stop());
        assert_eq!(value["ok"], true);
    }
}
