import Foundation
import Network

/// Whether the phone currently has Wi-Fi (or wired) networking. The PC is only reachable that
/// way; on mobile data alone, the app goes straight to Bluetooth.
@MainActor
final class NetworkMonitor {
    private(set) var onLocalNetwork = true
    var onChange: (() -> Void)?
    private let monitor = NWPathMonitor()

    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let local = path.status == .satisfied
                && path.availableInterfaces.contains { $0.type == .wifi || $0.type == .wiredEthernet }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, self.onLocalNetwork != local else { return }
                    self.onLocalNetwork = local
                    self.onChange?()
                }
            }
        }
        monitor.start(queue: .global(qos: .utility))
    }

    deinit {
        monitor.cancel()
    }
}
