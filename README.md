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

## Protocol

The phone and PC exchange JSON text frames over `wss://<pc>:47800/ws`. The first frame must be `hello`.

| Direction | Messages |
|---|---|
| Phone → PC | `hello{token,device,v}`, `move{dx,dy}`, `scroll{dx,dy}`, `button{b,s}`, `text{s}`, `key{k,mods,s}`, `ping` |
| PC → phone | `welcome`, `error{msg}`, `paused{paused}`, `pong` |

See [`daemon/src/protocol.rs`](daemon/src/protocol.rs) for the full definitions.
