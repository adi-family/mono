import Foundation

/// Codable mirror of the JSON status file `adi-dns` writes (see
/// `crates/adi-dns/src/status.rs`) — polled to learn the dynamically-bound port.
struct DaemonStatus: Codable {
    let pid: Int32
    let domain: String
    let boundAddr: String
    let port: Int
    let routeInstalled: Bool
    let startedAtUnix: Int
    let version: String

    enum CodingKeys: String, CodingKey {
        case pid
        case domain
        case port
        case version
        case boundAddr = "bound_addr"
        case routeInstalled = "route_installed"
        case startedAtUnix = "started_at_unix"
    }
}
