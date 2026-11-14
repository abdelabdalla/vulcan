import SwiftUI
import UIKit

/// Invisible view that owns the iOS keyboard. UIKeyInput hands us every inserted string and every
/// backspace directly, with autocorrect and smart punctuation off, so typing maps 1:1 to the PC.
final class KeyCaptureView: UIView, UIKeyInput {
    var onText: ((String) -> Void)?
    var onBackspace: (() -> Void)?
    var accessory: UIView?

    override var canBecomeFirstResponder: Bool { true }
    override var inputAccessoryView: UIView? { accessory }

    // Always claim to have text so iOS sends backspace even when "empty".
    var hasText: Bool { true }
    func insertText(_ text: String) { onText?(text) }
    func deleteBackward() { onBackspace?() }

    // UITextInputTraits
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var inlinePredictionType: UITextInlinePredictionType = .no
    var keyboardType: UIKeyboardType = .default
    var keyboardAppearance: UIKeyboardAppearance = .dark
    var returnKeyType: UIReturnKeyType = .default
}

struct KeyboardHost: UIViewRepresentable {
    @Binding var isActive: Bool
    let model: AppModel

    func makeUIView(context: Context) -> KeyCaptureView {
        let view = KeyCaptureView()
        view.onText = { model.typeText($0) }
        view.onBackspace = { model.pressKey("BackSpace") }

        let bar = UIHostingController(
            rootView: KeysBar(onHide: { isActive = false }).environment(model)
        )
        bar.sizingOptions = .intrinsicContentSize
        bar.view.backgroundColor = .clear
        bar.view.frame = CGRect(x: 0, y: 0, width: 400, height: KeysBar.height)
        bar.view.autoresizingMask = [.flexibleWidth]
        context.coordinator.bar = bar
        view.accessory = bar.view
        return view
    }

    func updateUIView(_ view: KeyCaptureView, context: Context) {
        if isActive, !view.isFirstResponder {
            DispatchQueue.main.async { view.becomeFirstResponder() }
        } else if !isActive, view.isFirstResponder {
            view.resignFirstResponder()
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        /// Keeps the hosting controller alive for as long as its view is the accessory.
        var bar: UIViewController?
    }
}
