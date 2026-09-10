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

        // The control panel, in the app (`PanelWindow`). A second `Window` rather than a
        // `WindowGroup`: there is one machine and one panel, so a second copy of it would only be
        // a second place for the same state to be — and `openWindow` on a `Window` already
        // brings the existing one forward, which is what pressing the button a second time means.
        //
        // Not shown at launch. The window above is the whole app until the install is set up, and
        // the panel is what pressing its one button opens.
        Window("Control panel", id: PanelWindow.sceneID) {
            PanelWindow(home: Core.panelURL)
        }
        .defaultSize(width: 1180, height: 800)
        // A title bar of its own line, with the tabs on the next one — and it stays **AppKit's**,
        // so double-click to zoom or minimise (as that Mac is set to), drag, and the rest work
        // because they are the real thing rather than an imitation.
        //
        // `.hiddenTitleBar` is here for one of the two things it does: it makes SwiftUI *want* the
        // title bar transparent, which is what lets the window's own `--bg-side` show through
        // instead of the system's grey. Its other effect — `fullSizeContentView` — is the one that
        // breaks the gestures, because SwiftUI's hosting view then spans the band and claims every
        // click in it; `WindowChrome` takes that bit back out and keeps it out.
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentMinSize)
        // ⌘R and ⌘W, on the menu bar (`PanelCommands`). Attached to this scene because it is the
        // window they are for, though a menu bar belongs to the app: the items are on it whichever
        // window is front, and it is `FrontPanel` — which knows the panel's tabs and whether the
        // panel has the keyboard — that decides what they do and whether they can be pressed.
        .commands { PanelCommands() }
    }
}
