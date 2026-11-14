import AVFoundation
import MediaPlayer
import SwiftUI

/// Turns the phone's volume buttons into PC volume up/down while active.
///
/// iOS has no API for the buttons themselves, so this watches the phone's output volume,
/// reports each change as a press, and puts the volume back to a midpoint (so both buttons
/// keep registering) using a hidden MPVolumeView, which also stops iOS showing its volume HUD.
/// The phone's own volume is restored when this stops. It also stands down while another app
/// is playing audio, so the buttons work normally for the phone's music or video.
struct VolumeButtons: UIViewRepresentable {
    let active: Bool
    let onPress: (_ up: Bool) -> Void

    func makeUIView(context: Context) -> MPVolumeView {
        let view = MPVolumeView(frame: CGRect(x: -200, y: -200, width: 100, height: 40))
        view.alpha = 0.0001
        view.isUserInteractionEnabled = false
        context.coordinator.volumeView = view
        return view
    }

    func updateUIView(_ view: MPVolumeView, context: Context) {
        context.coordinator.onPress = onPress
        context.coordinator.setWanted(active)
    }

    static func dismantleUIView(_ view: MPVolumeView, coordinator: Coordinator) {
        coordinator.setWanted(false)
    }

    func makeCoordinator() -> Coordinator { Coordinator(onPress: onPress) }

    @MainActor
    final class Coordinator {
        var onPress: (Bool) -> Void
        weak var volumeView: MPVolumeView?
        private var observation: NSKeyValueObservation?
        private var savedVolume: Float?
        private let midpoint: Float = 0.5

        init(onPress: @escaping (Bool) -> Void) { self.onPress = onPress }

        /// Whether the app wants capture (connected, enabled); actual capture also needs
        /// the phone to be silent.
        private var wanted = false
        private var timer: Timer?
        private var hintObserver: NSObjectProtocol?

        func setWanted(_ value: Bool) {
            guard value != wanted else { return }
            wanted = value
            if value {
                // Fires when other apps start or stop audio; the timer covers anything it misses.
                hintObserver = NotificationCenter.default.addObserver(
                    forName: AVAudioSession.silenceSecondaryAudioHintNotification, object: nil, queue: .main
                ) { [weak self] _ in
                    MainActor.assumeIsolated { self?.reevaluate() }
                }
                timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
                    MainActor.assumeIsolated { self?.reevaluate() }
                }
                reevaluate()
            } else {
                timer?.invalidate()
                timer = nil
                if let hintObserver { NotificationCenter.default.removeObserver(hintObserver) }
                hintObserver = nil
                stopCapture()
            }
        }

        private func reevaluate() {
            if wanted, !AVAudioSession.sharedInstance().isOtherAudioPlaying {
                startCapture()
            } else {
                stopCapture()
            }
        }

        private func startCapture() {
            guard observation == nil else { return }
            let session = AVAudioSession.sharedInstance()
            // Ambient + mix: don't interrupt anything the phone is playing.
            try? session.setCategory(.ambient, options: [.mixWithOthers])
            try? session.setActive(true)
            savedVolume = session.outputVolume
            observation = session.observe(\.outputVolume, options: [.new]) { [weak self] _, change in
                guard let volume = change.newValue else { return }
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { self?.changed(volume) }
                }
            }
            // The volume view's slider only works once it's in a window.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, self.observation != nil else { return }
                    self.setVolume(self.midpoint)
                }
            }
        }

        private func stopCapture() {
            guard observation != nil else { return }
            observation = nil
            if let savedVolume { setVolume(savedVolume) }
            savedVolume = nil
        }

        private func changed(_ volume: Float) {
            guard observation != nil else { return }
            // Our own reset to the midpoint.
            if abs(volume - midpoint) < 0.01 { return }
            onPress(volume > midpoint)
            setVolume(midpoint)
        }

        private func setVolume(_ volume: Float) {
            guard let slider = volumeView?.subviews.compactMap({ $0 as? UISlider }).first else { return }
            slider.setValue(volume, animated: false)
            slider.sendActions(for: .valueChanged)
        }
    }
}
