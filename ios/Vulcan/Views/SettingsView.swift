import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @AppStorage(Settings.pointerSpeed) private var pointerSpeed = 1.0
    @AppStorage(Settings.scrollSpeed) private var scrollSpeed = 1.0
    @AppStorage(Settings.naturalScroll) private var naturalScroll = true
    @AppStorage(Settings.tapToClick) private var tapToClick = true
    @AppStorage(Settings.scrollWheels) private var scrollWheels = true
    @AppStorage(Settings.volumeButtons) private var volumeButtons = true
    @AppStorage(Settings.combos) private var combos = Settings.defaultCombos
    @AppStorage(Settings.manualHost) private var manualHost = ""
    @AppStorage(Settings.bluetoothFallback) private var bluetoothFallback = true
    @State private var confirmUnpair = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Trackpad") {
                    LabeledContent("Pointer speed") {
                        Slider(value: $pointerSpeed, in: 0.3...3)
                    }
                    LabeledContent("Scroll speed") {
                        Slider(value: $scrollSpeed, in: 0.3...3)
                    }
                    Toggle("Natural scrolling", isOn: $naturalScroll)
                    Toggle("Tap to click", isOn: $tapToClick)
                    Toggle("Scroll wheels", isOn: $scrollWheels)
                }

                Section {
                    Toggle("Volume buttons control the PC", isOn: $volumeButtons)
                } footer: {
                    Text("While Vulcan is open and nothing is playing on the phone, its volume buttons change the PC's volume by 5%. When the phone plays audio, the buttons control the phone as usual.")
                }

                Section {
                    TextEditor(text: $combos)
                        .font(.system(.body, design: .monospaced))
                        .frame(minHeight: 140)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Shortcut buttons")
                } footer: {
                    Text("One per line, e.g. Ctrl+Shift+T or Super+Return. Modifiers: Ctrl, Alt, Super, Shift, AltGr.")
                }

                if let p = model.pairing {
                    Section {
                        LabeledContent("PC", value: p.hostname)
                        LabeledContent("Last address", value: "\(p.host):\(p.port)")
                        LabeledContent("This phone", value: p.deviceName)
                        LabeledContent("Certificate", value: String(p.fingerprint.prefix(16)) + "…")
                            .font(.footnote)
                        TextField("Manual address (optional)", text: $manualHost)
                            .keyboardType(.numbersAndPunctuation)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        Toggle("Bluetooth when Wi-Fi isn't available", isOn: $bluetoothFallback)
                        Button("Reconnect") { model.connection.reconnect() }
                    } header: {
                        Text("Connection")
                    } footer: {
                        Text("Vulcan finds your PC automatically if its IP changes. Set a manual address only if your router blocks discovery.")
                    }

                    Section {
                        Button("Unpair this phone", role: .destructive) { confirmUnpair = true }
                    }
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog("Unpair from this PC?", isPresented: $confirmUnpair, titleVisibility: .visible) {
                Button("Unpair", role: .destructive) {
                    model.unpair()
                    dismiss()
                }
            } message: {
                Text("You'll need to scan a new code to connect again. To fully revoke access, also remove this phone from the PC's tray menu.")
            }
        }
    }
}
