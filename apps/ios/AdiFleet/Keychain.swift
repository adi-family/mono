import Foundation

/// Where a node's password lives.
///
/// The mesh grant is machine-scoped and the password is what makes it human-scoped
/// (`docs/fleet.md` §5) — so on a phone, which is a machine one person holds, the password is the
/// whole of the second layer. It goes in the Keychain and nowhere else: not in `UserDefaults`, not
/// beside the fleet registry, and never on disk in plaintext, which is the property §8 asks both
/// sides to keep.
///
/// `ThisDeviceOnly` is deliberate. A credential that synced to iCloud would follow the account onto
/// a device that was never paired, and the whole design says a machine is reachable because its key
/// was authorized — not because someone signed in.
enum Keychain {
    /// One entry per node, keyed by petname.
    private static let service = "family.adi.fleet.node"

    /// What a node's services demand.
    struct Credential: Codable {
        let username: String
        let password: String
    }

    /// Store (or replace) the credential for `node`.
    ///
    /// Both halves travel together as one JSON item rather than as an account/password pair,
    /// because they are minted together at pairing and are only ever useful together.
    static func save(_ credential: Credential, for node: String) {
        guard let data = try? JSONEncoder().encode(credential) else { return }
        // Delete-then-add rather than SecItemUpdate: re-pairing rotates the password (§8), and this
        // way the new value cannot end up merged with a stale attribute set.
        SecItemDelete(query(for: node) as CFDictionary)

        var attributes = query(for: node)
        attributes[kSecValueData as String] = data
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(attributes as CFDictionary, nil)
    }

    /// The credential stored for `node`, if any.
    static func credential(for node: String) -> Credential? {
        var query = query(for: node)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data
        else { return nil }
        return try? JSONDecoder().decode(Credential.self, from: data)
    }

    /// Forget a node's credential — called when the node is unpaired, so nothing is left behind
    /// that would silently work again if the same petname were re-used later.
    static func remove(for node: String) {
        SecItemDelete(query(for: node) as CFDictionary)
    }

    private static func query(for node: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: node,
        ]
    }
}
