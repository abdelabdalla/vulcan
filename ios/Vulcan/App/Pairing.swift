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
enum PairingStore {
    private static var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "vulcan.pairing",
            kSecAttrAccount as String: "pc",
        ]
    }

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
        SecItemAdd(q as CFDictionary, nil)
    }

    static func clear() {
        SecItemDelete(query as CFDictionary)
    }
}
