import SwiftUI

/// Shown above the iOS keyboard while typing: special keys, sticky modifiers, F-keys and custom combos.
struct KeysBar: View {
    static let height: CGFloat = 100

    @Environment(AppModel.self) private var model
    @AppStorage(Settings.combos) private var combosText = Settings.defaultCombos
    @State private var showFKeys = false
    let onHide: () -> Void

    var body: some View {
        VStack(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    key("esc", "Escape")
                    key("tab", "Tab")
                    mod("ctrl", "ctrl")
                    mod("alt", "alt")
                    mod("super", "⌘ super")
                    mod("shift", "⇧ shift")
                    key("←", "Left")
                    key("↓", "Down")
                    key("↑", "Up")
                    key("→", "Right")
                    key("del", "Delete")
                    key("home", "Home")
                    key("end", "End")
                    key("pgup", "PageUp")
                    key("pgdn", "PageDown")
                }
                .padding(.horizontal, 6)
            }
            HStack(spacing: 6) {
                Button { onHide() } label: {
                    Image(systemName: "keyboard.chevron.compact.down")
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(KeyStyle(highlight: false))

                Button { showFKeys.toggle() } label: {
                    Text(showFKeys ? "combos" : "F1–12").frame(height: 36).padding(.horizontal, 8)
                }
                .buttonStyle(KeyStyle(highlight: showFKeys))

                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        if showFKeys {
                            ForEach(1...12, id: \.self) { n in key("F\(n)", "F\(n)") }
                        } else {
                            ForEach(Combo.parseList(combosText)) { c in
                                Button(c.id) { model.combo(c) }
                                    .buttonStyle(KeyStyle(highlight: false))
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 6)
        }
        .padding(.vertical, 6)
        .frame(height: Self.height)
        .frame(maxWidth: .infinity)
        .background(.bar)
    }

    private func key(_ label: String, _ name: String) -> some View {
        Button(label) { model.pressKey(name) }
            .buttonStyle(KeyStyle(highlight: false))
    }

    private func mod(_ name: String, _ label: String) -> some View {
        let state = model.modState(name)
        return Button { model.tapMod(name) } label: {
            HStack(spacing: 3) {
                Text(label)
                if state == .locked { Image(systemName: "lock.fill").font(.caption2) }
            }
        }
        .buttonStyle(KeyStyle(highlight: state != .off))
    }
}

struct KeyStyle: ButtonStyle {
    let highlight: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.callout, design: .rounded).weight(.medium))
            .lineLimit(1)
            .frame(minWidth: 36, minHeight: 36)
            .padding(.horizontal, 6)
            .foregroundStyle(highlight ? Color.black : Color.primary)
            .background(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(highlight ? Theme.accent : Color.white.opacity(configuration.isPressed ? 0.3 : 0.12))
            )
            .contentShape(Rectangle())
    }
}
