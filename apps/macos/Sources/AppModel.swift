import Foundation

/// What the power button shows: dim when off, blue while a command runs or the service
/// is still coming up, orange once it's running.
enum PowerState: Equatable {
    case off
    case inProgress
    case on
}

/// The view model: a thin shell over the `adi-mono` CLI. It holds the last status
/// report and triggers actions; all control logic lives in `adi-core`.
@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var report: Report = .empty
    /// True while a command is running, so the UI can show a spinner and disable input.
    @Published private(set) var busy = false

    private var timer: Timer?

    init() {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }

    /// On == at least one service is enabled (the big button's state).
    var isOn: Bool { report.services.contains { $0.enabled } }
    var anyRunning: Bool { report.anyRunning }

    /// Button color state: a command in flight or a service still starting is "in
    /// progress" (blue); actually running is "done" (orange); otherwise off.
    var powerState: PowerState {
        if busy { return .inProgress }
        if anyRunning { return .on }
        if isOn { return .inProgress }
        return .off
    }

    /// Short word under the power button.
    var statusSummary: String {
        if report.services.isEmpty { return "No services" }
        if anyRunning { return "Running" }
        if isOn { return "Starting…" }
        return "Off"
    }

    /// The big On/Off button: enable or disable the whole platform.
    func togglePower() {
        perform([isOn ? "disable" : "enable"])
    }

    /// Poll `adi-mono status --json` off the main thread; publish on the main actor.
    func refresh() {
        Task.detached(priority: .utility) {
            if let latest = Core.report() {
                await MainActor.run { self.report = latest }
            }
        }
    }

    /// Trigger `adi-mono <args>` off the main thread — some actions prompt for an admin
    /// password, which must not block the UI — then republish fresh status.
    func perform(_ args: [String]) {
        busy = true
        Task.detached(priority: .userInitiated) {
            _ = Core.run(args)
            let latest = Core.report()
            await MainActor.run {
                if let latest { self.report = latest }
                self.busy = false
            }
        }
    }
}
