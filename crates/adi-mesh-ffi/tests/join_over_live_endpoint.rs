//! That a viewer can spend an invite **over the endpoint it already has bound**.
//!
//! This is the one thing about `Viewer::join` that is not obvious from reading it. The app holds a
//! live endpoint on its identity for its whole run; `join::join` binds a *second* one on the same
//! secret key, and two endpoints sharing a key race for the same relay session. So the viewer
//! calls `join::join_on` with its own endpoint instead — and the question this answers is whether
//! an endpoint that registered only the join ALPN *for accepting* can still dial out on it.
//! (It can: `connect` names the ALPN per connection. But "it should" is not a test.)
//!
//! Ignored, because it needs a real peer and a real network — it is not a unit test and must not
//! run in CI. Run it by hand against a machine whose mesh is up:
//!
//! ```text
//! TOKEN=$(adi-mono mesh invite --no-qr --json | jq -r .token) \
//!     cargo test -p adi-mesh-ffi --test join_over_live_endpoint -- --ignored --nocapture
//! ```
//!
//! It pairs for real, so it leaves a node in the minting machine's fleet. Clean up afterwards:
//! `adi-mono mesh unpair <the petname it printed>`.

use adi_mesh_ffi::viewer::Viewer;

#[test]
#[ignore = "needs a live minting peer; see the module comment"]
fn a_viewer_spends_an_invite_over_its_own_endpoint() {
    let token =
        std::env::var("TOKEN").expect("set TOKEN to an `adi-invite:…` from `adi-mono mesh invite`");

    // Its own store, so the pairing this writes cannot land in the developer's real fleet.
    let home = tempfile::tempdir().expect("a temp home");
    let viewer = Viewer::start(home.path().to_str()).expect("the viewer starts");

    // The invite names the minter's relay address, so this dials out over the relay the viewer is
    // already sitting on. Without a session it would still find a direct path on the same LAN,
    // which is why this is not gated on `ticket()` being up.
    let paired = viewer
        .join(&token)
        .expect("spending the invite over the live endpoint");

    println!(
        "paired: petname={} username={} password={} chars",
        paired.petname,
        paired.username,
        paired.password.len()
    );

    assert!(!paired.petname.is_empty(), "a pairing must yield a petname");
    assert!(
        !paired.password.is_empty(),
        "the plaintext password crosses the FFI exactly here — an empty one means the Keychain \
         would be given nothing and every later request to this node would 401"
    );

    // The registry the app reads is the same one the pairing wrote, so the node must be listed.
    let nodes = viewer.nodes().expect("reading the fleet back");
    assert!(
        nodes.iter().any(|node| node.petname == paired.petname),
        "a node paired this way must appear in nodes(), or the fleet list would stay empty after \
         a successful pairing: {nodes:#?}"
    );
}
