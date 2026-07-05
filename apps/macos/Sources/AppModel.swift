import AppKit
import Foundation

/// Base directory for the app's runtime files: `$HOME/<name>/mono`, where `<name>`
/// is `ADI_DIR` (default `.adi`, the adi platform home). The `mono` subdir isolates
/// this app's files from the platform's own (`hive`/`cocoon`/`workforce`). A
/// login-launched LaunchAgent only sees env vars set in the launchd session
/// (e.g. `launchctl setenv ADI_DIR …`), not shell exports.
enum AppPaths {
    static let dirName: String = {
        let env = ProcessInfo.processInfo.environment["ADI_DIR"]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if let env, !env.isEmpty { return env }
        return ".adi"
    }()

    /// `mono` = this menu-bar "mono-app" namespace, kept clear of the platform's dirs.
    static var support: String { NSHomeDirectory() + "/" + dirName + "/mono" }
}

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var rows: [ServiceRow] = []

    private let services: [any ManagedService] = [
        DNSService()
    ]

    private var timer: Timer?

    init() {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }

    var anyRunning: Bool { rows.contains { $0.isRunning } }

    func toggle(_ id: String) {
        guard let svc = services.first(where: { $0.id == id }) else { return }
        if Launchd.isLoaded(label: svc.id) {
            Launchd.disable(label: svc.id)
            svc.onDisable()
        } else {
            let program = svc.program()
            Launchd.enable(label: svc.id, program: program, log: svc.logPath, env: svc.env)
            svc.onEnable()
        }
        refresh()
    }

    func perform(serviceID: String, actionID: String) {
        guard let svc = services.first(where: { $0.id == serviceID }),
            let action = svc.extraActions.first(where: { $0.id == actionID })
        else { return }
        action.perform()
        refresh()
    }

    func refresh() {
        rows = services.map { svc in
            let loaded = Launchd.isLoaded(label: svc.id)
            let status = Launchd.readStatus(svc.statusPath)
            let running = status.map { Launchd.processAlive($0.pid) } ?? false
            let statusText: String
            if running {
                statusText = svc.detail(status)
            } else if loaded {
                statusText = "Enabled · starting…"
            } else {
                statusText = "Stopped"
            }
            let actions = svc.extraActions
                .filter { $0.isVisible() }
                .map { ActionRow(id: $0.id, title: $0.title()) }
            return ServiceRow(
                id: svc.id,
                name: svc.name,
                isEnabled: loaded,
                isRunning: running,
                statusText: statusText,
                actions: actions
            )
        }
    }
}
