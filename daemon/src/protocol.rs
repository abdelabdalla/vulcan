//! Wire protocol: JSON text frames over a TLS WebSocket, tagged by `"t"`.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ButtonState {
    Down,
    Up,
    Click,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyState {
    #[default]
    Press,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeAction {
    Up,
    Down,
    Mute,
    /// Absolute level in `value` (0.0–1.0).
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayAction {
    Off,
    On,
    /// Off if any targeted monitor is on, otherwise on.
    Toggle,
}

/// One monitor as the phone draws it: logical position and size in the desktop layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Connector name, e.g. "DP-1"; used to address the monitor.
    pub name: String,
    /// Friendly name: the `[displays]` alias from config, else the model.
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub on: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Lock,
    Suspend,
    Reboot,
    Poweroff,
}

/// A switchable USB device from the config (e.g. a keyboard whose lights can't be turned off).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsbInfo {
    pub name: String,
    pub icon: String,
    /// Connected (authorized). False while switched off.
    pub on: bool,
    /// Plugged in at all.
    pub present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    Text,
    Image,
}

/// A custom command from the config, as shown on the phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub id: String,
    pub name: String,
    pub icon: String,
}

/// Phone -> PC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Must be the first message; `token` comes from pairing.
    Hello { token: String, device: String, v: u32 },
    /// Relative pointer motion in phone points.
    Move { dx: f64, dy: f64 },
    /// Two-finger scroll in phone points; dy > 0 scrolls down.
    Scroll { dx: f64, dy: f64 },
    Button { b: MouseButton, s: ButtonState },
    /// Literal text typed on the phone keyboard.
    Text { s: String },
    /// A named key ("Escape", "F5", "BackSpace", "q") with optional modifiers.
    Key {
        k: String,
        #[serde(default)]
        mods: Vec<String>,
        #[serde(default)]
        s: KeyState,
    },
    Media { a: MediaAction },
    Volume {
        a: VolumeAction,
        #[serde(default)]
        value: Option<f64>,
    },
    Power { a: PowerAction },
    /// Turn monitors off or on without suspending: one by connector name, or all.
    Display {
        a: DisplayAction,
        #[serde(default)]
        monitor: Option<String>,
    },
    /// Connect, disconnect or (with no `on`) toggle a configured USB device by name.
    Usb {
        name: String,
        #[serde(default)]
        on: Option<bool>,
    },
    /// Put the phone's clipboard on the PC. Images are base64 in `data`.
    /// `auto` marks automatic sync, which the PC ignores when phone→PC auto is off.
    ClipSet {
        kind: ClipKind,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        mime: Option<String>,
        #[serde(default)]
        data: Option<String>,
        #[serde(default)]
        auto: bool,
    },
    /// Ask for the PC's current clipboard.
    ClipGet,
    /// Change the auto-sync settings (shared with the tray).
    ClipSettings {
        #[serde(default)]
        to_phone: Option<bool>,
        #[serde(default)]
        to_pc: Option<bool>,
    },
    /// Run a custom command from the config by id.
    Cmd { id: String },
    /// Ask for current volume and now-playing info.
    Refresh,
    Ping,
}

/// PC -> phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        v: u32,
        host: String,
        device: String,
        caps: Vec<String>,
        #[serde(default)]
        commands: Vec<CommandInfo>,
    },
    Error { msg: String },
    /// Non-error feedback, e.g. "Ran Workspace 2".
    Info { msg: String },
    Volume { level: f64, muted: bool },
    /// `status` is Playing / Paused / Stopped, or "none" when no player is running.
    Media { status: String, title: String, artist: String, player: String },
    Displays { monitors: Vec<MonitorInfo> },
    UsbDevices { devices: Vec<UsbInfo> },
    /// PC clipboard content. `hash` lets the phone skip content it already has.
    Clip {
        kind: ClipKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        hash: String,
    },
    ClipSettings { to_phone: bool, to_pc: bool },
    Paused { paused: bool },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_client() {
        let m: ClientMsg = serde_json::from_str(r#"{"t":"move","dx":1.5,"dy":-2}"#).unwrap();
        assert_eq!(m, ClientMsg::Move { dx: 1.5, dy: -2.0 });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"key","k":"q","mods":["super"]}"#).unwrap();
        assert_eq!(m, ClientMsg::Key { k: "q".into(), mods: vec!["super".into()], s: KeyState::Press });
        let m: ClientMsg =
            serde_json::from_str(r#"{"t":"button","b":"left","s":"down"}"#).unwrap();
        assert_eq!(m, ClientMsg::Button { b: MouseButton::Left, s: ButtonState::Down });
        assert!(serde_json::from_str::<ClientMsg>(r#"{"t":"rm_rf"}"#).is_err());
        let m: ClientMsg = serde_json::from_str(r#"{"t":"media","a":"play_pause"}"#).unwrap();
        assert_eq!(m, ClientMsg::Media { a: MediaAction::PlayPause });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"volume","a":"set","value":0.4}"#).unwrap();
        assert_eq!(m, ClientMsg::Volume { a: VolumeAction::Set, value: Some(0.4) });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"power","a":"poweroff"}"#).unwrap();
        assert_eq!(m, ClientMsg::Power { a: PowerAction::Poweroff });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"display","a":"toggle"}"#).unwrap();
        assert_eq!(m, ClientMsg::Display { a: DisplayAction::Toggle, monitor: None });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"display","a":"off","monitor":"DP-2"}"#).unwrap();
        assert_eq!(m, ClientMsg::Display { a: DisplayAction::Off, monitor: Some("DP-2".into()) });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"usb","name":"Keyboard light"}"#).unwrap();
        assert_eq!(m, ClientMsg::Usb { name: "Keyboard light".into(), on: None });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"clip_set","kind":"text","text":"hi","auto":true}"#).unwrap();
        assert_eq!(m, ClientMsg::ClipSet { kind: ClipKind::Text, text: Some("hi".into()), mime: None, data: None, auto: true });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"clip_settings","to_pc":false}"#).unwrap();
        assert_eq!(m, ClientMsg::ClipSettings { to_phone: None, to_pc: Some(false) });
        let m: ClientMsg = serde_json::from_str(r#"{"t":"cmd","id":"Workspace 2"}"#).unwrap();
        assert_eq!(m, ClientMsg::Cmd { id: "Workspace 2".into() });
    }

    #[test]
    fn server_roundtrip() {
        let m = ServerMsg::Welcome {
            v: 1,
            host: "pc".into(),
            device: "iPhone".into(),
            caps: vec!["input".into()],
            commands: vec![CommandInfo { id: "a".into(), name: "a".into(), icon: "terminal".into() }],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.starts_with(r#"{"t":"welcome""#));
        assert_eq!(serde_json::from_str::<ServerMsg>(&s).unwrap(), m);
    }
}
