import Foundation

/// One background service the ADI app supervises. Conformers describe *what* to run
/// and *where* its files live; ``Launchd``/``AppModel`` handle the lifecycle.
protocol ManagedService {
    var id: String { get }
    var name: String { get }
    var statusPath: String { get }
    var logPath: String { get }
    var env: [String: String] { get }
    var extraActions: [ServiceAction] { get }

    /// Full argv (binary + args). May write a config file as a side effect.
    func program() -> [String]
    func onEnable()
    func onDisable()
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

/// `title`/`isVisible` are closures so one entry can read "Install …" or "Remove …".
struct ServiceAction: Identifiable {
    let id: String
    let title: () -> String
    let isVisible: () -> Bool
    let perform: () -> Void
}

struct ServiceRow: Identifiable {
    let id: String
    let name: String
    let isEnabled: Bool
    let isRunning: Bool
    let statusText: String
    let actions: [ActionRow]
}

struct ActionRow: Identifiable {
    let id: String
    let title: String
}
