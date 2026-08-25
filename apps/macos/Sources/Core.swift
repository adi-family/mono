import Foundation

/// Bridges the app to `adi-core` by running the bundled `adi-mono` CLI. The app owns
/// no launchd/config/route logic anymore — every action is `adi-mono <args>`, and live
/// state is the JSON `adi-mono status --json` emits. This is the only place that talks
/// to the core.
enum Core {
    /// The bundled CLI binary name. Slated to be renamed to `adi`; change this one
    /// constant (and `crates/adi-cli/Cargo.toml`'s `[[bin]] name`) to match.
    static let binaryName = "adi-mono"

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
}
