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
    /// This device's key: what a node's operator authorizes (§2). Kept because it is what the mesh
    /// reports about itself, and cheap to hold; no screen shows it since the toolbar chip that used
    /// to went — an invite carries the key, so nobody has to read it off a screen to pair.
    private(set) var key: String = ""
    /// False until the relay session is up. Until then an invite would name an endpoint that only
    /// the local network can dial, so the pairing button says so instead of minting one.
    private(set) var ready = false
    private(set) var nodes: [Node] = []
    /// Set when something failed in a way the person should see.
    var failure: String?
    /// The petname of the node paired most recently, so the list can say which row is new.
    private(set) var justPaired: String?

    /// Each node's dashboards, by petname, once it has answered. A node that has not answered yet
    /// is *absent*; one that answered with nothing is an empty array — and the two read
    /// differently on screen, which is the point of not defaulting to `[]`.
    private(set) var dashboards: [String: [NodeDashboard]] = [:]
    /// Petnames with a listing in flight, so a section can say so rather than look empty.
    private(set) var listing: Set<String> = []
    /// Why a node's listing failed, by petname.
    ///
    /// Deliberately not `failure`: one sleeping node must not raise an alert over the whole fleet,
    /// and the sentence belongs next to the node it is about.
    private(set) var listingFailure: [String: String] = [:]

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
        await collectPairings()
        await reloadNodes()
        // Both detached, and for the same reason: the relay handshake takes seconds and a node
        // that is asleep takes the core's whole step timeout to say so. The node list is a file
        // read and is on screen immediately; a launch that waited for either of these would look
        // hung while it had everything it needed to draw.
        Task { await listEverything() }
        Task { await pollReadiness() }
    }

    /// Re-read the node list and collect any pairing that completed.
    func refresh() async {
        await collectPairings()
        await reloadNodes()
        await listEverything()
    }

    /// Just the node list — the part that is a file read and always answers.
    private func reloadNodes() async {
        do {
            nodes = try await Mesh.shared.nodes()
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Ask every node for its dashboards, all at once.
    ///
    /// Concurrently, because these are independent machines: one asleep would otherwise add its
    /// whole timeout to the wait for every node after it.
    private func listEverything() async {
        let nodes = nodes
        await withTaskGroup(of: Void.self) { group in
            for node in nodes {
                // A node nothing has ever been listed for may be one whose pairing landed a moment
                // ago, and a pairing is not visible to the node's own gateway straight away — see
                // `list(_:attempts:)`. Patience is spent only there: a node already listed once is
                // asked once, so an ordinary refresh stays as quick as it was.
                let attempts = dashboards[node.petname] == nil ? Self.grantWindowAttempts : 1
                group.addTask { @MainActor in await self.list(node, attempts: attempts) }
            }
        }
    }

    /// How many times a just-paired node is asked before its refusal is believed.
    ///
    /// One a second, and comfortably past the five seconds the node's gateway takes to notice the
    /// grant (`adi-mesh/src/gateway.rs`, `RELOAD_INTERVAL`).
    private static let grantWindowAttempts = 8

    /// Ask one node what dashboards it has.
    ///
    /// A node this phone has no password for is not asked at all. That is not a failure to report
    /// as one: the panel's own gate is what the password answers (`docs/fleet.md` §5), and opening
    /// the node's control panel once is what fills the Keychain — so the sentence says that.
    ///
    /// `attempts` exists for the one moment the first answer is expected to be wrong. A node whose
    /// pairing has just completed refuses this call for a few seconds: its gateway judges requests
    /// against an in-memory snapshot of its registry and re-reads it every five seconds
    /// (`adi-mesh/src/gateway.rs`, `RELOAD_INTERVAL`), so a listing sent the instant the handshake
    /// returns is judged against a registry that has never heard of this phone. `ServiceView`
    /// already waits that window out for the same reason.
    ///
    /// Asking once and keeping that refusal is what left a freshly paired node showing nothing but
    /// its `app` row — with no way back to the dashboards short of relaunching the app, since
    /// nothing asks again on its own. So the refusals inside the window are not the node's answer
    /// and are not shown; only the last one is.
    func list(_ node: Node, attempts: Int = 1) async {
        guard let credential = Keychain.credential(for: node.petname) else {
            listingFailure[node.petname] =
                "No password for \(node.petname) on this phone yet — open its control panel once and sign in."
            return
        }
        listing.insert(node.petname)
        defer { listing.remove(node.petname) }
        for attempt in 1...max(attempts, 1) {
            do {
                let catalog = try await Mesh.shared.dashboards(node: node.petname, credential: credential)
                dashboards[node.petname] = catalog.dashboards
                listingFailure[node.petname] = nil
                return
            } catch {
                guard attempt < attempts else {
                    listingFailure[node.petname] = error.localizedDescription
                    return
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    /// Ask `node` to share `dashboard` with this phone, so its page can be opened directly.
    ///
    /// The grant is the node's to make and this only asks for it — through the same control panel
    /// the phone is already inside, with the same password. On success the row stops saying it is
    /// unshared straight away rather than at the next listing, because the person is looking at it.
    ///
    /// Returns the error to show, or `nil` when the node agreed.
    func share(_ service: String, on node: Node) async -> String? {
        guard let credential = Keychain.credential(for: node.petname) else {
            return "This phone has no password for \(node.petname), so it cannot ask for anything."
        }
        do {
            try await Mesh.shared.allow(node: node.petname, service: service, credential: credential)
        } catch {
            return error.localizedDescription
        }
        dashboards[node.petname] = dashboards[node.petname]?.map {
            $0.service == service ? $0.shared() : $0
        }
        // The core mirrored the grant into this phone's own registry, so the node's service list
        // has changed too.
        await reloadNodes()
        return nil
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

    /// Spend an invite a machine minted, pairing with it from this side.
    ///
    /// The other half of `invite()`. Everything a completed pairing means happens here too, in the
    /// same order `collectPairings` does it — password to the Keychain first, then the list — so a
    /// node paired this way is indistinguishable from one that dialled us.
    ///
    /// Returns the petname on success so the sheet can dismiss, `nil` when it failed and `failure`
    /// is carrying the reason.
    @discardableResult
    func spend(invite token: String) async -> String? {
        do {
            let paired = try await Mesh.shared.join(token: token)
            Keychain.save(
                .init(username: paired.username, password: paired.password),
                for: paired.petname
            )
            justPaired = paired.petname
            await reloadNodes()
            await listEverything()
            return paired.petname
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
            // Nothing of a forgotten node stays behind: its listing would otherwise reappear under
            // a re-used petname that is a different machine.
            dashboards[node.petname] = nil
            listingFailure[node.petname] = nil
            await reloadNodes()
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
