import Foundation
import Security

/// Everything needed to reach and trust one PC, from its `vulcan://pair?...` QR code.
struct Pairing: Codable, Equatable {
    /// Last known address; refreshed via Bonjour when the PC's IP changes.
    var host: String
    var port: Int
    var token: String
    /// SHA-256 of the PC's TLS certificate (lowercase hex). This is the PC's identity.
    var fingerprint: String
    var hostname: String
    var deviceName: String

    init?(url: URL) {
        guard url.scheme == "vulcan", url.host == "pair",
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        else { return nil }
        var q: [String: String] = [:]
        for item in items {
            if let v = item.value { q[item.name] = v }
        }
        guard let h = q["h"], let p = q["p"].flatMap(Int.init),
              let t = q["t"], t.count == 64,
              let f = q["f"]?.lowercased(), f.count == 64
        else { return nil }
        host = h
        port = p
        token = t
        fingerprint = f
        hostname = q["n"] ?? h
        deviceName = q["d"] ?? "iPhone"
    }
}

/// Keeps the pairing in the Keychain (survives the weekly re-sign as long as the team and bundle ID stay the same).
/// It lives in a keychain group shared with the Share extension, so that can send files on its own.
enum PairingStore {
    /// `<team prefix>vulcan.shared`, filled in at build time (see project.yml).
    private static var group: String? {
        (Bundle.main.object(forInfoDictionaryKey: "VulcanKeychainGroup") as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    private static var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "vulcan.pairing",
            kSecAttrAccount as String: "pc",
        ]
    }

    /// Searches every keychain group this target can see.
    static func load() -> Pairing? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else {
            return nil
        }
        return try? JSONDecoder().decode(Pairing.self, from: data)
    }

    static func save(_ pairing: Pairing) {
        guard let data = try? JSONEncoder().encode(pairing) else { return }
        SecItemDelete(query as CFDictionary)
        var q = query
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        if let group {
            q[kSecAttrAccessGroup as String] = group
            if SecItemAdd(q as CFDictionary, nil) == errSecSuccess { return }
            // Not signed with the shared group (e.g. the simulator): keep it app-only.
            q[kSecAttrAccessGroup as String] = nil
        }
        SecItemAdd(q as CFDictionary, nil)
    }

    /// Pairings saved before the Share extension existed sit in the app's private group; move them.
    static func shareWithExtension() {
        guard let group, let pairing = load() else { return }
        var q = query
        q[kSecAttrAccessGroup as String] = group
        if SecItemCopyMatching(q as CFDictionary, nil) != errSecSuccess { save(pairing) }
    }

    static func clear() {
        SecItemDelete(query as CFDictionary)
    }
}
