# Vulcan for iPhone

The SwiftUI app that drives `vulcand` on the PC. It needs iOS 17 or later.

## Build and install with a free Apple ID

You need a Mac with Xcode. Do this once:

1. Install XcodeGen with `brew install xcodegen`.
2. In Xcode, open **Settings → Accounts** and add your Apple ID. This creates a "Personal Team".
3. Create `ios/Config/Local.xcconfig`. It's git-ignored, so regenerating the project won't overwrite it.
   ```
   DEVELOPMENT_TEAM = ABCDE12345            // your Personal Team ID
   PRODUCT_BUNDLE_IDENTIFIER = com.yourname.vulcan
   ```
   To find the team ID, pick the team in any project's Signing & Capabilities tab, then look at `DEVELOPMENT_TEAM` in Build Settings. You can also find it in Keychain Access, in the "Apple Development" certificate's Organizational Unit.
4. Connect the iPhone by USB and trust the Mac. On the phone, turn on **Settings → Privacy & Security → Developer Mode**, which needs a restart.

Each time you build:

```sh
cd ios
xcodegen            # generates Vulcan.xcodeproj from project.yml
open Vulcan.xcodeproj
```

Pick your iPhone as the run destination and press **Run**.

The first time the app installs, the phone asks you to trust the developer. Go to **Settings → General → VPN & Device Management** to do that.

Things to know about free signing:

- **The app expires after 7 days.** Run it from Xcode again to renew it. Your pairing is kept, because it lives in the Keychain.
- **Wireless installs work after the first USB install.** Enable this under Xcode → Window → Devices and Simulators → Connect via network.
- **There's a limit of 3 sideloaded apps per device.**

To run the unit tests: `xcodebuild test -scheme Vulcan -destination 'platform=iOS Simulator,name=iPhone 16'`.

## Using it

1. **Pair.** On the PC, choose **Pair new device…** from the tray icon, or run `vulcand pair`. Scan the code from the app, or with the Camera app, which opens the app for you.
2. **Use the trackpad.**

   | Gesture | Action |
   |---|---|
   | Drag | Move the pointer |
   | Tap | Left click |
   | Two-finger tap | Right click |
   | Three-finger tap | Middle click |
   | Two-finger drag | Scroll (with momentum) |
   | Tap, then touch and drag | Click and drag |

   The L, M and R buttons can be held down, so you can drag with another finger.
3. **Type.** Tap the keyboard button. Typing goes straight to the PC, and backspace works.
   - The bar above the keyboard has Esc, Tab, arrows, Home/End/PgUp/PgDn, F1–F12 and your shortcut buttons. You edit the shortcuts in Settings.
   - **Ctrl, Alt, Super and Shift are sticky.** Tap once to apply to the next key. Tap twice to lock. Tap again to release.

The PC is identified by its certificate fingerprint, so if its IP changes the app finds it again over Bonjour. If your router blocks multicast, set a manual address in Settings.

## Layout

| Folder | Contents |
|---|---|
| `Vulcan/App` | App entry, `AppModel`, pairing and Keychain storage, settings |
| `Vulcan/Connection` | WebSocket client with certificate pinning, Bonjour discovery, protocol messages |
| `Vulcan/Input` | UIKit trackpad gestures and the `UIKeyInput` keyboard capture |
| `Vulcan/Views` | SwiftUI screens |
