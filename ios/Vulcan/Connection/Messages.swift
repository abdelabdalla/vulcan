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

enum MediaAction: String {
    case playPause = "play_pause"
    case next, previous
}

enum VolumeAction: String {
    case up, down, mute, set
}

enum DisplayAction: String {
    case off, on, toggle
}

enum PowerAction: String, Identifiable {
    case lock, suspend, reboot, poweroff
    var id: String { rawValue }
}

/// One PC monitor with its position in the desktop layout (logical pixels).
struct MonitorInfo: Decodable, Identifiable, Equatable {
    /// Connector name, e.g. "DP-1".
    let name: String
    /// Friendly name from the PC's config, or the model.
    let label: String
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let on: Bool

    var id: String { name }
}

/// A USB device the PC can disconnect and reconnect (e.g. to turn off a keyboard's lights).
struct UsbDevice: Decodable, Identifiable, Equatable {
    let name: String
    let icon: String
    let on: Bool
    let present: Bool

    var id: String { name }
}

/// A custom command button defined in the PC's config.
struct RemoteCommand: Decodable, Identifiable, Hashable {
    let id: String
    let name: String
    let icon: String
}

enum ClientMessage: Encodable {
    case hello(token: String, device: String)
    case move(dx: Double, dy: Double)
    case scroll(dx: Double, dy: Double)
    case button(MouseButton, ButtonAction)
    case text(String)
    case key(String, mods: [String] = [], state: KeyAction = .press)
    case media(MediaAction)
    case volume(VolumeAction, value: Double? = nil)
    case power(PowerAction)
    /// One monitor by connector name, or all of them when `monitor` is nil.
    case display(DisplayAction, monitor: String? = nil)
    /// Toggle (or set, with `on`) a configured USB device by name.
    case usb(String, on: Bool? = nil)
    case cmd(String)
    case refresh
    case ping

    /// Pausing on the PC drops everything except these.
    var allowedWhilePaused: Bool {
        switch self {
        case .refresh, .ping: return true
        default: return false
        }
    }

    static let protocolVersion = 1

    private enum CodingKeys: String, CodingKey {
        case t, token, device, v, dx, dy, b, s, k, mods, a, value, id, monitor, name, on
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
        case let .media(a):
            try c.encode("media", forKey: .t)
            try c.encode(a.rawValue, forKey: .a)
        case let .volume(a, value):
            try c.encode("volume", forKey: .t)
            try c.encode(a.rawValue, forKey: .a)
            try c.encodeIfPresent(value, forKey: .value)
        case let .power(a):
            try c.encode("power", forKey: .t)
            try c.encode(a.rawValue, forKey: .a)
        case let .display(a, monitor):
            try c.encode("display", forKey: .t)
            try c.encode(a.rawValue, forKey: .a)
            try c.encodeIfPresent(monitor, forKey: .monitor)
        case let .usb(name, on):
            try c.encode("usb", forKey: .t)
            try c.encode(name, forKey: .name)
            try c.encodeIfPresent(on, forKey: .on)
        case let .cmd(id):
            try c.encode("cmd", forKey: .t)
            try c.encode(id, forKey: .id)
        case .refresh:
            try c.encode("refresh", forKey: .t)
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
    let caps: [String]?
    let commands: [RemoteCommand]?
    let level: Double?
    let muted: Bool?
    let status: String?
    let title: String?
    let artist: String?
    let player: String?
    let monitors: [MonitorInfo]?
    let devices: [UsbDevice]?

    static func parse(_ text: String) -> ServerMessage? {
        try? JSONDecoder().decode(ServerMessage.self, from: Data(text.utf8))
    }
}
