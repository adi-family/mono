import Foundation

/// Bridges the app to `adi-core` by running the bundled `adi-mono` CLI. The app owns
/// no launchd/config/route logic anymore — every action is `adi-mono <args>`, and live
/// state is the JSON `adi-mono status --json` emits. This is the only place that talks
/// to the core.
enum Core {
    /// The bundled CLI binary name. Slated to be renamed to `adi`; change this one
    /// constant (and `crates/adi-cli/Cargo.toml`'s `[[bin]] name`) to match.
    static let binaryName = "adi-mono"

    /// Why a CLI call could not be turned into a value.
    ///
    /// A wrapper around one sentence rather than a case per failure: everything that goes wrong
    /// out there — offline, a refusal, a version that answers something this build cannot read —
    /// reaches the window the same way, as text in an alert, and the CLI has already written the
    /// sentence worth showing.
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// The zone this install serves — `adi`, or `adi-dev` for a dev build. From the bundle's
    /// own `Info.plist`, for the same reason the flavour is: both apps ship the same CLI, and a
    /// dev build must link to `app.adi-dev`, not to the real install's dashboard.
    static var domain: String {
        (Bundle.main.object(forInfoDictionaryKey: "ADIDomain") as? String)
            .flatMap { $0.isEmpty ? nil : $0 } ?? "adi"
    }

    private static var binaryPath: String {
        Bundle.main.resourceURL?.appendingPathComponent(binaryName).path
            ?? Bundle.main.bundlePath + "/Contents/Resources/" + binaryName
    }

    /// Which install this bundle drives, from its own `Info.plist` (`ADIFlavor`, stamped by
    /// build.sh). Absent means the release install, which is what an older bundle wants.
    ///
    /// The bundle carrying its own identity is the point: ADI Dev.app and ADI.app ship the
    /// same `adi-mono`, and a CLI that resolved the flavour from the environment would get the
    /// release install for both — so the dev app would enable, disable and reconfigure the
    /// real one. Reading it here means the wrong install is not reachable by accident.
    private static var flavor: String? {
        (Bundle.main.object(forInfoDictionaryKey: "ADIFlavor") as? String)
            .flatMap { $0.isEmpty ? nil : $0 }
    }

    /// The flavour id, for the places that only *report* it rather than act on it.
    static var flavorId: String { flavor ?? "release" }

    /// Where a bug gets filed.
    ///
    /// From `Info.plist`, beside `ADIDomain`, so the app names it in one place — and not
    /// restamped per flavour, because a dev build's bugs belong in the same tracker as a release
    /// build's. The canonical value is the workspace `Cargo.toml`'s `repository`.
    static var issuesURL: URL? {
        let configured = (Bundle.main.object(forInfoDictionaryKey: "ADIIssuesURL") as? String)
            .flatMap { $0.isEmpty ? nil : $0 }
        return URL(string: configured ?? "https://github.com/adi-family/mono/issues/new")
    }

    /// Whether this bundle is the real install rather than a second one running beside it.
    ///
    /// The update control is hidden on anything else. There is one release channel and its
    /// artifact is `ADI.app`, so asking a dev build to update itself would download the released
    /// bundle and swap it over its own — turning the copy you were testing with into a stale
    /// duplicate of the copy you were testing against. `adi-core` refuses to *schedule* the
    /// updater outside the release flavour for the same reason; this is the same refusal for the
    /// button somebody presses by hand.
    static var isReleaseInstall: Bool {
        flavor.map { $0 == "release" } ?? true
    }

    /// The version in this bundle's `Info.plist` — the number `build.sh` stamped from the git
    /// tag, and the same one the updater compares against the published manifest.
    ///
    /// Read from the plist rather than by running the CLI: it is wanted on the first frame, and
    /// it is the one piece of version information that is still there when nothing else works.
    static var installedVersion: String {
        (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String)
            .flatMap { $0.isEmpty ? nil : $0 } ?? "unknown"
    }

    /// Run `adi-mono <args>` to completion; returns exit status + combined stdout/stderr.
    /// Blocking — callers run it off the main thread (some actions prompt for a password).
    @discardableResult
    static func run(_ args: [String]) -> (status: Int32, output: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binaryPath)
        proc.arguments = args
        if let flavor {
            var env = ProcessInfo.processInfo.environment
            env["ADI_FLAVOR"] = flavor
            proc.environment = env
        }
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        do {
            try proc.run()
        } catch {
            return (-1, "failed to launch \(binaryName): \(error)")
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        proc.waitUntilExit()
        return (proc.terminationStatus, String(data: data, encoding: .utf8) ?? "")
    }

    /// `adi-mono status --json`, decoded into the report the menu renders.
    static func report() -> Report? {
        let result = run(["status", "--json"])
        guard result.status == 0, let data = result.output.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(Report.self, from: data)
    }

    /// Run `adi-mono <args>` keeping stdout and stderr apart. Blocking, like [`run`].
    ///
    /// [`run`] merges the two, which is right for a command whose output is only ever shown to a
    /// person and wrong for one whose stdout is parsed: a single warning on stderr would land in
    /// the middle of the JSON and the decode would fail with nothing to say about why.
    static func capture(_ args: [String]) -> (status: Int32, stdout: String, stderr: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: binaryPath)
        proc.arguments = args
        if let flavor {
            var env = ProcessInfo.processInfo.environment
            env["ADI_FLAVOR"] = flavor
            proc.environment = env
        }
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err

        // stderr is drained as it arrives rather than after stdout: a pipe holds about 64 KB
        // before the writer blocks, and a command that talked past that on stderr while we sat
        // reading stdout to EOF would deadlock — both sides waiting for the other.
        var errData = Data()
        let lock = NSLock()
        err.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            guard !chunk.isEmpty else { return }
            lock.lock()
            errData.append(chunk)
            lock.unlock()
        }

        do {
            try proc.run()
        } catch {
            err.fileHandleForReading.readabilityHandler = nil
            return (-1, "", "failed to launch \(binaryName): \(error)")
        }
        let outData = out.fileHandleForReading.readDataToEndOfFile()
        proc.waitUntilExit()
        err.fileHandleForReading.readabilityHandler = nil
        lock.lock()
        errData.append(err.fileHandleForReading.availableData)
        let stderr = String(data: errData, encoding: .utf8) ?? ""
        lock.unlock()

        return (proc.terminationStatus, String(data: outData, encoding: .utf8) ?? "", stderr)
    }

    /// Run `adi-mono <args> --json` and decode its stdout, or say why that could not be done in
    /// a sentence fit to show someone.
    ///
    /// The CLI's own convention is that a failed command prints a plain line on stderr and exits
    /// non-zero without emitting JSON, so stderr is the message whenever there is one.
    static func json<T: Decodable>(_ args: [String], as type: T.Type) -> Result<T, Failure> {
        let result = capture(args)
        let complaint = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        guard result.status == 0 else {
            return .failure(Failure(message: complaint.isEmpty
                ? "\(binaryName) \(args.joined(separator: " ")) failed (exit \(result.status))"
                : complaint))
        }
        guard let data = result.stdout.data(using: .utf8),
              let decoded = try? JSONDecoder().decode(type, from: data)
        else {
            return .failure(Failure(message: complaint.isEmpty
                ? "\(binaryName) answered something this version cannot read"
                : complaint))
        }
        return .success(decoded)
    }
}
