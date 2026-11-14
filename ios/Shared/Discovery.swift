import Foundation
import Network

/// Finds the paired PC on the LAN via Bonjour (`_vulcan._tcp`), matching the cert fingerprint
/// in its TXT record, then resolves it to an IPv4 address.
enum Discovery {
    static func find(fingerprint: String, timeout: TimeInterval = 4) async -> String? {
        guard let endpoint = await browse(fingerprint: fingerprint.lowercased(), timeout: timeout) else {
            return nil
        }
        return await resolve(endpoint, timeout: timeout)
    }

    private static func browse(fingerprint: String, timeout: TimeInterval) async -> NWEndpoint? {
        let params = NWParameters()
        params.includePeerToPeer = false
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: "_vulcan._tcp", domain: nil), using: params)

        return await withCheckedContinuation { continuation in
            let once = OnceFlag()
            @Sendable func finish(_ endpoint: NWEndpoint?) {
                guard once.claim() else { return }
                browser.cancel()
                continuation.resume(returning: endpoint)
            }
            browser.browseResultsChangedHandler = { results, _ in
                for result in results {
                    if case let .bonjour(txt) = result.metadata, txt["fp"]?.lowercased() == fingerprint {
                        finish(result.endpoint)
                        return
                    }
                }
            }
            browser.stateUpdateHandler = { state in
                if case .failed = state { finish(nil) }
            }
            browser.start(queue: .main)
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { finish(nil) }
        }
    }

    /// Open a throwaway TCP connection to the service to learn its IPv4 address.
    private static func resolve(_ endpoint: NWEndpoint, timeout: TimeInterval) async -> String? {
        let params = NWParameters.tcp
        if let ip = params.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
            ip.version = .v4
        }
        let connection = NWConnection(to: endpoint, using: params)

        return await withCheckedContinuation { continuation in
            let once = OnceFlag()
            @Sendable func finish(_ host: String?) {
                guard once.claim() else { return }
                connection.cancel()
                continuation.resume(returning: host)
            }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if case let .hostPort(host, _)? = connection.currentPath?.remoteEndpoint {
                        switch host {
                        case let .ipv4(addr): finish("\(addr)")
                        case let .name(name, _): finish(name)
                        default: finish(nil)
                        }
                    } else {
                        finish(nil)
                    }
                case .failed, .cancelled:
                    finish(nil)
                default:
                    break
                }
            }
            connection.start(queue: .main)
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { finish(nil) }
        }
    }
}
