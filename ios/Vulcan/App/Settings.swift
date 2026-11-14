import Foundation

/// User preferences (UserDefaults keys shared by @AppStorage and non-view code).
enum Settings {
    static let pointerSpeed = "pointerSpeed"
    static let scrollSpeed = "scrollSpeed"
    static let naturalScroll = "naturalScroll"
    static let tapToClick = "tapToClick"
    static let combos = "combos"
    static let manualHost = "manualHost"

    static let defaultCombos = "Ctrl+C\nCtrl+V\nCtrl+Z\nCtrl+Shift+T\nAlt+Tab\nSuper\nSuper+Q"

    static func register() {
        UserDefaults.standard.register(defaults: [
            pointerSpeed: 1.0,
            scrollSpeed: 1.0,
            naturalScroll: true,
            tapToClick: true,
            combos: defaultCombos,
            manualHost: "",
        ])
    }

    static var d: UserDefaults { .standard }
}

/// A key combination like "Ctrl+Shift+T" shown as a button in the keys bar.
struct Combo: Identifiable, Hashable {
    let id: String
    let key: String
    let mods: [String]

    private static let modNames: Set<String> = ["ctrl", "alt", "super", "shift", "altgr"]

    init?(_ text: String) {
        let parts = text.split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard let last = parts.last else { return nil }
        let mods = parts.dropLast().map { $0.lowercased() }
        guard mods.allSatisfy(Self.modNames.contains) else { return nil }
        id = text.trimmingCharacters(in: .whitespaces)
        key = last
        self.mods = Array(mods)
    }

    static func parseList(_ text: String) -> [Combo] {
        text.split(whereSeparator: \.isNewline).compactMap { Combo(String($0)) }
    }
}
