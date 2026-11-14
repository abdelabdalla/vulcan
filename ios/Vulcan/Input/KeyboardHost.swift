import SwiftUI
import UIKit

/// Invisible view that owns the iOS keyboard. UIKeyInput hands us every inserted string and every
/// backspace directly, with autocorrect and smart punctuation off, so typing maps 1:1 to the PC.
final class KeyCaptureView: UIView, UIKeyInput {
    var onText: ((String) -> Void)?
    var onBackspace: (() -> Void)?
    /// Called when the keyboard goes away, including when iOS dismisses it on its own.
    var onResign: (() -> Void)?

    override var canBecomeFirstResponder: Bool { true }

    @discardableResult
    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned { onResign?() }
        return resigned
    }

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
        view.onResign = { DispatchQueue.main.async { if isActive { isActive = false } } }
        return view
    }

    func updateUIView(_ view: KeyCaptureView, context: Context) {
        if isActive, !view.isFirstResponder {
            DispatchQueue.main.async { view.becomeFirstResponder() }
        } else if !isActive, view.isFirstResponder {
            view.resignFirstResponder()
        }
    }
}
