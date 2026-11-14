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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Lock,
    Suspend,
    Reboot,
    Poweroff,
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
    /// Turn the monitors off or on without suspending.
    Display { a: DisplayAction },
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
        let m: ClientMsg = serde_json::from_str(r#"{"t":"display","a":"off"}"#).unwrap();
        assert_eq!(m, ClientMsg::Display { a: DisplayAction::Off });
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
