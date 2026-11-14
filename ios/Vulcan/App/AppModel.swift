import Foundation
import Observation

/// Sticky modifier state from the keys bar: tap = next key only, double-tap = locked.
enum ModState {
    case off, once, locked
}

@Observable
@MainActor
final class AppModel {
    private(set) var pairing: Pairing?
    let connection = ConnectionManager()
    private(set) var mods: [String: ModState] = [:]
    var pairingError: String?

    init() {
        pairing = PairingStore.load()
        connection.onHostChanged = { [weak self] host in self?.updateHost(host) }
    }

    // MARK: Pairing

    @discardableResult
    func pair(with url: URL) -> Bool {
        guard let p = Pairing(url: url) else {
            pairingError = "That isn't a Vulcan pairing code."
            return false
        }
        pairingError = nil
        PairingStore.save(p)
        pairing = p
        connection.stop()
        connection.start(with: p)
        return true
    }

    func unpair() {
        connection.stop()
        PairingStore.clear()
        pairing = nil
        mods = [:]
    }

    private func updateHost(_ host: String) {
        guard var p = pairing, p.host != host else { return }
        p.host = host
        PairingStore.save(p)
        pairing = p
    }

    // MARK: Lifecycle

    func resume() {
        if let pairing { connection.start(with: pairing) }
    }

    func suspend() {
        connection.stop()
    }

    // MARK: Keyboard

    var activeMods: [String] {
        ["ctrl", "alt", "super", "shift"].filter { (mods[$0] ?? .off) != .off }
    }

    func modState(_ name: String) -> ModState { mods[name] ?? .off }

    func tapMod(_ name: String) {
        switch modState(name) {
        case .off: mods[name] = .once
        case .once: mods[name] = .locked
        case .locked: mods[name] = .off
        }
    }

    private func consumeOnce() {
        for (k, v) in mods where v == .once { mods[k] = .off }
    }

    /// Text from the iOS keyboard. With sticky modifiers held, they wrap the keystroke (e.g. Ctrl + c).
    func typeText(_ s: String) {
        let held = activeMods
        if held.isEmpty {
            connection.send(.text(s))
            return
        }
        for m in held { connection.send(.key(m, state: .down)) }
        connection.send(.text(s))
        for m in held.reversed() { connection.send(.key(m, state: .up)) }
        consumeOnce()
    }

    func pressKey(_ name: String, extraMods: [String] = []) {
        let all = extraMods + activeMods.filter { !extraMods.contains($0) }
        connection.send(.key(name, mods: all))
        consumeOnce()
    }

    func combo(_ c: Combo) {
        pressKey(c.key, extraMods: c.mods)
    }
}
