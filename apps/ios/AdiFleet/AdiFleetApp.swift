import SwiftUI

/// adi Fleet — a phone as a viewer on the mesh.
///
/// The device holds an Ed25519 identity and dials nodes by key. It runs no services and answers no
/// ALPN but the pairing one, which is why it needs no entitlement, no VPN profile and no open port:
/// everything it does, it initiates.
@main
struct AdiFleetApp: App {
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .onChange(of: scenePhase) { _, phase in
                    // iOS freezes the process in the background, and a QUIC connection that was
                    // alive when it went away is usually dead when it comes back — while still
                    // looking open until an idle timeout fires. Retiring the pool on the way back
                    // in turns a stall into a re-dial.
                    if phase == .active {
                        Task { await Mesh.shared.resume() }
                    }
                }
        }
    }
}
