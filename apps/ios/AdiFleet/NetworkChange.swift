import Foundation
import Network

/// Retires pooled mesh connections when the phone changes network.
///
/// The app already does this on foregrounding, which covers being suspended. It does not cover the
/// case a phone spends its life in: walking out of the door with the app open, and the connection
/// moving from Wi-Fi to cellular. QUIC does not notice at once — the old path simply stops
/// answering — and `Pool::get` hands the dead connection out again because `is_usable` asks only
/// whether it was *closed*, which it was not. Every request then waits out the 12-second step
/// timeout before anything re-dials.
///
/// `NWPathMonitor` knows within a moment, so this turns that stall into a re-dial.
///
/// It deliberately does **not** try to be clever about which change matters. Retiring a pool is
/// cheap — the next request dials, and dialling was going to happen anyway — while missing a change
/// costs a visible hang. What it does avoid is reacting to the same state twice, because the
/// monitor reports freely and a reset per report would retire connections that were just made.
@MainActor
final class NetworkChangeWatcher {
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "family.adi.fleet.path")
    /// What the last report described, so an unchanged one is ignored.
    private var last: Fingerprint?

    /// The parts of a path that mean "your existing connections are on a route that no longer
    /// exists". Deliberately not the whole `NWPath`: it is not `Equatable`, and its finer detail
    /// (endpoint lists, DNS config) changes for reasons that do not invalidate a socket.
    private struct Fingerprint: Equatable {
        let status: NWPath.Status
        let interfaces: [String]
        let isExpensive: Bool
        let isConstrained: Bool
    }

    func start() {
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
    }

    private func observe(_ seen: Fingerprint) {
        defer { last = seen }
        // The first report describes the network the app just launched on, and nothing is pooled
        // yet — resetting there would be a no-op that only muddies the log.
        guard let previous = last, previous != seen else { return }
        // A path going away is not itself a reason to retire anything: there is nothing to dial
        // until one comes back, and the report that brings it back is the one worth acting on.
        guard seen.status == .satisfied else { return }
        Task {
            await Mesh.shared.resume()
        }
    }
}
