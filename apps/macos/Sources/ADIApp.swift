import SwiftUI

/// A standard single-window macOS app: a translucent control panel with a big On/Off
/// button (see `ContentView`). The window sizes itself to its content, so it grows
/// when the details panel is revealed.
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
