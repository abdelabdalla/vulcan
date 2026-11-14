import Foundation

/// HTTPS uploads to the PC's /files/upload, trusting only the paired certificate.
final class Uploader: @unchecked Sendable {
    private let pairing: Pairing
    private let session: URLSession
    private let lock = NSLock()
    private var current: URLSessionTask?

    init(pairing: Pairing) {
        self.pairing = pairing
        let config = URLSessionConfiguration.default
        // Mobile data never reaches a LAN address; fail fast instead.
        config.allowsCellularAccess = false
        session = URLSession(configuration: config, delegate: PinningDelegate(fingerprint: pairing.fingerprint), delegateQueue: nil)
    }

    deinit {
        session.invalidateAndCancel()
    }

    /// Any HTTP answer from the paired PC counts (the pinning delegate rejects anyone else).
    func reachable() async -> Bool {
        guard var req = request(path: "/") else { return false }
        req.timeoutInterval = 3
        return (try? await session.data(for: req)) != nil
    }

    func upload(_ file: URL, name: String, progress: @escaping @Sendable (Double) -> Void) async throws {
        guard var req = request(path: "/files/upload", query: [.init(name: "name", value: name)]) else {
            throw UploadError(message: "Bad PC address")
        }
        req.httpMethod = "POST"
        req.timeoutInterval = 30
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let observation = ObservationBox()
            let task = session.uploadTask(with: req, fromFile: file) { data, response, error in
                observation.value?.invalidate()
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let http = response as? HTTPURLResponse else {
                    continuation.resume(throwing: UploadError(message: "No response from the PC"))
                    return
                }
                guard (200..<300).contains(http.statusCode) else {
                    let message = data
                        .flatMap { try? JSONDecoder().decode([String: String].self, from: $0)["error"] }
                        ?? "The PC returned \(http.statusCode)"
                    continuation.resume(throwing: UploadError(message: message))
                    return
                }
                continuation.resume()
            }
            observation.value = task.progress.observe(\.fractionCompleted) { p, _ in progress(p.fractionCompleted) }
            lock.withLock { current = task }
            task.resume()
        }
    }

    func cancel() {
        lock.withLock { current }?.cancel()
    }

    private func request(path: String, query: [URLQueryItem] = []) -> URLRequest? {
        var comps = URLComponents()
        comps.scheme = "https"
        comps.host = pairing.host
        comps.port = pairing.port
        comps.path = path
        if !query.isEmpty { comps.queryItems = query }
        guard let url = comps.url else { return nil }
        var req = URLRequest(url: url)
        req.setValue("Bearer \(pairing.token)", forHTTPHeaderField: "Authorization")
        return req
    }
}

/// Holds the progress observation so the completion handler can end it.
private final class ObservationBox: @unchecked Sendable {
    var value: NSKeyValueObservation?
}

struct UploadError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}
