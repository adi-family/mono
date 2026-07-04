import Foundation

/// One background service the ADI app supervises (DNS today; more to come).
///
/// A conforming type only describes *what* to run and *where* its files live; the
/// generic lifecycle (write plist, bootstrap, status) is handled by ``Launchd`` and
/// ``AppModel``. Add a service by adding a conformer to `AppModel.services` — no
/// menu code changes.
protocol ManagedService {
    /// launchd label, e.g. `family.adi.app.dns`. Unique per service.
    var id: String { get }
    /// Display name shown in the menu, e.g. `DNS`.
    var name: String { get }
    /// Path to the JSON status file the service writes when running.
    var statusPath: String { get }
    /// Path the service's stdout/stderr are redirected to.
    var logPath: String { get }
    /// Environment for the launchd job.
    var env: [String: String] { get }
    /// Service-specific menu items beyond enable/disable (e.g. "Install .adi route").
    var extraActions: [ServiceAction] { get }

    /// Full argv (binary + args). May write a config file as a side effect.
    func program() -> [String]
    /// Hook run right after the service is enabled (e.g. install OS routing).
    func onEnable()
    /// Hook run right after the service is disabled (e.g. remove OS routing).
    func onDisable()
    /// One-line running-state summary from the status file.
    func detail(_ status: DaemonStatus?) -> String
}

extension ManagedService {
    var env: [String: String] { ["RUST_LOG": "info"] }
    var extraActions: [ServiceAction] { [] }
    func onEnable() {}
    func onDisable() {}
    func detail(_ status: DaemonStatus?) -> String {
        guard let status else { return "" }
        return "Running · 127.0.0.1:\(status.port)"
    }
}

/// A service-specific extra menu action (title and visibility are dynamic so a
/// single entry can read "Install …" or "Remove …" depending on current state).
struct ServiceAction: Identifiable {
    let id: String
    let title: () -> String
    let isVisible: () -> Bool
    let perform: () -> Void
}

/// Immutable per-service snapshot the menu renders. Rebuilt on every refresh.
struct ServiceRow: Identifiable {
    let id: String
    let name: String
    let isEnabled: Bool
    let isRunning: Bool
    let statusText: String
    let actions: [ActionRow]
}

/// Rendered form of a ``ServiceAction`` (dynamic title resolved at snapshot time).
struct ActionRow: Identifiable {
    let id: String
    let title: String
}
