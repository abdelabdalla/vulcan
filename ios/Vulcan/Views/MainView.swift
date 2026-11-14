import SwiftUI

struct MainView: View {
    @Environment(AppModel.self) private var model
    @State private var keyboardUp = false
    @State private var showSettings = false
    @AppStorage(Settings.volumeButtons) private var volumeButtons = true
    @State private var showVolumeHUD = false
    @State private var hideVolumeHUD: Task<Void, Never>?
    @State private var tab = Mode.trackpad

    enum Mode {
        case trackpad, controls, files
    }

    private var conn: ConnectionManager { model.connection }

    var body: some View {
        VStack(spacing: 0) {
            StatusBar(showSettings: $showSettings, tab: $tab)

            switch tab {
            case .trackpad: trackpad
            case .controls: ControlsView()
            case .files: FilesView()
            }
        }
        .onChange(of: tab) { _, _ in keyboardUp = false }
        // PC volume after a volume-button press; the PC replies with the new level.
        .overlay(alignment: .center) {
            if showVolumeHUD {
                VolumeHUD(volume: conn.volume)
                    .transition(.scale(scale: 0.9).combined(with: .opacity))
                    .allowsHitTesting(false)
            }
        }
        .animation(.easeOut(duration: 0.2), value: showVolumeHUD)
        .overlay(alignment: .top) {
            if let toast = conn.toast {
                Text(toast.text)
                    .font(.footnote)
                    .foregroundStyle(toast.isError ? Color.white : Color.black)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(toast.isError ? Color.red.opacity(0.85) : Theme.accent, in: Capsule())
                    .padding(.top, 52)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(.default, value: conn.toast)
        .sheet(isPresented: $showSettings) { SettingsView() }
        // Hidden; turns the phone's volume buttons into PC volume ±5% while connected.
        .background(
            VolumeButtons(active: volumeButtons && conn.isConnected && !conn.paused) { up in
                conn.send(.volume(up ? .up : .down))
                flashVolumeHUD()
            }
            .frame(width: 1, height: 1)
            .allowsHitTesting(false)
        )
    }

    private func flashVolumeHUD() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        showVolumeHUD = true
        hideVolumeHUD?.cancel()
        hideVolumeHUD = Task {
            try? await Task.sleep(for: .seconds(1.5))
            if !Task.isCancelled { showVolumeHUD = false }
        }
    }

    @ViewBuilder
    private var trackpad: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 24, style: .continuous)
                .fill(Theme.surface)
            RoundedRectangle(cornerRadius: 24, style: .continuous)
                .strokeBorder(.white.opacity(0.08))
            TrackpadView(actions: TrackpadActions(
                move: { conn.queueMove(dx: $0, dy: $1) },
                scroll: { conn.queueScroll(dx: $0, dy: $1) },
                button: { conn.send(.button($0, $1)) }
            ))
            .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))

            if !conn.isConnected || conn.paused {
                OverlayMessage()
                    .allowsHitTesting(false)
            }
            // Invisible owner of the iOS keyboard.
            KeyboardHost(isActive: $keyboardUp, model: model)
                .frame(width: 1, height: 1)
                .opacity(0)
                .allowsHitTesting(false)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)

        ButtonRow(keyboardUp: $keyboardUp)
            .padding(12)

        // Part of the layout rather than the keyboard's inputAccessoryView: on iOS 26 SwiftUI
        // counted the accessory's height twice and left a gap above the keyboard.
        if keyboardUp {
            KeysBar(onHide: { keyboardUp = false })
        }
    }
}

private struct StatusBar: View {
    @Environment(AppModel.self) private var model
    @Binding var showSettings: Bool
    @Binding var tab: MainView.Mode

    var body: some View {
        let (color, text) = describe(model.connection.state, paused: model.connection.paused)
        HStack {
            Circle().fill(color).frame(width: 10, height: 10)
            Text(text).font(.subheadline).lineLimit(1)
            Spacer()
            Picker("Mode", selection: $tab) {
                Image(systemName: "hand.point.up.left").tag(MainView.Mode.trackpad)
                Image(systemName: "slider.horizontal.3").tag(MainView.Mode.controls)
                Image(systemName: "folder").tag(MainView.Mode.files)
            }
            .pickerStyle(.segmented)
            .frame(width: 150)
            Button { showSettings = true } label: {
                Image(systemName: "gearshape.fill").font(.title3)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private func describe(_ state: ConnectionManager.State, paused: Bool) -> (Color, String) {
        switch state {
        case .idle: return (.gray, "Not connected")
        case let .connecting(host): return (.orange, "Connecting to \(host)…")
        case .searching: return (.orange, "Looking for your PC…")
        case let .connected(name): return paused ? (Theme.warning, "\(name): input paused") : (Theme.accent, name)
        case let .waiting(reason): return (.orange, reason)
        case .unpaired: return (.red, "Not paired")
        }
    }
}

private struct OverlayMessage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 12) {
            switch model.connection.state {
            case let .unpaired(reason):
                Image(systemName: "link.badge.plus").font(.largeTitle)
                Text(reason).multilineTextAlignment(.center)
                Text("Open Settings ⚙︎ to pair again.").font(.footnote).foregroundStyle(.secondary)
            case .connected where model.connection.paused:
                Image(systemName: "pause.circle").font(.largeTitle).foregroundStyle(Theme.warning)
                Text("Input is paused from the PC's tray icon.")
            default:
                ProgressView()
                Text("Waiting for the PC…").foregroundStyle(.secondary)
            }
        }
        .padding(24)
    }
}

private struct ButtonRow: View {
    @Environment(AppModel.self) private var model
    @Binding var keyboardUp: Bool

    var body: some View {
        HStack(spacing: 10) {
            MouseButtonView(label: "L", button: .left)
            MouseButtonView(label: "M", button: .middle).frame(maxWidth: 64)
            MouseButtonView(label: "R", button: .right)
            Button {
                keyboardUp.toggle()
            } label: {
                Image(systemName: keyboardUp ? "keyboard.chevron.compact.down" : "keyboard")
                    .font(.title2)
                    .frame(width: 64, height: 56)
            }
            .buttonStyle(KeyStyle(highlight: keyboardUp))
        }
        .frame(height: 56)
    }
}

/// Press-and-hold mouse button: down on touch, up on release, so you can drag with a second finger.
private struct MouseButtonView: View {
    @Environment(AppModel.self) private var model
    let label: String
    let button: MouseButton
    @State private var pressed = false

    var body: some View {
        RoundedRectangle(cornerRadius: 14, style: .continuous)
            .fill(Color.white.opacity(pressed ? 0.3 : 0.12))
            .overlay(Text(label).font(.headline))
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in
                        guard !pressed else { return }
                        pressed = true
                        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                        model.connection.send(.button(button, .down))
                    }
                    .onEnded { _ in
                        pressed = false
                        model.connection.send(.button(button, .up))
                    }
            )
    }
}

/// Volume indicator shown after the phone's volume buttons change the PC volume.
private struct VolumeHUD: View {
    let volume: VolumeState?

    var body: some View {
        let level = volume?.level ?? 0
        let muted = volume?.muted ?? false
        VStack(spacing: 12) {
            Image(systemName: muted ? "speaker.slash.fill" : level < 0.34 ? "speaker.wave.1.fill" : level < 0.67 ? "speaker.wave.2.fill" : "speaker.wave.3.fill")
                .font(.system(size: 34))
                .contentTransition(.symbolEffect(.replace))
            Text("PC volume").font(.caption).foregroundStyle(.secondary)
            ProgressView(value: min(level, 1))
                .tint(muted ? .secondary : Theme.accent)
                .frame(width: 140)
                .animation(.easeOut(duration: 0.15), value: level)
            Text(volume.map { muted ? "Muted" : "\(Int(($0.level * 100).rounded()))%" } ?? "…")
                .font(.headline.monospacedDigit())
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 20)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}
