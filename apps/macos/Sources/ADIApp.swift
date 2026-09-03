import SwiftUI

/// A standard single-window macOS app: a small dark panel that controls the local ADI services
/// (see `ContentView`). The window sizes itself to its content, so it grows and shrinks with the
/// step it is on.
@main
struct ADIApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        Window("ADI", id: "main") {
            ContentView(model: model)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
    }
}
