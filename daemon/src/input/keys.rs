use evdev::KeyCode as K;

/// Map a key name from the phone ("Escape", "F5", "a", "Left", ...) to an evdev key.
pub fn key_by_name(name: &str) -> Option<K> {
    let lower = name.to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Some(k) = single_char(c)
    {
        return Some(k);
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return fkey(n);
    }
    Some(match lower.as_str() {
        "escape" | "esc" => K::KEY_ESC,
        "tab" => K::KEY_TAB,
        "return" | "enter" => K::KEY_ENTER,
        "backspace" => K::KEY_BACKSPACE,
        "delete" | "del" => K::KEY_DELETE,
        "insert" => K::KEY_INSERT,
        "home" => K::KEY_HOME,
        "end" => K::KEY_END,
        "pageup" => K::KEY_PAGEUP,
        "pagedown" => K::KEY_PAGEDOWN,
        "left" => K::KEY_LEFT,
        "right" => K::KEY_RIGHT,
        "up" => K::KEY_UP,
        "down" => K::KEY_DOWN,
        "space" => K::KEY_SPACE,
        "capslock" => K::KEY_CAPSLOCK,
        "print" | "printscreen" => K::KEY_SYSRQ,
        "menu" => K::KEY_COMPOSE,
        "volumeup" => K::KEY_VOLUMEUP,
        "volumedown" => K::KEY_VOLUMEDOWN,
        "mute" => K::KEY_MUTE,
        "playpause" => K::KEY_PLAYPAUSE,
        "next" => K::KEY_NEXTSONG,
        "previous" | "prev" => K::KEY_PREVIOUSSONG,
        "brightnessup" => K::KEY_BRIGHTNESSUP,
        "brightnessdown" => K::KEY_BRIGHTNESSDOWN,
        _ => return modifier(&lower),
    })
}

/// Modifier names used in `mods` and sticky keys.
pub fn modifier(name: &str) -> Option<K> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => K::KEY_LEFTCTRL,
        "shift" => K::KEY_LEFTSHIFT,
        "alt" => K::KEY_LEFTALT,
        "super" | "meta" | "win" | "cmd" => K::KEY_LEFTMETA,
        "altgr" => K::KEY_RIGHTALT,
        _ => return None,
    })
}

fn fkey(n: u16) -> Option<K> {
    Some(match n {
        1..=10 => K(K::KEY_F1.0 + n - 1),
        11 => K::KEY_F11,
        12 => K::KEY_F12,
        13..=24 => K(K::KEY_F13.0 + n - 13),
        _ => return None,
    })
}

/// Physical key for a lowercase letter/digit (layout-independent positions on QWERTY-like layouts).
fn single_char(c: char) -> Option<K> {
    const LETTERS: [K; 26] = [
        K::KEY_A, K::KEY_B, K::KEY_C, K::KEY_D, K::KEY_E, K::KEY_F, K::KEY_G, K::KEY_H, K::KEY_I,
        K::KEY_J, K::KEY_K, K::KEY_L, K::KEY_M, K::KEY_N, K::KEY_O, K::KEY_P, K::KEY_Q, K::KEY_R,
        K::KEY_S, K::KEY_T, K::KEY_U, K::KEY_V, K::KEY_W, K::KEY_X, K::KEY_Y, K::KEY_Z,
    ];
    Some(match c {
        'a'..='z' => LETTERS[(c as u8 - b'a') as usize],
        '0' => K::KEY_0,
        '1'..='9' => K(K::KEY_1.0 + (c as u8 - b'1') as u16),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(key_by_name("Escape"), Some(K::KEY_ESC));
        assert_eq!(key_by_name("F5"), Some(K::KEY_F5));
        assert_eq!(key_by_name("f12"), Some(K::KEY_F12));
        assert_eq!(key_by_name("F13"), Some(K::KEY_F13));
        assert_eq!(key_by_name("q"), Some(K::KEY_Q));
        assert_eq!(key_by_name("Q"), Some(K::KEY_Q));
        assert_eq!(key_by_name("7"), Some(K::KEY_7));
        assert_eq!(key_by_name("super"), Some(K::KEY_LEFTMETA));
        assert_eq!(key_by_name("nonsense"), None);
    }
}
