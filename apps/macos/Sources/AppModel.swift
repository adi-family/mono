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

    /// Set when the app is running somewhere its services must not point at, which is what the
    /// window asks about before doing anything else.
    @Published private(set) var installLocation = InstallLocation.current()
    /// A failed move, to show instead of silently doing nothing.
    @Published var moveFailure: String?

    init() {
        refresh()
        // Bring the whole stack up on launch (`adi-mono up`): idempotent and it never
        // restarts a running service, so on a machine that's already up it's a no-op, while
        // on a fresh one it installs + starts everything (one admin prompt for the DNS
        // route + front door). This is what makes services autostart when the app opens.
        //
        // Not from a disk image or a translocated copy. `adi-core` refuses those anyway, but
        // this is the launch that would otherwise hit the refusal on every single open —
        // double-clicking the app inside the downloaded .dmg is the most common first run
        // there is, and the honest response to it is the move prompt, not a no-op.
        if !installLocation.needsMoving {
            perform(["up"])
        }
        // Unwrap *before* the Task, so it captures an immutable binding rather than the
        // outer closure's mutable optional. Reading a captured `var` from concurrently
        // executing code is rejected outright by Swift 5.10 ("reference to captured var
        // 'self'"); newer compilers accept it, which is how this survived every local build
        // and only surfaced on a CI runner with an older toolchain.
        timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in self.refresh() }
        }
    }

    /// Copy the app into Applications and relaunch from there. Never returns on success.
    func moveToApplications() {
        do {
            try InstallLocation.moveToApplications()
        } catch {
            moveFailure = error.localizedDescription
        }
    }

    /// Carry on without moving: bring the stack up anyway, accepting that the services will
    /// break when the volume goes. Offered because refusing to do anything at all is worse than
    /// letting someone who knows what they are doing proceed.
    func proceedWithoutMoving() {
        installLocation = .durable
        perform(["up"])
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
