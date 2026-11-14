import SwiftUI
import UIKit

/// A one-finger scroll strip beside the trackpad, like a mouse wheel: drag down to scroll
/// down, right to scroll right. Ridges move under the finger, with a haptic tick every few
/// points, and a flick keeps scrolling with momentum.
struct ScrollWheel: View {
    enum Axis {
        case vertical, horizontal
    }

    let axis: Axis
    /// Scroll amount in phone points (dx, dy); dy > 0 scrolls down, dx > 0 scrolls right.
    let onScroll: (Double, Double) -> Void

    @AppStorage(Settings.scrollSpeed) private var scrollSpeed = 1.0
    @State private var offset: CGFloat = 0
    @State private var lastTranslation: CGFloat = 0
    @State private var touching = false
    @State private var momentum: Task<Void, Never>?
    @State private var tickAccumulator: CGFloat = 0
    private let ticker = UISelectionFeedbackGenerator()

    private static let ridgeSpacing: CGFloat = 10
    private static let tickDistance: CGFloat = 18

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        Canvas { context, size in
            // Ridges perpendicular to the scroll direction, shifted by the scroll offset.
            let length = axis == .vertical ? size.height : size.width
            let phase = offset.truncatingRemainder(dividingBy: Self.ridgeSpacing)
            var p = phase < 0 ? phase + Self.ridgeSpacing : phase
            while p < length {
                var path = Path()
                if axis == .vertical {
                    path.move(to: CGPoint(x: size.width * 0.25, y: p))
                    path.addLine(to: CGPoint(x: size.width * 0.75, y: p))
                } else {
                    path.move(to: CGPoint(x: p, y: size.height * 0.25))
                    path.addLine(to: CGPoint(x: p, y: size.height * 0.75))
                }
                // Fade ridges towards the ends so it reads as a rounded wheel.
                let edge = min(p, length - p) / (length / 2)
                context.stroke(path, with: .color(.white.opacity(0.08 + 0.22 * edge)), lineWidth: 1.5)
                p += Self.ridgeSpacing
            }
        }
        .background(shape.fill(Theme.surface))
        .overlay(shape.strokeBorder(touching ? Theme.accent.opacity(0.6) : .white.opacity(0.08)))
        .clipShape(shape)
        .contentShape(shape)
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    if !touching {
                        touching = true
                        momentum?.cancel()
                        lastTranslation = 0
                        ticker.prepare()
                    }
                    let t = axis == .vertical ? value.translation.height : value.translation.width
                    step(t - lastTranslation)
                    lastTranslation = t
                }
                .onEnded { value in
                    touching = false
                    let v = axis == .vertical ? value.velocity.height : value.velocity.width
                    startMomentum(v)
                }
        )
        .accessibilityLabel(axis == .vertical ? "Vertical scroll wheel" : "Horizontal scroll wheel")
    }

    private func step(_ delta: CGFloat) {
        guard delta != 0 else { return }
        offset += delta
        let amount = Double(delta) * scrollSpeed
        if axis == .vertical { onScroll(0, amount) } else { onScroll(amount, 0) }
        tickAccumulator += abs(delta)
        if tickAccumulator >= Self.tickDistance {
            tickAccumulator = 0
            ticker.selectionChanged()
        }
    }

    private func startMomentum(_ velocity: CGFloat) {
        guard abs(velocity) > 200 else { return }
        momentum = Task { @MainActor in
            var v = velocity
            let frame = 1.0 / 60.0
            while !Task.isCancelled, abs(v) > 30 {
                step(v * frame)
                v *= 0.94
                try? await Task.sleep(for: .milliseconds(16))
            }
        }
    }
}
