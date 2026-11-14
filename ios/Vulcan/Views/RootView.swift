import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            if model.pairing == nil {
                PairingView()
            } else {
                MainView()
            }
        }
        .onChange(of: scenePhase, initial: true) { _, phase in
            switch phase {
            case .active: model.resume()
            case .background: model.suspend()
            default: break
            }
        }
    }
}
