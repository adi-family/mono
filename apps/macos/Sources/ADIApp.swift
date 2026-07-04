import SwiftUI

/// ADI — a menu-bar-only (LSUIElement) app that supervises local ADI services as
/// per-user launchd LaunchAgents. DNS is the first built-in service; the registry
/// in ``AppModel`` is designed to hold more services and generic daemons.
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
