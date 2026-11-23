//! That a viewer which **replaces its endpoint** still reaches a real machine afterwards — over a
//! real relay, by real discovery, on the endpoint the app itself would bind.
//!
//! The unit tests in `viewer.rs` prove the machinery against a node made up on the spot, with no
//! relay and a fixed address. What they cannot prove is the part that only exists for real:
//! that `bind_endpoint` can be called a second time in one process on the stored identity, that
//! the relay takes the new session once the old one is closed, and that a machine dialled by key
//! alone is found again from an endpoint that has never heard of it. If any of those failed, a
//! phone would lose its whole fleet on the first network change and keep it lost — which is the
//! bug this exists to keep fixed, made worse.
//!
//! Ignored, because it needs a real peer and a real network. Run it by hand against a machine
//! whose mesh is up, with an invite pinned to a petname so the cleanup is one known command:
//!
//! ```text
//! TOKEN=$(adi-mono mesh invite --no-qr --json --petname reconnect-test | jq -r .token) \
//!     cargo test -p adi-mesh-ffi --test reconnect_against_a_live_node -- --ignored --nocapture
//! adi-mono mesh unpair reconnect-test
//! ```
//!
//! It pairs for real, so the last line is not optional.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use adi_mesh_ffi::viewer::Viewer;
use base64::Engine as _;

/// One request through the loopback port, the way the app's own HTTP client makes it. Answers the
/// status line, or the reason there was none.
fn get(port: u16, authorization: &str) -> Result<String, String> {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    write!(
        tcp,
        "GET /api/dashboards HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Authorization: Basic {authorization}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("write: {e}"))?;
    let mut reply = Vec::new();
    tcp.read_to_end(&mut reply)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&reply);
    text.lines()
        .next()
        .map(str::to_string)
        .ok_or_else(|| "an empty reply".to_string())
}

/// Ask until the answer is a `200`, for at most `within`. Returns how long that took.
fn answers(what: &str, port: u16, authorization: &str, within: Duration) -> Duration {
    let began = Instant::now();
    let mut last = String::new();
    while began.elapsed() < within {
        match get(port, authorization) {
            Ok(status) if status.contains(" 200") => {
                println!("{what}: {status} after {:.1?}", began.elapsed());
                return began.elapsed();
            }
            Ok(status) => last = status,
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("{what}: no 200 within {within:?}; the last answer was: {last}");
}

#[test]
#[ignore = "needs a live minting peer; see the module comment"]
fn a_viewer_reaches_its_node_again_after_replacing_its_endpoint() {
    let token =
        std::env::var("TOKEN").expect("set TOKEN to an `adi-invite:…` from `adi-mono mesh invite`");

    // Its own store, so the pairing this writes cannot land in the developer's real fleet.
    let home = tempfile::tempdir().expect("a temp home");
    let viewer = Viewer::start(home.path().to_str()).expect("the viewer starts");
    let paired = viewer.join(&token).expect("spending the invite");
    let authorization = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", paired.username, paired.password));
    let port = viewer
        .open(&paired.petname, "app")
        .expect("a loopback port for the node's panel");

    // The node's gateway re-reads its registry every five seconds, so the first answers after a
    // pairing are refusals. Waited out, not asserted on.
    answers(
        "freshly paired",
        port,
        &authorization,
        Duration::from_secs(30),
    );

    // Back from the background: every listener binds its port again, and the pool is retired.
    let before = viewer.generation();
    viewer.resume();
    assert!(viewer.generation() > before, "resume retires the pool");
    answers(
        "after resume",
        port,
        &authorization,
        Duration::from_secs(20),
    );

    // The network changed: a new endpoint, from the socket up. Asked at once, on purpose — the
    // request is meant to wait for the new endpoint rather than fail on the old.
    for round in 1..=2 {
        let before = viewer.generation();
        viewer.reconnect();
        let took = answers(
            &format!("after reconnect {round}"),
            port,
            &authorization,
            Duration::from_secs(40),
        );
        assert!(
            viewer.generation() > before,
            "a reconnect that the request outran is not one this test has proved anything about"
        );
        assert!(
            took < Duration::from_secs(20),
            "a reconnect is meant to cost about a second, and this one cost {took:?}"
        );
        // Two reports of one event are one reconnect; a second *event* has to be a moment later.
        std::thread::sleep(Duration::from_secs(3));
    }

    // And the app's own questions travel the same road as a page's.
    let catalog = viewer
        .dashboards(&paired.petname, &paired.username, &paired.password)
        .expect("the node's dashboards, over the new endpoint");
    println!(
        "the node calls this viewer {:?} and lists {} dashboard(s)",
        catalog.me,
        catalog.dashboards.len()
    );

    // The new endpoint is attended like the first: it can still be paired with.
    viewer
        .invite()
        .expect("an invite, minted on an endpoint a reconnect bound");

    viewer.stop();
}
