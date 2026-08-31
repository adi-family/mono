import XCTest

/// Drives the app to each screen worth photographing and captures it.
///
/// This is the screenshot harness, not a test suite. `simctl` can boot, install and screenshot but
/// it cannot **tap**, so without something inside the UI session nothing can navigate and the only
/// obtainable shot is the launch screen. XCUITest is that something.
///
/// It still asserts, because a harness that cannot fail will happily photograph a blank screen and
/// report success. Every capture is gated on the thing it is meant to show actually being on
/// screen first.
///
/// ## Running it
///
/// The fleet has to be populated, which means a real pairing with a real machine. The token is
/// passed in the environment rather than typed — 941 characters through `typeText` is slow and
/// flaky — and spent through the app's own **Enter an invite** field, so the shot of a populated
/// fleet is a shot of a fleet that was genuinely paired:
///
/// ```sh
/// apps/ios/shots.sh                 # mints an invite on adi-demo and runs this
/// ```
///
/// An invite is single-use, so each run needs a fresh one. That is `shots.sh`'s job.
final class ScreenshotTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false

        // Belt and braces. The app pastes through a `PasteButton`, which is system-attested and so
        // raises no permission alert — but a stray system modal in front of the app does not fail
        // a query, it makes it *time out*, which reads as "the button does not exist" and sends
        // you looking at the wrong thing. That is exactly what happened on the first run here.
        addUIInterruptionMonitor(withDescription: "paste or system alert") { alert in
            for label in ["Allow Paste", "Allow", "OK"] where alert.buttons[label].exists {
                alert.buttons[label].tap()
                return true
            }
            return false
        }
    }

    func testCaptureAppStoreScreens() throws {
        let app = XCUIApplication()
        app.launchEnvironment["ADI_UITEST"] = "1"
        app.launch()

        // 1 — the empty state. Not a store screenshot, but it is what App Review meets first, and
        // having it captured is how we can tell whether the "no nodes" copy still reads well.
        XCTAssertTrue(app.staticTexts["No nodes yet"].waitForExistence(timeout: 30),
                      "the app should open on the empty fleet")
        capture(app, "01-empty")

        // 2 — the scanner, which is the tab the sheet opens on because it is the flow almost
        // everybody uses: the machine draws a code and the phone reads it.
        //
        // On a simulator there is no camera, so what this captures is the fallback state, not a
        // viewfinder. That is worth knowing before anyone reaches for it as a store panel — a
        // picture of the live camera has to come off a real device with a code in front of it.
        app.buttons["Pair a node"].firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Pair a node"].waitForExistence(timeout: 10))
        capture(app, "03-scan")

        // 3 — minting, the other direction. Its QR is the prettiest thing in the app and needs no
        // fleet, so it is captured before pairing.
        app.buttons["Invite a machine"].tap()
        XCTAssertTrue(app.images.firstMatch.waitForExistence(timeout: 60),
                      "the QR should appear once an invite has been minted")
        capture(app, "02-invite")

        // Hand the token to the waiting script. `shots.sh` is polling the simulator's pasteboard;
        // when it sees one it runs `adi-mono mesh join` on the machine, which dials back here.
        //
        // This direction — **this device mints, the machine dials** — is deliberate and is not just
        // what the harness finds convenient. It is the app's default tab, and it is the only one
        // that yields a real name for the machine today: the dialler declares its nickname in the
        // handshake, so the row reads `adi-demo`. Spending an invite the *machine* minted files it
        // under a key-derived `viewer-25f6795fa6` instead, until every node is rebuilt with the
        // `Accepted.nickname` field that fixes it.
        let copy = app.buttons["Copy command"].firstMatch
        XCTAssertTrue(copy.waitForExistence(timeout: 30))
        copy.tap()

        // 4 — the populated fleet. Nothing happens on this screen: the machine dials back on its
        // own and the sheet dismisses itself, so the node row appearing is the whole signal.
        let node = app.staticTexts["adi-demo"]
        XCTAssertTrue(node.waitForExistence(timeout: 240),
                      "the machine never dialled back — did `mesh join` run, and is the node up?")
        // Its dashboards arrive on a second round trip, and the first one usually fails: the node's
        // gateway serves from a snapshot of its registry and re-reads it every five seconds
        // (`docs/fleet.md` §8), so for a moment after pairing it is still consulting a registry that
        // has never heard of this phone and refuses the connection. The app retries on its own and
        // the row carries the refusal meanwhile — which is honest, and is not what a store
        // screenshot should show. So: pull to refresh until the dashboard is really there.
        // The first listing does not retry on a timer — it is re-driven by `.refreshable` — so this
        // pulls to refresh rather than waiting harder.
        let dashboard = app.staticTexts["Hello Reviewer"]
        for _ in 0..<10 where !dashboard.exists {
            if dashboard.waitForExistence(timeout: 20) { break }
            pullToRefresh(app)
        }
        XCTAssertTrue(dashboard.waitForExistence(timeout: 90),
                      "the node paired but never listed its dashboards")
        // One more beat so the transient error under the row has cleared from the view.
        Thread.sleep(forTimeInterval: 2)
        capture(app, "04-fleet")

        // 5 — a dashboard open on its own origin, which is the product's actual point. Tapping a
        // row the phone has no grant for is what asks for one (`adi_mesh_allow`), so this also
        // exercises the self-serve grant a reviewer depends on.
        if dashboard.exists {
            dashboard.tap()
            // Opening a row the phone holds no grant for asks for one first, so this waits through
            // a grant round trip *and* the node's five-second registry reload before the page even
            // starts loading. iPad is reliably slower at it than iPhone.
            let web = app.webViews.firstMatch
            XCTAssertTrue(web.waitForExistence(timeout: 60), "the dashboard screen never opened")

            // The page is a WKWebView; its text is what proves the remote machine answered.
            let greeting = app.webViews.staticTexts.containing(
                NSPredicate(format: "label CONTAINS[c] 'Hello reviewer'")).firstMatch
            let rendered = greeting.waitForExistence(timeout: 150)
            // Let the clock tick once so the shot cannot catch the placeholder.
            Thread.sleep(forTimeInterval: 2)
            // Captured *before* the assertion on purpose: when this fails, the picture of what was
            // on screen is the only thing that says why, and an assertion above it throws the shot
            // away. That cost two runs' worth of guessing on the iPad.
            capture(app, "05-dashboard")
            XCTAssertTrue(rendered, "the dashboard opened but never rendered the remote page")
        }
    }

    /// Pull the fleet list down far enough to fire `.refreshable`.
    ///
    /// `app.swipeDown()` is not enough: it is a short flick aimed at the whole app, and SwiftUI's
    /// refresh control wants a deliberate drag that starts inside the scroll view. A press-and-drag
    /// across most of the list is what actually triggers it.
    private func pullToRefresh(_ app: XCUIApplication) {
        let list = app.collectionViews.firstMatch.exists
            ? app.collectionViews.firstMatch
            : app.descendants(matching: .any).matching(identifier: "Fleet").firstMatch
        let target = list.exists ? list : app
        let top = target.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.25))
        let bottom = target.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85))
        top.press(forDuration: 0.1, thenDragTo: bottom)
    }

    /// Save one full-screen capture under `name`, kept whether or not the test passes.
    private func capture(_ app: XCUIApplication, _ name: String) {
        let shot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
