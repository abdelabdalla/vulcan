# Vulcan

Vulcan lets you control a Linux PC (running Hyprland on Wayland) from an iPhone over your local network.

| Part | What it is |
|---|---|
| `daemon/` | `vulcand`, the Rust service on the PC. It injects mouse and keyboard input through uinput and serves a TLS WebSocket. It advertises itself over mDNS and puts a tray icon in Waybar. |
| `ios/` | The SwiftUI iPhone app (coming next). |

## PC setup

```sh
cargo install --path daemon --locked --root ~/.local   # -> ~/.local/bin/vulcand
mkdir -p ~/.config/systemd/user
cp daemon/vulcand.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now vulcand
```

The service starts with your graphical session, and a phone icon appears in Waybar's `tray` module:

| Icon colour | Meaning |
|---|---|
| Grey | Idle |
| Green | A phone is connected |
| Amber | Input is paused |

The tray menu has **Pause input**, **Pair new device…**, **Devices** (to revoke a phone), **Open config** and **Quit**.

### Requirements

- **Write access to `/dev/uinput`.** On systemd setups the logged-in user usually has this through a `uaccess` ACL. Check with `getfacl /dev/uinput`.
- **`libxkbcommon`.** Hyprland already depends on it.

## Pairing

Run `vulcand pair`, or use **Pair new device…** in the tray, and scan the QR code with the app.

The QR code contains:

- The PC's address and port.
- A random 256-bit token. Only its SHA-256 hash is stored, in `~/.config/vulcan/devices.toml`.
- The SHA-256 fingerprint of the daemon's self-signed certificate. The app pins this fingerprint, so traffic is encrypted and the phone only talks to this PC.

The PC is identified by that fingerprint, not by its IP, so IP changes need no re-pairing. The app finds the PC again over mDNS (`_vulcan._tcp`).

```sh
vulcand devices          # list paired phones
vulcand revoke "iPhone"  # unpair (an active session is dropped within 5 s)
```

## Config

The config lives at `~/.config/vulcan/config.toml` and is created on first run:

| Key | Default | What it controls |
|---|---|---|
| `port` | 47800 | The port the daemon listens on |
| `keyboard.layout` | `auto` | The XKB layout used to turn characters into key presses. `auto` reads it from Hyprland. |
| `pointer.sensitivity` | 1.5 | Pointer speed |
| `pointer.acceleration` | 0.4 | Extra speed for fast swipes |
| `pointer.scroll_speed` | 4.0 | Scroll speed |
| `pointer.cursor_boost_size` | 64 | Cursor size while the phone moves the pointer, so it's easy to find from across the room. `0` turns this off. |
| `pointer.cursor_boost_ms` | 2000 | How long the cursor stays big after the last movement |
| `pointer.cursor_size` | 24 | The normal cursor size to return to |
| `power.enabled` | `true` | Allow lock, sleep, restart and shut down from the phone |
| `power.lock` | `loginctl lock-session` | The command that locks the screen. The default needs hypridle running, because hypridle's `lock_cmd` is what starts hyprlock. |
| `[displays]` | none | Friendly monitor names for the phone, keyed by connector name, e.g. `DP-2 = "Left"` |
| `[[usb]]` | none | USB devices the phone can disconnect and reconnect. Each has a `name`, an `id` (`vendor:product` from `lsusb`) and an optional `icon`. |
| `[[commands]]` | none | Buttons on the phone's Controls tab. Each has a `name`, a `run` command and an optional `icon` (an SF Symbol name). |

Custom commands run through `sh -c`, and only the commands listed in the config can run. Start apps through Hyprland, so that Hyprland owns them rather than vulcand. With the Lua config:

```toml
[[commands]]
name = "VS Code"
icon = "chevron.left.forwardslash.chevron.right"
run = "hyprctl dispatch 'hl.dsp.exec_cmd(\"code\")'"
```

### Clipboard sync

Text and images sync between the PC and the phone while the Vulcan app is open. There are two automatic directions, toggled from the phone's Controls tab or the tray menu:

| Setting | What it does |
|---|---|
| PC → phone | Each new PC copy goes to the phone's clipboard. Via Universal Clipboard, it can then be pasted on a Mac too. |
| Phone → PC | A phone copy goes to the PC when the app opens, or as soon as the phone's clipboard changes. iOS asks "Allow Paste" each time unless you set **Settings → Vulcan → Paste from Other Apps → Allow**. |

Both are on by default. The setting is stored in `~/.config/vulcan/state.toml`.

Two manual actions always work, whatever the toggles say:

- **Paste** is the system paste button, so it doesn't trigger a prompt.
- **Get from PC** fetches the PC's current clipboard.

Content is identified by hash, so a copy never bounces back to where it came from. The PC side uses `wl-paste --watch` and `wl-copy`. Images over 20 MB don't sync.

### File transfer

Transfers use HTTPS on the same port, certificate and pairing token as the WebSocket. They only run while the app is open.

**Phone → PC:** files land in `~/Downloads/Vulcan`, and a name that's already taken gets ` (1)` added. A desktop notification has a **Show in folder** button. To send:

- use the app's Files tab (Photos or Files picker)
- or choose **Share → Vulcan** from any app

**PC → phone:** queue files in any of these ways:

- right-click files in Dolphin → **Send to iPhone**. Install this once with `vulcand install-dolphin`.
- the tray's **Send file to phone…**
- `vulcand send <files>`

Queued files download automatically when the app is open, and stay queued for 24 hours otherwise. On the phone:

- images and videos go to Photos
- everything else goes to **Files › On My iPhone › Vulcan › Received**

**Browsing:** the Files tab can browse your home folder and download from it. It's read-only. Images and videos show previews, which the PC takes from Dolphin's thumbnail cache or renders with ImageMagick or ffmpeg. **Select** lets you pick several files and folders at once. A folder downloads whole into **Files › Vulcan › Received**, keeping its structure. Hidden files and folders are listed only when **Show hidden** is on. Paths that lead outside home, including through symlinks, are refused.

### iPhone notifications on the PC

iPhone notifications appear as desktop notifications through Apple's notification protocol (ANCS) over Bluetooth LE, the same way a smartwatch gets them. This needs no app: it works whenever the phone is in Bluetooth range.

To pair once:

1. In the tray, choose **Pair iPhone for notifications…**. This makes the PC pairable and discoverable for 2 minutes.
2. On the iPhone, go to **Settings → Bluetooth** and tap the PC.
3. Accept the code and allow notifications.
4. Tap ⓘ next to the PC and turn on **Share System Notifications**.

After that, the phone reconnects by itself. Notifications that were already on the phone when it connected are skipped. Dismissing a notification on the phone closes it on the PC.

To turn this off, set `[notifications] enabled = false` in the config.

### Bluetooth fallback

When the phone can't reach the PC over Wi-Fi, the app connects over Bluetooth LE instead. It uses the bond made when pairing for notifications, so pair the iPhone from the tray first.

| Over Bluetooth | |
|---|---|
| Works | Trackpad, keyboard, all controls (media, volume, screens, USB devices, power, commands), and text clipboard up to 64 KB |
| Needs Wi-Fi | Files, browsing the PC, and clipboard images |

How the link works:

- **Protocol:** the same JSON messages as Wi-Fi, split into small packets and reassembled at the other end. Sessions still check the pairing token.
- **Switching back:** while on Bluetooth, the app checks for Wi-Fi every 20 seconds and switches back as soon as it can.
- **Turning it off:** on the PC, set `[bluetooth] remote = false` in the config. On the phone, use the Settings toggle.

### Switching USB devices off

Some devices' lights have no software control: the Cooler Master Quickfire TK keyboard, for example. Vulcan can disconnect such a device instead. Disconnecting deauthorizes the device and lets the kernel suspend it, which cuts it to standby power so its lights go off. Reconnecting brings it back.

This needs a one-time udev rule so vulcand can write the device's sysfs attributes without root. Add the devices under `[[usb]]` in the config, then run:

```sh
vulcand udev-rule | sudo tee /etc/udev/rules.d/70-vulcan-usb.rules
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=usb --action=add
```

If a device is left disconnected, there are three ways to get it back:

- the tray menu's "<name> connected" item
- the phone
- replugging the device or rebooting

## Protocol

The phone and PC exchange JSON text frames over `wss://<pc>:47800/ws`. The first frame must be `hello`.

| Direction | Messages |
|---|---|
| Phone → PC | `hello{token,device,v}`, `move{dx,dy}`, `scroll{dx,dy}`, `button{b,s}`, `text{s}`, `key{k,mods,s}`, `media{a}`, `volume{a,value?}`, `power{a}`, `display{a,monitor?}`, `usb{name,on?}`, `clip_set{kind,text?,mime?,data?,auto}`, `clip_get`, `clip_settings{to_phone?,to_pc?}`, `file_done{id}`, `cmd{id}`, `refresh`, `ping` |
| PC → phone | `welcome{caps,commands}`, `error{msg}`, `info{msg}`, `volume{level,muted}`, `media{status,title,artist,player}`, `displays{monitors}`, `usb_devices{devices}`, `clip{kind,text?,mime?,data?,hash}`, `clip_settings{to_phone,to_pc}`, `file_offer{id,name,size}`, `paused{paused}`, `pong` |

The HTTPS endpoints all need `Authorization: Bearer <token>`:

- `POST /files/upload?name=`
- `GET /files/<offer id>`
- `GET /fs/list?path=&hidden=`
- `GET /fs/download?path=&hidden=`
- `GET /fs/walk?path=&hidden=`
- `GET /fs/thumb?path=&hidden=`

See [`daemon/src/protocol.rs`](daemon/src/protocol.rs) for the full definitions.
