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
        seedUpdateState()
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


    // MARK: keeping this install current
    //
    // The update control lives in the app, and it runs the bundled CLI, because that is the only
    // place it can be counted on. Every other way to a new version goes through a name this
    // install serves — the control panel is `app.adi` — and a broken `.adi` route is exactly the
    // fault somebody needs the fix for: they cannot open the page that would tell them how to
    // repair the thing that stops them opening pages. `adi-mono` is in `Contents/Resources` and
    // talks to GitHub over the system's own DNS, so it works when nothing else here does.

    @Published private(set) var updateState: UpdateState = .unknown

    enum UpdateState: Equatable {
        /// Nothing has been asked yet, or the record on disk says nothing useful.
        case unknown
        case checking
        case upToDate
        case available(String)
        /// The published version has no build for this machine — a real answer, not an error.
        case unavailable(String)
        case installing
        case failed
    }

    /// The version in the bundle. Never blank and never late: this is the number to show while
    /// the updater is deciding what to say about it.
    let installedVersion = Core.installedVersion

    /// Whether to offer the control at all — see `Core.isReleaseInstall`.
    let updatable = Core.isReleaseInstall

    /// Seed the update row from what the background agent already wrote down.
    ///
    /// A local file read, so it costs nothing and needs no network: the periodic updater has
    /// been checking every few hours, and the window can say what it found before anyone
    /// presses anything.
    private func seedUpdateState() {
        guard updatable else { return }
        Task.detached(priority: .utility) {
            guard case let .success(status) = Core.json(["update", "status", "--json"],
                                                        as: UpdateStatus.self)
            else { return }
            let state: UpdateState? = switch status.lastOutcome {
            case "update-available": status.latestVersion.map { .available($0) }
            case "up-to-date", "installed": .upToDate
            default: nil
            }
            guard let state else { return }
            await MainActor.run { self.updateState = state }
        }
    }

    /// Ask the release channel, now. The one action that reaches the network on purpose.
    func checkForUpdates() {
        guard updateState != .checking, updateState != .installing else { return }
        updateState = .checking
        Task.detached(priority: .userInitiated) {
            let result = Core.json(["update", "check", "--json"], as: UpdateCheck.self)
            await MainActor.run {
                switch result {
                case let .success(check) where check.updateAvailable:
                    self.updateState = .available(check.latest)
                case let .success(check) where !check.hasArtifact && check.latest != check.installed:
                    self.updateState = .unavailable(check.latest)
                case .success:
                    self.updateState = .upToDate
                case let .failure(why):
                    self.updateState = .failed
                    self.notice = Notice(
                        title: "ADI could not check for updates",
                        body: why.message + "\n\nThis usually means no route to the internet. Everything "
                            + "already installed keeps working."
                    )
                }
            }
        }
    }

    /// Install the published version.
    ///
    /// This process does not outlive the command it starts: a successful update swaps the bundle
    /// and then terminates and reopens the app from the new one. The CLI is a child rather than a
    /// part of this process, so it survives that and finishes the job — which is why nothing here
    /// tries to report success. What comes back is a window that is already the new version.
    func installUpdate() {
        guard updateState != .installing else { return }
        updateState = .installing
        Task.detached(priority: .userInitiated) {
            let result = Core.json(["update", "run", "--json"], as: UpdateRun.self)
            await MainActor.run {
                switch result {
                case let .success(run) where run.outcome == "rolled-back":
                    self.updateState = .failed
                    self.notice = Notice(
                        title: "The update was rolled back",
                        body: (run.why ?? "The services did not come back.")
                            + "\n\nADI is running the version it was on before, so nothing is lost."
                    )
                case let .success(run):
                    self.updateState = .upToDate
                    if run.outcome == "installed" {
                        self.notice = Notice(
                            title: "ADI was updated",
                            body: "Now on \(run.to ?? "the latest version")."
                        )
                    }
                case let .failure(why):
                    self.updateState = .failed
                    self.notice = Notice(title: "The update did not finish", body: why.message)
                }
            }
        }
    }

    // MARK: reporting a problem

    @Published private(set) var reportState: ReportState = .idle

    enum ReportState: Equatable {
        case idle
        case collecting
        /// Where the archive landed, and how many things the collector already thinks are wrong.
        case ready(URL, Int)
        case failed
    }

    /// The last archive collected, kept whole so the issue draft can name it and repeat its
    /// findings. The published state carries only what the row draws.
    private var lastReport: DiagnosticBundle?

    /// Collect one archive of everything that could explain a failure, and show it in Finder.
    ///
    /// The button exists because the person who hits a fault here is the one least able to say
    /// what it was: DNS, the front door, a service that never started and an update that rolled
    /// back all present as "it doesn't work". `adi-mono diagnose` reads all of it at once — it
    /// starts and stops nothing — and leaves one file to attach to a message.
    func createReport() {
        guard reportState != .collecting else { return }
        reportState = .collecting
        Task.detached(priority: .userInitiated) {
            let result = Core.json(["diagnose", "--json"], as: DiagnosticBundle.self)
            await MainActor.run {
                switch result {
                case let .success(bundle):
                    let url = URL(fileURLWithPath: bundle.path)
                    self.lastReport = bundle
                    self.reportState = .ready(url, bundle.findings.count)
                    // Revealed rather than merely reported: the next thing anyone does with this
                    // file is drag it into a message, and the store directory it lands in is
                    // hidden in Finder — a path in a label would be a dead end.
                    Self.reveal(url)
                case let .failure(why):
                    self.reportState = .failed
                    self.notice = Notice(title: "The report could not be written", body: why.message)
                }
            }
        }
    }

    /// Show a finished report in Finder again, for a second attempt at sending it.
    func revealReport() {
        guard case let .ready(url, _) = reportState else { return }
        Self.reveal(url)
    }

    private static func reveal(_ url: URL) {
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    // MARK: filing it upstream

    /// How much of the draft to put in the URL. GitHub prefills an issue from the query string,
    /// and both it and the browser have a length past which the request is simply refused —
    /// which would present as a button that opens a blank page. The draft is a few hundred
    /// characters, so this only ever bites a machine with an implausible number of findings.
    private static let issueBodyLimit = 6000

    /// Open a new GitHub issue with what we already know filled in.
    ///
    /// Prefilled because the facts that decide a bug report — which build, which OS, and which
    /// of the three setup gates is open — are exactly the ones nobody thinks to include, and
    /// asking for them costs a round trip each. The one thing it cannot carry is the archive:
    /// GitHub takes an attachment only from a drop onto the form, so the draft ends by saying so.
    func openIssue() {
        guard let url = issueURL() else {
            notice = Notice(
                title: "ADI could not open GitHub",
                body: "The issue address in this build is not a valid URL."
            )
            return
        }
        NSWorkspace.shared.open(url)
    }

    private func issueURL() -> URL? {
        guard let base = Core.issuesURL,
              var components = URLComponents(url: base, resolvingAgainstBaseURL: false)
        else { return nil }
        components.queryItems = [
            URLQueryItem(name: "body", value: String(issueBody().prefix(Self.issueBodyLimit))),
        ]
        // `URLComponents` leaves a literal `+` alone in a query, and a form on the other end
        // reads it back as a space — so a version like `1.2.0+dev` would arrive mangled. After
        // encoding is the only point where a real plus and an encoded space can still be told
        // apart, which is why this is a fixup rather than part of building the value.
        components.percentEncodedQuery = components.percentEncodedQuery?
            .replacingOccurrences(of: "+", with: "%2B")
        return components.url
    }

    /// The draft: a space to write in, then the state of this install.
    private func issueBody() -> String {
        func mark(_ granted: Bool) -> String { granted ? "yes" : "**no**" }

        let setup = report.setup
        var lines = [
            "<!-- What happened, and what you expected instead. -->",
            "",
            "",
            "---",
            "",
            "**ADI** \(Core.installedVersion) · \(Core.flavorId) · "
                + "macOS \(Self.osVersion) · \(Self.architecture)",
            "",
            "**Setup** — app somewhere durable: \(mark(setup.locationDurable)) · "
                + ".\(Core.domain) route: \(mark(setup.dnsRoute)) · "
                + "front door: \(mark(setup.frontDoor))",
        ]

        if !report.services.isEmpty {
            let states = report.services.map { service -> String in
                let word = if service.running {
                    "running"
                } else {
                    service.enabled ? "enabled, not running" : "off"
                }
                return "\(service.name): \(word)"
            }
            lines += ["", "**Services** — " + states.joined(separator: " · ")]
        }

        if let bundle = lastReport {
            if !bundle.findings.isEmpty {
                lines += ["", "**The report already flags**"]
                lines += bundle.findings.prefix(10).map { "- \($0)" }
            }
            let name = (bundle.path as NSString).lastPathComponent
            lines += ["", "Attached: `\(name)` — drag it into this box; it has the logs."]
        } else {
            lines += [
                "",
                "Press **Create Report** in ADI and drag the archive it makes into this box — "
                    + "it carries the logs, the routes and every service's state.",
            ]
        }
        return lines.joined(separator: "\n")
    }

    private static var osVersion: String {
        let v = ProcessInfo.processInfo.operatingSystemVersion
        return "\(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
    }

    /// Which slice of the universal binary is running — the truth on a Mac using Rosetta, and
    /// the thing to know when an app launches on one machine and not another.
    private static var architecture: String {
        #if arch(arm64)
            "arm64"
        #else
            "x86_64"
        #endif
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
