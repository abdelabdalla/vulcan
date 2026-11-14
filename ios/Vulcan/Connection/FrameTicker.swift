import QuartzCore

/// Calls `tick` once per display frame (up to 120 Hz on ProMotion screens) until invalidated.
final class FrameTicker: NSObject {
    private var link: CADisplayLink?
    private let tick: (CFTimeInterval) -> Void

    init(_ tick: @escaping (CFTimeInterval) -> Void) {
        self.tick = tick
        super.init()
        let link = CADisplayLink(target: self, selector: #selector(step))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func step(_ link: CADisplayLink) {
        tick(link.targetTimestamp - link.timestamp)
    }

    func invalidate() {
        link?.invalidate()
        link = nil
    }

    deinit {
        link?.invalidate()
    }
}
