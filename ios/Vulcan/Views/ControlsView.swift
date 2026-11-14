import SwiftUI
import UIKit

/// Media, volume, power and the custom command buttons from the PC's config.
struct ControlsView: View {
    @Environment(AppModel.self) private var model
    @State private var sliderValue = 0.0
    @State private var dragging = false
    @State private var lastVolumeSend = Date.distantPast
    @State private var confirming: PowerAction?

    private var conn: ConnectionManager { model.connection }

    var body: some View {
        ScrollView {
            VStack(spacing: 14) {
                nowPlaying
                volume
                if conn.caps.contains("power") || conn.caps.contains("display") { power }
                commands
            }
            .padding(12)
        }
        .disabled(!conn.isConnected || conn.paused)
        .opacity(conn.isConnected && !conn.paused ? 1 : 0.5)
        // Keep volume and now-playing fresh while this tab is visible.
        .task(id: conn.isConnected) {
            while !Task.isCancelled, conn.isConnected {
                conn.send(.refresh)
                try? await Task.sleep(for: .seconds(3))
            }
        }
        .onChange(of: conn.volume, initial: true) { _, new in
            if !dragging, let new { sliderValue = new.level }
        }
        .confirmationDialog(
            confirming.map { "\(title(for: $0)) the PC?" } ?? "",
            isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
            titleVisibility: .visible,
            presenting: confirming
        ) { action in
            Button(title(for: action), role: .destructive) { conn.send(.power(action)) }
        }
    }

    // MARK: Sections

    private var nowPlaying: some View {
        Card(title: "Now playing", icon: "music.note") {
            let media = conn.media
            VStack(alignment: .leading, spacing: 4) {
                if let media, media.hasPlayer {
                    Text(media.title.isEmpty ? "Unknown title" : media.title)
                        .font(.headline)
                        .lineLimit(2)
                    Text([media.artist, media.player].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else {
                    Text("Nothing playing").foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            HStack(spacing: 28) {
                RoundButton(icon: "backward.fill") { conn.send(.media(.previous)) }
                RoundButton(icon: media?.isPlaying == true ? "pause.fill" : "play.fill", large: true) {
                    conn.send(.media(.playPause))
                }
                RoundButton(icon: "forward.fill") { conn.send(.media(.next)) }
            }
            .frame(maxWidth: .infinity)
            .padding(.top, 6)
        }
    }

    private var volume: some View {
        Card(title: "Volume", icon: "speaker.wave.2.fill") {
            HStack(spacing: 12) {
                Button {
                    conn.send(.volume(.mute))
                } label: {
                    Image(systemName: conn.volume?.muted == true ? "speaker.slash.fill" : "speaker.fill")
                        .frame(width: 28)
                }
                Slider(value: $sliderValue, in: 0...1, onEditingChanged: { editing in
                    dragging = editing
                    if !editing { conn.send(.volume(.set, value: sliderValue)) }
                })
                .onChange(of: sliderValue) { _, value in
                    // Throttle while dragging; the final value is sent when the drag ends.
                    guard dragging, Date().timeIntervalSince(lastVolumeSend) > 0.1 else { return }
                    lastVolumeSend = Date()
                    conn.send(.volume(.set, value: value))
                }
                Text("\(Int((sliderValue * 100).rounded()))%")
                    .font(.callout.monospacedDigit())
                    .frame(width: 44, alignment: .trailing)
            }
            HStack {
                Button { conn.send(.volume(.down)) } label: { Label("−5%", systemImage: "minus") }
                Spacer()
                Button { conn.send(.volume(.up)) } label: { Label("+5%", systemImage: "plus") }
            }
            .buttonStyle(.bordered)
        }
    }

    private var power: some View {
        Card(title: "Power", icon: "power") {
            HStack(spacing: 10) {
                if conn.caps.contains("display") {
                    PowerButton(title: "Screen", icon: "display") { conn.send(.display(.toggle)) }
                }
                if conn.caps.contains("power") {
                    PowerButton(title: "Lock", icon: "lock.fill") { conn.send(.power(.lock)) }
                    PowerButton(title: "Restart", icon: "arrow.clockwise") { confirming = .reboot }
                    PowerButton(title: "Shut down", icon: "power", destructive: true) { confirming = .poweroff }
                }
            }
        }
    }

    private var commands: some View {
        Card(title: "Commands", icon: "terminal") {
            if conn.commands.isEmpty {
                Text("Add buttons on the PC in ~/.config/vulcan/config.toml under [[commands]], then run `systemctl --user restart vulcand`.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 10)], spacing: 10) {
                    ForEach(conn.commands) { command in
                        Button {
                            UIImpactFeedbackGenerator(style: .light).impactOccurred()
                            conn.send(.cmd(command.id))
                        } label: {
                            VStack(spacing: 6) {
                                Image(systemName: UIImage(systemName: command.icon) != nil ? command.icon : "terminal")
                                    .font(.title2)
                                Text(command.name)
                                    .font(.footnote)
                                    .lineLimit(2)
                                    .multilineTextAlignment(.center)
                            }
                            .frame(maxWidth: .infinity, minHeight: 72)
                        }
                        .buttonStyle(KeyStyle(highlight: false))
                    }
                }
            }
        }
    }

    private func title(for action: PowerAction) -> String {
        switch action {
        case .lock: return "Lock"
        case .suspend: return "Sleep"
        case .reboot: return "Restart"
        case .poweroff: return "Shut down"
        }
    }
}

private struct Card<Content: View>: View {
    let title: String
    let icon: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(title, systemImage: icon)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
    }
}

private struct RoundButton: View {
    let icon: String
    var large = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(large ? .title : .title3)
                .frame(width: large ? 64 : 48, height: large ? 64 : 48)
                .background(Circle().fill(large ? Theme.accent : Color.white.opacity(0.12)))
                .foregroundStyle(large ? Color.black : Color.primary)
        }
        .buttonStyle(.plain)
    }
}

private struct PowerButton: View {
    let title: String
    let icon: String
    var destructive = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: icon).font(.title3)
                Text(title).font(.caption)
            }
            .frame(maxWidth: .infinity, minHeight: 64)
            .foregroundStyle(destructive ? Color.red : Color.primary)
        }
        .buttonStyle(KeyStyle(highlight: false))
    }
}
