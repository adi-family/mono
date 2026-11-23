import SwiftUI

/// adi Fleet — a phone as a viewer on the mesh.
///
/// The device holds an Ed25519 identity and dials nodes by key. It runs no services and answers no
/// ALPN but the pairing one, which is why it needs no entitlement, no VPN profile and no open port:
/// everything it does, it initiates.
@main
struct AdiFleetApp: App {
    init() {
        // The navigation bar and the segmented control are UIKit's; they take the tokens here,
        // once, rather than per screen.
        ADI.applyAppearance()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                // The app is dark (design/DESIGN.md §3), and every tinted control — the back
                // chevron, a link's disclosure, a toggle — takes ink rather than the accent (§3:
                // orange is never a selected state or a link).
                .preferredColorScheme(.dark)
                .tint(ADI.ink)
            // Coming back from the background and changing network are handled in `RootView`,
            // beside the model: both end in re-reading what the screens show, and the model is
            // what knows how.
        }
    }
}
