import Foundation
import Network

/// Says when the phone has moved to a different network while the app is open.
///
/// The app already recovers on foregrounding, which covers being suspended. It does not cover the
/// case a phone spends its life in: walking out of the door with the app open, and the connection
/// moving from Wi-Fi to cellular. QUIC does not notice at once — the old path simply stops
/// answering — and the core's connection pool hands the dead connection out again because it asks
/// only whether a connection was *closed*, which it was not.
///
/// `NWPathMonitor` knows within a moment. What is done with that is the caller's: the fleet model
/// has the mesh look for a connection again and re-reads what the screens show
/// (`FleetModel.networkChanged`). Retiring the pool alone, which is all this used to do, left the
/// endpoint under it exactly as stale as the connections — and left whatever had already failed on
/// screen, with nothing to ask again.
///
/// It deliberately does **not** try to be clever about which change matters. Reconnecting is
/// cheap — a second, and dialling was going to happen anyway — while missing a change costs a
/// visible hang. What it does avoid is reacting to the same state twice, because the monitor
/// reports freely, and reacting to every step of one move, because a network comes up in several.
@MainActor
final class NetworkChangeWatcher {
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "family.adi.fleet.path")
    /// What the last report described, so an unchanged one is ignored.
    private var last: Fingerprint?
    /// The report waiting out `settle` before it is acted on.
    private var pending: Task<Void, Never>?
    private var changed: (@MainActor () async -> Void)?

    /// How long a new path has to stay put before it counts as the network the phone is on now.
    /// Joining a Wi-Fi is reported in steps — the interface, then its route, then cellular
    /// stepping back — and each would otherwise be a reconnect that cuts off the one before it.
    private static let settle: Duration = .milliseconds(600)

    /// The parts of a path that mean "your existing connections are on a route that no longer
    /// exists". Deliberately not the whole `NWPath`: it is not `Equatable`, and its finer detail
    /// (endpoint lists, DNS config) changes for reasons that do not invalidate a socket.
    private struct Fingerprint: Equatable {
        let status: NWPath.Status
        let interfaces: [String]
        let isExpensive: Bool
        let isConstrained: Bool
    }

    /// Start watching. `changed` runs on the main actor once per move, after it has settled.
    func start(changed: @escaping @MainActor () async -> Void) {
        self.changed = changed
        monitor.pathUpdateHandler = { [weak self] path in
            let seen = Fingerprint(
                status: path.status,
                interfaces: path.availableInterfaces.map(\.name),
                isExpensive: path.isExpensive,
                isConstrained: path.isConstrained
            )
            Task { @MainActor in
                self?.observe(seen)
            }
        }
        monitor.start(queue: queue)
    }

    func stop() {
        monitor.cancel()
        pending?.cancel()
    }

    private func observe(_ seen: Fingerprint) {
        defer { last = seen }
        // The first report describes the network the app just launched on, and the mesh has only
        // just bound to it — there is nothing older to replace.
        guard let previous = last, previous != seen else { return }
        // A path going away is not itself a reason to do anything: there is nothing to dial
        // until one comes back, and the report that brings it back is the one worth acting on.
        // It does withdraw a report still settling, which described a network that is now gone.
        pending?.cancel()
        guard seen.status == .satisfied else { return }
        pending = Task { [weak self] in
            try? await Task.sleep(for: Self.settle)
            guard !Task.isCancelled else { return }
            await self?.changed?()
        }
    }
}
