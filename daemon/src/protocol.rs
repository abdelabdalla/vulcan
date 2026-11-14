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
    Ping,
}

/// PC -> phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome { v: u32, host: String, device: String, caps: Vec<String> },
    Error { msg: String },
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
    }

    #[test]
    fn server_roundtrip() {
        let m = ServerMsg::Welcome { v: 1, host: "pc".into(), device: "iPhone".into(), caps: vec!["input".into()] };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.starts_with(r#"{"t":"welcome""#));
        assert_eq!(serde_json::from_str::<ServerMsg>(&s).unwrap(), m);
    }
}
