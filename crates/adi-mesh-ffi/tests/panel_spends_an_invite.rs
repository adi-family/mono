//! That the **control panel** can spend an invite: `POST /api/fleet/join`, end to end, against a
//! real minter that is not this machine's own mesh.
//!
//! The mirror of `join_over_live_endpoint.rs`. That one proves a viewer can dial out over the
//! endpoint it already holds; this one proves the panel does the same thing, over the endpoint its
//! in-process daemon holds, and answers with the credential the handshake minted. It is the half
//! of `docs/fleet.md` §8 that used to need a terminal, so the thing worth proving is that it needs
//! one no longer.
//!
//! The minter here is a [`Viewer`] with a temp `$HOME` — a second machine on this box, with its own
//! identity and its own registry, discarded when the test ends. Only the panel's side of the
//! pairing is real, and the test unpairs it again.
//!
//! Ignored, because it needs a running panel and a real relay — it is not a unit test and must not
//! run in CI. Run it against a panel whose mesh is up:
//!
//! ```text
//! cargo test -p adi-mesh-ffi --test panel_spends_an_invite -- --ignored --nocapture
//! ```
//!
//! `PANEL` overrides the panel it talks to (default `http://127.0.0.1:8000`). It cleans up after
//! itself with `adi-mono mesh unpair`; if it fails between pairing and cleanup, the petname it
//! printed is still filed here and unpairing it by hand is the whole of the repair.

use std::process::Command;

use adi_mesh_ffi::viewer::Viewer;

/// `curl` rather than an HTTP client crate: this crate needs none of its own, and the request is
/// one POST. Answers the body and the status, because a 400 with a message is a result to assert
/// on and not a transport failure.
fn post(url: &str, body: &str) -> (String, String) {
    let out = Command::new("curl")
        .args([
            "-s",
            "-w",
            "\n%{http_code}",
            "-H",
            "Content-Type: application/json",
            "-X",
            "POST",
            url,
            "-d",
            body,
        ])
        .output()
        .expect("curl runs");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, status) = text
        .rsplit_once('\n')
        .expect("curl -w writes the status last");
    (body.to_string(), status.to_string())
}

/// Mint as soon as the minter has an address worth handing out, giving up after 30s.
fn mint_when_the_relay_is_up(minter: &Viewer) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match minter.invite() {
            Ok(token) => return token,
            Err(e) if std::time::Instant::now() >= deadline => {
                panic!("the minter never got a relay session: {e:#}")
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

#[test]
#[ignore = "needs a running control panel with its mesh up; see the module comment"]
fn the_panel_spends_an_invite_and_answers_with_the_credential() {
    let panel = std::env::var("PANEL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_string());
    // Read *before* the minter starts, and handed back to the CLI at the end. `Viewer::start`
    // repoints `$HOME` for the whole process — that is how it gets a store of its own — and a
    // child process inherits it, so an `adi-mono mesh unpair` run without this cleans up the temp
    // store and leaves the real pairing behind. Which it did, the first time this test ran.
    let real_home = std::env::var("HOME").expect("a home to put back");

    // The minting side: its own home, so nothing it writes lands in this machine's real fleet.
    let home = tempfile::tempdir().expect("a temp home");
    let minter = Viewer::start(home.path().to_str()).expect("the minter starts");
    // An invite names the address a node will dial, so it cannot be minted until the relay session
    // is up — a few seconds from a cold start, and the first thing to go wrong here if the network
    // is slow. Waited out rather than asserted, because "not yet" is not a failure.
    let token = mint_when_the_relay_is_up(&minter);

    let (body, status) = post(
        &format!("{panel}/api/fleet/join"),
        &serde_json::json!({ "token": token }).to_string(),
    );
    assert_eq!(status, "200", "{body}");

    let answer: serde_json::Value = serde_json::from_str(&body).expect("the answer is JSON");
    let viewer = answer["viewer"]
        .as_str()
        .expect("a viewer name")
        .to_string();
    println!(
        "paired with {viewer}; panel is filed there as {}",
        answer["petname"]
    );

    // The credential exists in plaintext exactly once, in this response. An empty one means the
    // page would show nothing and every later request to that machine would 401.
    assert!(
        !answer["password"].as_str().unwrap_or_default().is_empty(),
        "a join must answer with the password it minted: {body}"
    );
    assert_eq!(answer["username"], "adi");
    assert_eq!(answer["grants"][0], "http:app");

    // The answer carries the registry it just changed, so a page needs no second request.
    let listed = answer["fleet"]["nodes"]
        .as_array()
        .expect("a fleet list")
        .iter()
        .any(|node| node["petname"] == viewer.as_str());
    assert!(
        listed,
        "the new peer is missing from the answered fleet: {body}"
    );

    // And the far side saw the same pairing from its own end.
    let pairings = minter.take_pairings();
    assert!(
        !pairings.is_empty(),
        "the minter recorded no pairing, so something else answered this invite"
    );

    // Put the machine back: this pairing was a test, and it is real until it is undone.
    let unpaired = Command::new("adi-mono")
        .args(["mesh", "unpair", &viewer])
        .env("HOME", &real_home)
        .status()
        .expect("adi-mono runs");
    assert!(unpaired.success(), "unpair {viewer} by hand");
}
