import AppKit
import Foundation
import SwiftUI

/// What the power button shows: dim when off, blue while a command runs or the service
/// is still coming up, orange once it's running.
enum PowerState: Equatable {
    case off
    case inProgress
    case on
}

/// How long to wait for the stack to answer before giving up on opening the dashboard.
/// Generous: a cold start binds ports, reads the store and, on a node, waits for the mesh.
private let serviceStartTimeout: TimeInterval = 30

/// Poll `adi-mono status` until something is running or the timeout passes.
///
/// Free function rather than a static on `AppModel`: the class is `@MainActor`, so anything
/// declared inside it is actor-isolated and cannot be touched from the detached task doing the
/// waiting — which is the whole point of doing it off the main thread.
private func waitForServices() async -> Report? {
    let deadline = Date().addingTimeInterval(serviceStartTimeout)
    var latest = Core.report()
    while !(latest?.anyRunning ?? false), Date() < deadline {
        try? await Task.sleep(nanoseconds: 400_000_000)
        latest = Core.report()
    }
    return latest
}

/// The view model: a thin shell over the `adi-mono` CLI. It holds the last status
/// report and triggers actions; all control logic lives in `adi-core`.
@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var report: Report = .empty
    /// True while a command is running, so the UI can show a spinner and disable input.
    @Published private(set) var busy = false

    private var timer: Timer?

    /// Something the person needs to be told, shown as an alert. One channel rather than a
    /// flag per failure, so a new one does not mean a new modifier on the view.
    struct Notice: Identifiable {
        let id = UUID()
        let title: String
        let body: String
    }

    @Published var notice: Notice?
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
                    self.notice = Notice(title: "ADI could not move itself",
                                         body: error.localizedDescription)
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

    /// True while services are being started on the way to the dashboard.
    @Published private(set) var launching = false

    /// Open the dashboard, starting ADI first if it is not up.
    ///
    /// The button used to grey out when nothing was running, which is a dead end: it states a
    /// precondition and leaves the person to work out how to satisfy it. Pressing it always
    /// means "take me to the dashboard", so when nothing is running that means starting
    /// everything and then going there.
    ///
    /// It waits for the services to actually answer rather than opening straight after
    /// `enable` returns — launchd accepting a job is not the same as the front door serving,
    /// and a browser tab that fails to load would look like ADI is broken.
    func openDashboard() {
        guard !launching else { return }
        if anyRunning {
            Self.open(domain: Core.domain)
            return
        }
        launching = true
        Task.detached(priority: .userInitiated) {
            _ = Core.run(["enable"])
            // `settled` is a `let`: the polling loop's mutable var stays inside the free
            // function. Reading a captured `var` from concurrently-executing code is an error
            // in Swift 6 and, as the timer above records, the kind of thing that compiles here
            // and fails on a CI runner with a different toolchain.
            let settled = await waitForServices()
            let cameUp = settled?.anyRunning ?? false
            await MainActor.run {
                if let settled { self.report = settled }
                self.launching = false
                if cameUp {
                    Self.open(domain: Core.domain)
                } else {
                    self.notice = Notice(
                        title: "ADI did not start",
                        body: "The services were asked to start but nothing is answering yet. "
                            + "Try again, or check the logs in Console."
                    )
                }
            }
        }
    }

    private static func open(domain: String) {
        guard let url = URL(string: "http://app.\(domain)/") else { return }
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

    /// Enable or disable the whole platform.
    func togglePower() {
        perform([isOn ? "disable" : "enable"])
    }

    /// The switch's binding. Reads the real state and writes by running the command — never by
    /// setting a local flag, so the control can only ever show what the services are actually
    /// doing, including when the command fails.
    var servicesOn: Binding<Bool> {
        Binding(get: { self.isOn }, set: { _ in self.togglePower() })
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
