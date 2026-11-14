import Foundation

// Mirrors daemon/src/protocol.rs. JSON text frames tagged by "t".

enum MouseButton: String {
    case left, right, middle
}

enum ButtonAction: String {
    case down, up, click
}

enum KeyAction: String {
    case press, down, up
}

enum ClientMessage: Encodable {
    case hello(token: String, device: String)
    case move(dx: Double, dy: Double)
    case scroll(dx: Double, dy: Double)
    case button(MouseButton, ButtonAction)
    case text(String)
    case key(String, mods: [String] = [], state: KeyAction = .press)
    case ping

    static let protocolVersion = 1

    private enum CodingKeys: String, CodingKey {
        case t, token, device, v, dx, dy, b, s, k, mods
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .hello(token, device):
            try c.encode("hello", forKey: .t)
            try c.encode(token, forKey: .token)
            try c.encode(device, forKey: .device)
            try c.encode(Self.protocolVersion, forKey: .v)
        case let .move(dx, dy):
            try c.encode("move", forKey: .t)
            try c.encode(dx, forKey: .dx)
            try c.encode(dy, forKey: .dy)
        case let .scroll(dx, dy):
            try c.encode("scroll", forKey: .t)
            try c.encode(dx, forKey: .dx)
            try c.encode(dy, forKey: .dy)
        case let .button(b, s):
            try c.encode("button", forKey: .t)
            try c.encode(b.rawValue, forKey: .b)
            try c.encode(s.rawValue, forKey: .s)
        case let .text(s):
            try c.encode("text", forKey: .t)
            try c.encode(s, forKey: .s)
        case let .key(k, mods, state):
            try c.encode("key", forKey: .t)
            try c.encode(k, forKey: .k)
            try c.encode(mods, forKey: .mods)
            try c.encode(state.rawValue, forKey: .s)
        case .ping:
            try c.encode("ping", forKey: .t)
        }
    }

    var json: String {
        let data = (try? JSONEncoder().encode(self)) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}

struct ServerMessage: Decodable {
    let t: String
    let msg: String?
    let paused: Bool?
    let host: String?
    let device: String?

    static func parse(_ text: String) -> ServerMessage? {
        try? JSONDecoder().decode(ServerMessage.self, from: Data(text.utf8))
    }
}
