import SwiftUI

@main
struct VulcanApp: App {
    @State private var model: AppModel

    init() {
        Settings.register()
        _model = State(initialValue: AppModel())
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(.dark)
                .tint(Theme.accent)
                // Pairing links scanned with the system Camera app open here.
                .onOpenURL { model.pair(with: $0) }
        }
    }
}

enum Theme {
    static let accent = Color(red: 0xa6 / 255, green: 0xe3 / 255, blue: 0xa1 / 255)
    static let warning = Color(red: 0xf9 / 255, green: 0xe2 / 255, blue: 0xaf / 255)
    static let surface = Color(red: 0x1e / 255, green: 0x1e / 255, blue: 0x2e / 255)
}
