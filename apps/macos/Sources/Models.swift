import Foundation

/// Codable mirror of the JSON `adi-mono status --json` emits (see
/// `crates/adi-core/src/commands.rs` + `service.rs`). The app renders these directly;
/// it derives no state of its own, so new services/actions appear with no UI changes.
struct Report: Codable {
    let anyRunning: Bool
    let services: [ServiceReport]
    let setup: SetupReport

    enum CodingKeys: String, CodingKey {
        case anyRunning = "any_running"
        case services
        case setup
    }

    static let empty = Report(anyRunning: false, services: [], setup: .unknown)
}

/// What still has to happen before the app can do anything, straight from `adi-core`.
///
/// Deliberately not derived here. Whether the bundle is somewhere services may point at, and
/// whether the two privileged grants are in place, are questions about the running executable
/// and about files under `/etc` and `/Library` — the core answers all three so there is one
/// definition of "ready" rather than a second one in Swift that drifts from it.
struct SetupReport: Codable {
    let locationDurable: Bool
    let dnsRoute: Bool
    let frontDoor: Bool
    let ready: Bool

    enum CodingKeys: String, CodingKey {
        case locationDurable = "location_durable"
        case dnsRoute = "dns_route"
        case frontDoor = "front_door"
        case ready
    }

    /// Before the first report arrives. Not ready, and not claiming the bundle is misplaced
    /// either — showing the move prompt for a few milliseconds on every launch would be a lie
    /// most of the time.
    static let unknown = SetupReport(locationDurable: true, dnsRoute: false,
                                     frontDoor: false, ready: false)
}

struct ServiceReport: Codable, Identifiable {
    let id: String
    let name: String
    let enabled: Bool
    let running: Bool
    let detail: String
    let actions: [ActionReport]
}

/// One button. `args` is the argv to pass back to `adi-mono` to perform it, so the
/// menu stays generic — it never encodes what an action does.
struct ActionReport: Codable, Identifiable {
    let id: String
    let title: String
    let args: [String]
}

/// `adi-mono update status --json` — the updater's persisted record (`adi-update`'s `State`).
///
/// A file read, not a fetch, which is why the window can say "1.2.0 is available" the moment it
/// opens: the background agent has been checking every few hours and wrote down what it found.
struct UpdateStatus: Codable {
    let installedVersion: String?
    let latestVersion: String?
    let lastOutcome: String?
    let lastError: String?

    enum CodingKeys: String, CodingKey {
        case installedVersion = "installed_version"
        case latestVersion = "latest_version"
        case lastOutcome = "last_outcome"
        case lastError = "last_error"
    }
}

/// `adi-mono update check --json` — what the release channel says right now.
struct UpdateCheck: Codable {
    let installed: String
    let latest: String
    let updateAvailable: Bool
    /// Whether the published release carries a build for this machine. A newer version that does
    /// not is not an update anyone can take, and saying so beats offering a button that fails.
    let hasArtifact: Bool

    enum CodingKeys: String, CodingKey {
        case installed
        case latest
        case updateAvailable = "update_available"
        case hasArtifact = "has_artifact"
    }
}

/// `adi-mono update run --json` — adi-core's `RunOutcome`, which is tagged by `outcome` and
/// carries a different set of fields per arm, hence the optionals.
struct UpdateRun: Codable {
    /// `up-to-date`, `installed`, or `rolled-back`.
    let outcome: String
    let installed: String?
    let from: String?
    let to: String?
    /// Why the new version was put back, when `outcome` is `rolled-back`.
    let why: String?
}

/// `adi-mono diagnose --json` — the archive that was written, and the collector's own reading
/// of what looks wrong on this machine.
struct DiagnosticBundle: Codable {
    let path: String
    let bytes: Int
    let files: [String]
    let findings: [String]
}
