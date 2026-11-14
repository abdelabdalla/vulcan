import Foundation

/// Lets exactly one of several racing callbacks (result vs. timeout) win.
final class OnceFlag: @unchecked Sendable {
    private var done = false
    private let lock = NSLock()

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if done { return false }
        done = true
        return true
    }
}

struct Toast: Equatable {
    let text: String
    let isError: Bool
}

struct VolumeState: Equatable {
    let level: Double
    let muted: Bool
}

struct MediaState: Equatable {
    /// Playing / Paused / Stopped, or "none" when no player is running.
    let status: String
    let title: String
    let artist: String
    let player: String

    var hasPlayer: Bool { status != "none" }
    var isPlaying: Bool { status == "Playing" }
}
