import XCTest

/// Drives the chat screens against a control panel reached directly (`ADI_DEV_PANEL`), so they can
/// be exercised without spending a pairing.
///
/// ```sh
/// TEST_RUNNER_ADI_DEV_PANEL=mac=http://127.0.0.1:8000 \
/// TEST_RUNNER_ADI_SHOT_DIR=/tmp/adi-chat-shots \
///   xcodebuild test -project AdiFleet.xcodeproj -scheme AdiFleetShots \
///     -destination id=<simulator> -only-testing:AdiFleetUITests/ChatTests
/// ```
///
/// Skipped unless a panel is named: there is nothing to chat with otherwise. It reads the panel's
/// real chats and writes nothing unless `ADI_CHAT_AGENT` names an agent to send one message to.
final class ChatTests: XCTestCase {
    private var environment: [String: String] { ProcessInfo.processInfo.environment }

    override func setUp() {
        continueAfterFailure = false
    }

    func testChats() throws {
        guard let panel = environment["ADI_DEV_PANEL"] else {
            throw XCTSkip("set TEST_RUNNER_ADI_DEV_PANEL=<name>=<url> to run the chat screens")
        }
        let app = XCUIApplication()
        app.launchEnvironment["ADI_DEV_PANEL"] = panel
        app.launch()

        // The app opens on its apps; one of them is all this asks of that screen.
        XCTAssertTrue(app.navigationBars["Apps"].waitForExistence(timeout: 30))
        // A named app if one was asked for (`ADI_APP_NAME`), else the first.
        let tiles = app.buttons.matching(identifier: "app")
        XCTAssertTrue(tiles.firstMatch.waitForExistence(timeout: 90), "the panel never listed an app")
        let tile = environment["ADI_APP_NAME"].map {
            tiles.matching(NSPredicate(format: "label CONTAINS %@", $0)).firstMatch
        } ?? tiles.firstMatch
        capture(app, "apps-01-grid")
        tile.tap()
        XCTAssertTrue(app.webViews.firstMatch.waitForExistence(timeout: 60), "the app never opened")
        Thread.sleep(forTimeInterval: 5)
        capture(app, "apps-02-open")
        app.navigationBars.buttons.firstMatch.tap()

        app.buttons["Chats"].firstMatch.tap()
        XCTAssertTrue(app.navigationBars["Chats"].waitForExistence(timeout: 30))
        // Every session, so the chat this run looks for is listed whoever started it.
        app.buttons["filter"].firstMatch.tap()
        capture(app, "chat-00-filter")
        app.buttons["All sessions"].firstMatch.tap()
        // By identifier: a notice about a machine that did not answer is a row in the same list.
        let chats = app.descendants(matching: .any).matching(identifier: "chat")
        let first = chats.firstMatch
        XCTAssertTrue(first.waitForExistence(timeout: 90), "the panel never listed a chat")
        capture(app, "chat-01-list")

        // A named chat if one was asked for, else whichever is on top.
        if let wanted = environment["ADI_CHAT_TITLE"] {
            let field = app.searchFields.firstMatch
            if !field.exists { app.swipeDown() }
            XCTAssertTrue(field.waitForExistence(timeout: 10))
            field.tap()
            field.typeText(wanted)
            XCTAssertTrue(chats.firstMatch.waitForExistence(timeout: 20))
        }
        chats.firstMatch.tap()
        XCTAssertTrue(composer(app).waitForExistence(timeout: 30), "the chat never opened")
        Thread.sleep(forTimeInterval: 4)
        capture(app, "chat-02-thread")

        // Open the first folded group of tool calls, if the chat has one in view.
        let groups = app.buttons.matching(identifier: "calls")
        let calls = groups.element(boundBy: max(groups.count - 1, 0))
        if groups.count > 0, calls.isHittable {
            calls.tap()
            Thread.sleep(forTimeInterval: 3)
            capture(app, "chat-03-calls")
        }
        app.swipeDown()
        Thread.sleep(forTimeInterval: 1)
        capture(app, "chat-04-scrolled")

        // `ADI_CHAT_AGENT` picks an agent in the new-chat sheet; a message is sent to it only when
        // `ADI_CHAT_SEND` is set too, because that starts a real run on a real machine.
        guard let agent = environment["ADI_CHAT_AGENT"] else { return }
        // From a fresh launch: an active search replaces the bar the button lives in.
        app.terminate()
        app.launch()
        // From the apps screen, which is where the panel's home offers it too.
        XCTAssertTrue(app.buttons["New chat"].firstMatch.waitForExistence(timeout: 60))
        app.buttons["New chat"].firstMatch.tap()
        // The picker opens on the starred agents; the one asked for may not be one of them.
        let all = app.buttons["all-agents"].firstMatch
        XCTAssertTrue(app.buttons.matching(identifier: "agent").firstMatch.waitForExistence(timeout: 60),
                      "the panel never listed its agents")
        capture(app, "chat-05-agents")
        // Under the starred ones, so usually below the fold.
        let sheet = app.collectionViews.element(boundBy: app.collectionViews.count - 1)
        for _ in 0..<8 where !all.exists { sheet.swipeUp() }
        XCTAssertTrue(all.exists, "the picker has no way to every agent")
        all.tap()
        let row = app.buttons.matching(identifier: "agent")
            .matching(NSPredicate(format: "label CONTAINS %@", agent)).firstMatch
        for _ in 0..<20 where !row.exists { sheet.swipeUp() }
        XCTAssertTrue(row.exists, "no agent called \(agent)")
        row.tap()

        let composer = composer(app)
        XCTAssertTrue(composer.waitForExistence(timeout: 20))
        guard environment["ADI_CHAT_SEND"] != nil else { return }
        composer.tap()
        composer.typeText(environment["ADI_CHAT_MESSAGE"] ?? "Reply with exactly one word: pong")
        capture(app, "chat-06-draft")
        app.buttons["Send"].tap()
        Thread.sleep(forTimeInterval: 3)
        capture(app, "chat-07-sent")
        let answered = app.staticTexts.containing(NSPredicate(format: "label CONTAINS[c] 'pong'"))
        // Two of them: the question, and the answer.
        let deadline = Date().addingTimeInterval(180)
        while answered.count < 2, Date() < deadline { Thread.sleep(forTimeInterval: 2) }
        capture(app, "chat-08-answered")
        XCTAssertGreaterThanOrEqual(answered.count, 2, "the agent never answered")
    }

    /// The message field. By identifier, because a field that grows with its text is a text view
    /// to XCUITest on some systems and a text field on others.
    private func composer(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: "composer").firstMatch
    }

    /// Attach a capture to the result, and write it to `ADI_SHOT_DIR` when one is named.
    private func capture(_ app: XCUIApplication, _ name: String) {
        let shot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let directory = environment["ADI_SHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            try? shot.pngRepresentation.write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(name).png"))
        }
    }
}
