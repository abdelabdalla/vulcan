import Foundation
import Observation

/// Keeps one WebSocket open to the paired PC: tries the last known address, falls back to Bonjour,
/// reconnects with backoff, and coalesces pointer motion to one message per display frame.
@Observable
@MainActor
final class ConnectionManager {
    enum State: Equatable {
        case idle
        case connecting(String)
        case searching
        case connected(String)
        case waiting(String)
        /// The PC rejected our token; the user has to pair again.
        case unpaired(String)
    }

    private enum Outcome: Equatable {
        case unreachable
        case wasConnected
        case rejected(String)
    }

    private(set) var state: State = .idle
    private(set) var paused = false
    private(set) var toast: Toast?
    /// Capabilities and custom command buttons announced by the PC in `welcome`.
    private(set) var caps: Set<String> = []
    private(set) var commands: [RemoteCommand] = []
    private(set) var volume: VolumeState?
    private(set) var media: MediaState?
    private(set) var monitors: [MonitorInfo] = []
    private(set) var usbDevices: [UsbDevice] = []
    /// Clipboard auto-sync settings, owned by the PC (shared with its tray menu).
    private(set) var clipToPhone = true
    private(set) var clipToPC = true
    var onHostChanged: ((String) -> Void)?
    var onClip: ((ServerMessage) -> Void)?
    var onClipSettings: (() -> Void)?

    private(set) var pairing: Pairing?
    /// Address of the current connection, for HTTPS transfers.
    private(set) var activeHost: String?
    var onConnected: (() -> Void)?
    var onFileOffer: ((_ id: String, _ name: String) -> Void)?
    private var loop: Task<Void, Never>?
    private var socket: URLSessionWebSocketTask?
    private var session: URLSession?
    private var ticker: FrameTicker?
    private var pendingMove = (dx: 0.0, dy: 0.0)
    private var pendingScroll = (dx: 0.0, dy: 0.0)
    private var toastTask: Task<Void, Never>?

    var isConnected: Bool {
        if case .connected = state { return true }
        return false
    }

    // MARK: Lifecycle

    func start(with pairing: Pairing) {
        if loop != nil, self.pairing?.token == pairing.token { return }
        stop()
        self.pairing = pairing
        loop = Task { [weak self] in await self?.run() }
    }

    func stop() {
        loop?.cancel()
        loop = nil
        teardown()
        state = .idle
        paused = false
    }

    /// Retry immediately (e.g. from a "Reconnect" button).
    func reconnect() {
        guard let pairing else { return }
        stop()
        start(with: pairing)
    }

    private func run() async {
        var delay = 1.0
        while !Task.isCancelled, let pairing {
            let outcome = await attemptAll(pairing)
            if Task.isCancelled { return }
            switch outcome {
            case let .rejected(msg):
                state = .unpaired(msg == "pairing revoked" || msg == "not paired"
                    ? "This phone isn't paired with \(pairing.hostname) any more."
                    : msg)
                return
            case .wasConnected:
                delay = 1
                state = .waiting("Connection lost, retrying…")
            case .unreachable:
                state = .waiting("Can't reach \(pairing.hostname), retrying…")
            }
            try? await Task.sleep(for: .seconds(delay))
            delay = min(delay * 2, 8)
        }
    }

    private func attemptAll(_ p: Pairing) async -> Outcome {
        var hosts: [String] = []
        let manual = (Settings.d.string(forKey: Settings.manualHost) ?? "").trimmingCharacters(in: .whitespaces)
        if !manual.isEmpty { hosts.append(manual) }
        if !hosts.contains(p.host) { hosts.append(p.host) }

        for host in hosts {
            let outcome = await attempt(p, host: host)
            if outcome != .unreachable || Task.isCancelled { return outcome }
        }
        // The PC may have a new IP: look it up by fingerprint over Bonjour.
        state = .searching
        if let found = await Discovery.find(fingerprint: p.fingerprint), !hosts.contains(found), !Task.isCancelled {
            return await attempt(p, host: found)
        }
        return .unreachable
    }

    private func attempt(_ p: Pairing, host: String) async -> Outcome {
        state = .connecting(host)
        guard let url = URL(string: "wss://\(host):\(p.port)/ws") else { return .unreachable }

        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 5
        config.waitsForConnectivity = false
        let session = URLSession(
            configuration: config,
            delegate: PinningDelegate(fingerprint: p.fingerprint),
            delegateQueue: nil
        )
        let ws = session.webSocketTask(with: url)
        ws.maximumMessageSize = 32 << 20 // matches the PC; clipboard images can be up to 20 MB
        self.session = session
        socket = ws
        ws.resume()
        defer {
            if socket === ws { teardown() }
        }

        do {
            try await ws.send(.string(ClientMessage.hello(token: p.token, device: p.deviceName).json))
            let first = await Self.receive(ws, timeout: 5)
            guard case let .string(text)? = first, let msg = ServerMessage.parse(text) else { return .unreachable }
            if msg.t == "error" { return .rejected(msg.msg ?? "Rejected by the PC") }
            guard msg.t == "welcome" else { return .unreachable }
            caps = Set(msg.caps ?? [])
            commands = msg.commands ?? []
        } catch {
            return .unreachable
        }
        if Task.isCancelled { return .unreachable }

        state = .connected(p.hostname)
        activeHost = host
        onConnected?()
        paused = false
        if host != p.host { onHostChanged?(host) }
        startTicker()

        let keepalive = Task { [weak self] in await self?.keepAlive(ws) }
        defer { keepalive.cancel() }

        do {
            while true {
                let message = try await ws.receive()
                if case let .string(text) = message, let msg = ServerMessage.parse(text), let fatal = handle(msg) {
                    return .rejected(fatal)
                }
            }
        } catch {
            return .wasConnected
        }
    }

    /// Returns a message if the PC ended our pairing.
    private func handle(_ msg: ServerMessage) -> String? {
        switch msg.t {
        case "volume":
            if let level = msg.level { volume = VolumeState(level: level, muted: msg.muted ?? false) }
        case "media":
            media = MediaState(
                status: msg.status ?? "none",
                title: msg.title ?? "",
                artist: msg.artist ?? "",
                player: msg.player ?? ""
            )
        case "file_offer":
            if let id = msg.id, let name = msg.name { onFileOffer?(id, name) }
        case "clip":
            onClip?(msg)
        case "clip_settings":
            clipToPhone = msg.toPhone ?? clipToPhone
            clipToPC = msg.toPc ?? clipToPC
            onClipSettings?()
        case "usb_devices":
            usbDevices = msg.devices ?? []
        case "displays":
            monitors = msg.monitors ?? []
        case "info":
            if let text = msg.msg { show(toast: text, isError: false) }
        case "paused":
            paused = msg.paused ?? false
        case "error":
            let text = msg.msg ?? "Unknown error"
            if text == "pairing revoked" || text == "not paired" { return text }
            show(toast: text, isError: true)
        default:
            break
        }
        return nil
    }

    /// WebSocket pings every few seconds; a missing pong means the link is dead (Wi-Fi drop, PC asleep).
    private func keepAlive(_ ws: URLSessionWebSocketTask) async {
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(4))
            if Task.isCancelled { return }
            if await !Self.ping(ws) {
                ws.cancel(with: .goingAway, reason: nil)
                return
            }
        }
    }

    private nonisolated static func ping(_ ws: URLSessionWebSocketTask) async -> Bool {
        await withCheckedContinuation { continuation in
            let once = OnceFlag()
            ws.sendPing { error in
                if once.claim() { continuation.resume(returning: error == nil) }
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + 4) {
                if once.claim() { continuation.resume(returning: false) }
            }
        }
    }

    /// First frame with a deadline (nil on timeout or error).
    private nonisolated static func receive(
        _ ws: URLSessionWebSocketTask, timeout: Double
    ) async -> URLSessionWebSocketTask.Message? {
        await withCheckedContinuation { continuation in
            let once = OnceFlag()
            ws.receive { result in
                if once.claim() { continuation.resume(returning: try? result.get()) }
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                if once.claim() { continuation.resume(returning: nil) }
            }
        }
    }

    private func teardown() {
        ticker?.invalidate()
        ticker = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        session?.invalidateAndCancel()
        session = nil
        pendingMove = (0, 0)
        pendingScroll = (0, 0)
    }

    // MARK: Sending

    func queueMove(dx: Double, dy: Double) {
        pendingMove.dx += dx
        pendingMove.dy += dy
    }

    func queueScroll(dx: Double, dy: Double) {
        pendingScroll.dx += dx
        pendingScroll.dy += dy
    }

    /// Send immediately. Pending motion is flushed first so clicks land where the pointer is.
    func send(_ message: ClientMessage) {
        guard isConnected else { return }
        if paused, !message.allowedWhilePaused { return }
        flush()
        sendRaw(message)
    }

    private func startTicker() {
        ticker?.invalidate()
        ticker = FrameTicker { [weak self] _ in
            MainActor.assumeIsolated { self?.flush() }
        }
    }

    private func flush() {
        if pendingMove.dx != 0 || pendingMove.dy != 0 {
            sendRaw(.move(dx: pendingMove.dx, dy: pendingMove.dy))
            pendingMove = (0, 0)
        }
        if pendingScroll.dx != 0 || pendingScroll.dy != 0 {
            sendRaw(.scroll(dx: pendingScroll.dx, dy: pendingScroll.dy))
            pendingScroll = (0, 0)
        }
    }

    private func sendRaw(_ message: ClientMessage) {
        socket?.send(.string(message.json)) { _ in }
    }

    /// Change clipboard auto-sync; updated locally at once, confirmed by the PC.
    func setClipSync(toPhone: Bool? = nil, toPC: Bool? = nil) {
        if let toPhone { clipToPhone = toPhone }
        if let toPC { clipToPC = toPC }
        send(.clipSettings(toPhone: toPhone, toPC: toPC))
    }

    func showInfo(_ text: String) {
        show(toast: text, isError: false)
    }

    func showError(_ text: String) {
        show(toast: text, isError: true)
    }

    private func show(toast text: String, isError: Bool) {
        toast = Toast(text: text, isError: isError)
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            if !Task.isCancelled { self?.toast = nil }
        }
    }
}

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
