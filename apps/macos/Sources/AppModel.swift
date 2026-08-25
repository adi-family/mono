import AppKit
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

    /// A failed move, to show instead of silently doing nothing.
    @Published var moveFailure: String?
    /// Set once the stack has been auto-started, so it happens once per launch and not on
    /// every two-second poll that happens to arrive after the last permission is granted.
    private var didAutoStart = false

    init() {
        refresh()
        // Bring the whole stack up on launch (`adi-mono up`): idempotent and it never
        // restarts a running service, so on a machine that's already up it's a no-op, while
        // on a fresh one it installs + starts everything (one admin prompt for the DNS
        // route + front door). This is what makes services autostart when the app opens.
        //
        // Not until setup is finished. `adi-core` refuses to install from a disk image anyway,
        // and would refuse to route anything before the grants exist, but hitting those
        // refusals on every launch is not onboarding — the window asks for what is missing
        // instead, and `autoStartIfReady` runs this the moment the last piece lands.
        autoStartIfReady()
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

    /// True while the bundle is being copied, so the button can say so.
    @Published private(set) var moving = false

    /// Copy the app into Applications and relaunch from there. Never returns on success.
    ///
    /// Off the main thread: this copies the whole bundle, usually off a compressed disk image,
    /// and running it inline froze the window until it finished — indistinguishable from a
    /// button that did nothing.
    func moveToApplications() {
        guard !moving else { return }
        moving = true
        Task.detached(priority: .userInitiated) {
            do {
                let destination = try InstallLocation.copyToApplications()
                await MainActor.run { InstallLocation.relaunch(at: destination) }
            } catch {
                await MainActor.run {
                    self.moveFailure = error.localizedDescription
                    self.moving = false
                }
            }
        }
    }

    // MARK: onboarding
    //
    // Three states, in order, and each one is the whole window until it is satisfied. Nothing
    // below the current step is reachable: an app that cannot install services has no business
    // showing a power button, and a power button that turns on services nothing can route to
    // would come up "Running" while `app.adi` goes nowhere.

    enum Stage: Equatable {
        /// The bundle is on a disk image or translocated. Move it; nothing else is offered.
        case mustMove
        /// Moved, but one or both privileged grants are missing.
        case needsPermissions
        /// Everything is in place.
        case ready
    }

    var stage: Stage {
        if !report.setup.locationDurable { return .mustMove }
        if !report.setup.ready { return .needsPermissions }
        return .ready
    }

    var hasDNS: Bool { report.setup.dnsRoute }
    var hasNetwork: Bool { report.setup.frontDoor }

    /// Grant the DNS route — one admin prompt.
    func grantDNS() { perform(["dns", "grant-dns"]) }

    /// Grant the front door — one admin prompt.
    func grantNetwork() { perform(["dns", "grant-network"]) }

    /// Open the dashboard, which is the whole point of the app being on.
    func openDashboard() {
        guard let url = URL(string: "http://app.\(Core.domain)/") else { return }
        NSWorkspace.shared.open(url)
    }

    /// Start the stack as soon as — and only once — setup is complete.
    ///
    /// Called on launch and after every action, so granting the second permission starts
    /// everything without a further click. The flag is what keeps the two-second poll from
    /// re-running `up` forever.
    func autoStartIfReady() {
        guard !didAutoStart, report.setup.ready else { return }
        didAutoStart = true
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
                await MainActor.run {
                    self.report = latest
                    self.autoStartIfReady()
                }
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
                self.autoStartIfReady()
            }
        }
    }
}
