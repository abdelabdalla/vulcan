import SwiftUI
import UIKit

struct TrackpadActions {
    var move: (Double, Double) -> Void
    var scroll: (Double, Double) -> Void
    var button: (MouseButton, ButtonAction) -> Void
}

/// Laptop-style trackpad:
/// one-finger drag moves, tap = left click, two-finger tap = right click, three-finger tap = middle click,
/// two-finger drag scrolls (with momentum), tap then drag = click-and-drag.
struct TrackpadView: UIViewRepresentable {
    let actions: TrackpadActions

    func makeUIView(context: Context) -> TrackpadUIView {
        TrackpadUIView(actions: actions)
    }

    func updateUIView(_ view: TrackpadUIView, context: Context) {
        view.actions = actions
    }
}

final class TrackpadUIView: UIView {
    var actions: TrackpadActions

    /// A single tap's click is held back briefly so a following touch can turn it into a drag.
    private static let tapDragWindow: TimeInterval = 0.2
    private var pendingClick: DispatchWorkItem?
    private var dragArmed = false
    private var dragging = false
    private var momentum = CGPoint.zero
    private var momentumTicker: FrameTicker?
    private let haptic = UIImpactFeedbackGenerator(style: .light)

    private var pointerSpeed: Double { Settings.d.double(forKey: Settings.pointerSpeed) }
    private var scrollSpeed: Double { Settings.d.double(forKey: Settings.scrollSpeed) }
    private var natural: Bool { Settings.d.bool(forKey: Settings.naturalScroll) }
    private var tapToClick: Bool { Settings.d.bool(forKey: Settings.tapToClick) }

    init(actions: TrackpadActions) {
        self.actions = actions
        super.init(frame: .zero)
        isMultipleTouchEnabled = true
        backgroundColor = .clear

        let pan1 = UIPanGestureRecognizer(target: self, action: #selector(onePan(_:)))
        pan1.minimumNumberOfTouches = 1
        pan1.maximumNumberOfTouches = 1
        let pan2 = UIPanGestureRecognizer(target: self, action: #selector(twoPan(_:)))
        pan2.minimumNumberOfTouches = 2
        pan2.maximumNumberOfTouches = 2

        let tap1 = makeTap(touches: 1, action: #selector(oneTap))
        let tap2 = makeTap(touches: 2, action: #selector(twoTap))
        let tap3 = makeTap(touches: 3, action: #selector(threeTap))
        tap2.require(toFail: tap3)

        for g in [pan1, pan2, tap1, tap2, tap3] { addGestureRecognizer(g) }
        haptic.prepare()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func makeTap(touches: Int, action: Selector) -> UITapGestureRecognizer {
        let tap = UITapGestureRecognizer(target: self, action: action)
        tap.numberOfTouchesRequired = touches
        tap.delaysTouchesEnded = false
        return tap
    }

    // MARK: Touch bookkeeping (tap-then-drag)

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesBegan(touches, with: event)
        stopMomentum()
        // Finger down again right after a tap: maybe the start of a drag.
        if let pending = pendingClick, event?.allTouches?.count == 1 {
            pending.cancel()
            pendingClick = nil
            dragArmed = true
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesEnded(touches, with: event)
        settleArmedDrag()
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesCancelled(touches, with: event)
        settleArmedDrag()
    }

    /// If the touch after a tap neither dragged nor tapped, still deliver the held-back click.
    private func settleArmedDrag() {
        DispatchQueue.main.async { [weak self] in
            guard let self, self.dragArmed else { return }
            self.dragArmed = false
            self.click(.left)
        }
    }

    // MARK: Gestures

    @objc private func onePan(_ g: UIPanGestureRecognizer) {
        if g.state == .began, dragArmed {
            dragArmed = false
            dragging = true
            haptic.impactOccurred()
            actions.button(.left, .down)
        }
        if g.state == .began || g.state == .changed {
            let t = g.translation(in: self)
            g.setTranslation(.zero, in: self)
            actions.move(t.x * pointerSpeed, t.y * pointerSpeed)
        }
        if [.ended, .cancelled, .failed].contains(g.state), dragging {
            dragging = false
            actions.button(.left, .up)
        }
    }

    @objc private func twoPan(_ g: UIPanGestureRecognizer) {
        let sign = natural ? -1.0 : 1.0
        switch g.state {
        case .began, .changed:
            let t = g.translation(in: self)
            g.setTranslation(.zero, in: self)
            actions.scroll(sign * t.x * scrollSpeed, sign * t.y * scrollSpeed)
        case .ended:
            let v = g.velocity(in: self)
            startMomentum(CGPoint(x: sign * v.x * scrollSpeed, y: sign * v.y * scrollSpeed))
        default:
            break
        }
    }

    @objc private func oneTap() {
        guard tapToClick else { return }
        if dragArmed {
            // Second tap of a double-tap: send the held-back click plus this one.
            dragArmed = false
            click(.left)
            click(.left)
            return
        }
        let work = DispatchWorkItem { [weak self] in
            self?.pendingClick = nil
            self?.click(.left)
        }
        pendingClick = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.tapDragWindow, execute: work)
    }

    @objc private func twoTap() {
        guard tapToClick else { return }
        click(.right)
    }

    @objc private func threeTap() {
        guard tapToClick else { return }
        click(.middle)
    }

    private func click(_ b: MouseButton) {
        haptic.impactOccurred()
        actions.button(b, .click)
    }

    // MARK: Scroll momentum

    private func startMomentum(_ velocity: CGPoint) {
        stopMomentum()
        guard hypot(velocity.x, velocity.y) > 150 else { return }
        momentum = velocity
        momentumTicker = FrameTicker { [weak self] dt in
            MainActor.assumeIsolated { self?.stepMomentum(dt) }
        }
    }

    private func stepMomentum(_ dt: CFTimeInterval) {
        let dt = dt > 0 ? dt : 1.0 / 60
        actions.scroll(momentum.x * dt, momentum.y * dt)
        let decay = pow(0.04, dt) // ~4% of the speed left after one second
        momentum = CGPoint(x: momentum.x * decay, y: momentum.y * decay)
        if hypot(momentum.x, momentum.y) < 25 { stopMomentum() }
    }

    private func stopMomentum() {
        momentumTicker?.invalidate()
        momentumTicker = nil
    }
}
