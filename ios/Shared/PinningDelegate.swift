import CryptoKit
import Foundation

/// Trusts exactly one certificate: the PC's self-signed cert whose SHA-256 came from the pairing QR code.
final class PinningDelegate: NSObject, URLSessionDelegate, URLSessionTaskDelegate {
    private let fingerprint: String

    init(fingerprint: String) {
        self.fingerprint = fingerprint.lowercased()
    }

    func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        evaluate(challenge, completionHandler)
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        evaluate(challenge, completionHandler)
    }

    private func evaluate(
        _ challenge: URLAuthenticationChallenge,
        _ completion: (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let leaf = chain.first
        else {
            completion(.cancelAuthenticationChallenge, nil)
            return
        }
        let der = SecCertificateCopyData(leaf) as Data
        let digest = SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
        if digest == fingerprint {
            completion(.useCredential, URLCredential(trust: trust))
        } else {
            completion(.cancelAuthenticationChallenge, nil)
        }
    }
}
