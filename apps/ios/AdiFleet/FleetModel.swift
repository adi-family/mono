import Foundation
import SwiftUI

/// The app's single source of truth: what the mesh knows, in a form SwiftUI can observe.
///
/// Every mutating call funnels through here so that "a pairing completed" has exactly one meaning
/// in the app — the Keychain gets the password, the node list is refreshed, and the pairing sheet
/// can close — rather than three views each doing part of it.
@MainActor
@Observable
final class FleetModel {
    /// This device's key: what a node's operator authorizes (§2), shown so it can be read out.
    private(set) var key: String = ""
    /// False until the relay session is up. Until then an invite would name an endpoint that only
    /// the local network can dial, so the pairing button says so instead of minting one.
    private(set) var ready = false
    private(set) var nodes: [Node] = []
    /// Set when something failed in a way the person should see.
    var failure: String?
    /// The petname of the node paired most recently, so the list can say which row is new.
    private(set) var justPaired: String?

    private var started = false

    /// Start the mesh and load what it already knows. Safe to call more than once.
    func start() async {
        guard !started else { return }
        started = true
        do {
            key = try await Mesh.shared.start()
        } catch {
            failure = error.localizedDescription
            return
        }
        await refresh()
        // Detached, because the relay handshake takes seconds and the node list must be on screen
        // before then. `start()` awaiting this would mean a launch that looks hung.
        Task { await pollReadiness() }
    }

    /// Re-read the node list and collect any pairing that completed.
    func refresh() async {
        await collectPairings()
        do {
            nodes = try await Mesh.shared.nodes()
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Called when the app returns to the foreground: the OS froze the process, so every pooled
    /// connection is suspect and the node list may have changed under a pairing that landed just
    /// before suspension.
    func resume() async {
        await Mesh.shared.resume()
        await refresh()
        if !ready {
            Task { await pollReadiness() }
        }
    }

    /// A single-use invite to spend on a node.
    func invite() async -> String? {
        do {
            return try await Mesh.shared.invite()
        } catch {
            failure = error.localizedDescription
            return nil
        }
    }

    /// Unpair a node and forget its password.
    func forget(_ node: Node) async {
        do {
            _ = try await Mesh.shared.forget(node: node.petname)
            Keychain.remove(for: node.petname)
            await refresh()
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Move any completed pairing's password into the Keychain, once.
    ///
    /// This is the only consumer of `takePairings`, and it must stay that way: the Rust side hands
    /// each pairing over exactly once, so a second caller would race this one for a password that
    /// only exists in that one reply.
    func collectPairings() async {
        do {
            for pairing in try await Mesh.shared.takePairings() {
                Keychain.save(
                    .init(username: pairing.username, password: pairing.password),
                    for: pairing.petname
                )
                justPaired = pairing.petname
            }
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Poll until the relay session is up, so the UI can stop saying "not yet".
    ///
    /// Bounded rather than indefinite: if a phone has no route to a relay at all, the honest state
    /// is "not ready" and a loop that never ends would just keep a timer alive behind a screen
    /// nobody is looking at. Foregrounding runs `resume`, which starts this again.
    private func pollReadiness() async {
        for _ in 0..<40 {
            if let status = try? await Mesh.shared.status(), status.ready {
                key = status.key
                ready = true
                return
            }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }
}
