use std::collections::HashMap;

use anyhow::{Context, Result};
use xkbcommon::xkb;

/// One key press that produces a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub shift: bool,
    pub altgr: bool,
}

/// Reverse XKB lookup: character -> key + modifiers for the active layout.
pub struct TextMapper {
    map: HashMap<char, Stroke>,
}

/// Keypad keys behave differently in some apps and depend on NumLock; avoid them.
fn is_keypad(code: u16) -> bool {
    matches!(code, 55 | 71..=83 | 96 | 98 | 117 | 118 | 121)
}

impl TextMapper {
    pub fn new(layout: &str, variant: &str, options: &str) -> Result<Self> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let opts = (!options.is_empty()).then(|| options.to_string());
        let keymap = xkb::Keymap::new_from_names(
            &ctx, "evdev", "pc105", layout, variant, opts, xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .with_context(|| format!("unknown XKB layout {layout:?} variant {variant:?}"))?;

        let shift = keymap.mod_get_index("Shift");
        let altgr = keymap.mod_get_index("Mod5");
        let has_altgr = altgr != xkb::MOD_INVALID;
        let mut state = xkb::State::new(&keymap);
        let mut map = HashMap::new();

        // Fewest modifiers first, so the simplest way to type a char wins.
        for (s, a) in [(false, false), (true, false), (false, true), (true, true)] {
            if a && !has_altgr {
                continue;
            }
            let mask = (if s { 1 << shift } else { 0 }) | (if a { 1 << altgr } else { 0 });
            state.update_mask(mask, 0, 0, 0, 0, 0);
            for code in 1u16..=248 {
                if is_keypad(code) {
                    continue;
                }
                let cp = state.key_get_utf32(xkb::Keycode::new(code as u32 + 8));
                let Some(ch) = char::from_u32(cp) else { continue };
                if cp == 0 || ch.is_control() {
                    continue;
                }
                map.entry(ch).or_insert(Stroke { code, shift: s, altgr: a });
            }
        }
        Ok(Self { map })
    }

    pub fn lookup(&self, c: char) -> Option<Stroke> {
        self.map.get(&c).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::KeyCode as K;

    fn s(code: K, shift: bool, altgr: bool) -> Option<Stroke> {
        Some(Stroke { code: code.0, shift, altgr })
    }

    #[test]
    fn gb_layout() {
        let m = TextMapper::new("gb", "", "").unwrap();
        assert_eq!(m.lookup('a'), s(K::KEY_A, false, false));
        assert_eq!(m.lookup('A'), s(K::KEY_A, true, false));
        assert_eq!(m.lookup('"'), s(K::KEY_2, true, false));
        assert_eq!(m.lookup('@'), s(K::KEY_APOSTROPHE, true, false));
        assert_eq!(m.lookup('#'), s(K::KEY_BACKSLASH, false, false));
        assert_eq!(m.lookup('~'), s(K::KEY_BACKSLASH, true, false));
        assert_eq!(m.lookup('\\'), s(K::KEY_102ND, false, false));
        assert_eq!(m.lookup('|'), s(K::KEY_102ND, true, false));
        assert_eq!(m.lookup('£'), s(K::KEY_3, true, false));
        assert_eq!(m.lookup('€'), s(K::KEY_4, false, true));
        assert_eq!(m.lookup('/'), s(K::KEY_SLASH, false, false));
        assert_eq!(m.lookup('*'), s(K::KEY_8, true, false));
        assert_eq!(m.lookup(' '), s(K::KEY_SPACE, false, false));
        assert_eq!(m.lookup('🙂'), None);
    }

    #[test]
    fn us_layout() {
        let m = TextMapper::new("us", "", "").unwrap();
        assert_eq!(m.lookup('@'), s(K::KEY_2, true, false));
        assert_eq!(m.lookup('"'), s(K::KEY_APOSTROPHE, true, false));
    }
}
