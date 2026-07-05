import SwiftUI

/// Menu-bar-only (LSUIElement) app; supervises ADI services as launchd LaunchAgents.
@main
struct ADIApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        MenuBarExtra {
            MenuContent(model: model)
        } label: {
            Image(systemName: model.anyRunning ? "square.stack.3d.up.fill" : "square.stack.3d.up")
        }
        .menuBarExtraStyle(.menu)
    }
}
