pub mod keys;
pub mod text;

use std::collections::HashSet;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

use crate::config::PointerConfig;
use crate::protocol::{ButtonState, KeyState, MouseButton};
use text::TextMapper;

const HI_RES_PER_NOTCH: i32 = 120;

/// Two uinput devices (a mouse and a keyboard) driven by phone messages.
pub struct Input {
    mouse: VirtualDevice,
    kbd: VirtualDevice,
    text: TextMapper,
    cfg: PointerConfig,
    move_rem: (f64, f64),
    scroll_rem: (f64, f64),
    wheel_acc: (i32, i32),
    held_keys: HashSet<u16>,
    held_buttons: HashSet<u16>,
}

fn ev(kind: EventType, code: u16, value: i32) -> InputEvent {
    InputEvent::new(kind.0, code, value)
}

impl Input {
    pub fn new(cfg: PointerConfig, text: TextMapper) -> Result<Self> {
        let mut buttons = AttributeSet::<KeyCode>::new();
        for b in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE] {
            buttons.insert(b);
        }
        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        for a in [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ] {
            axes.insert(a);
        }
        let mouse = VirtualDevice::builder()
            .context("opening /dev/uinput (is your user allowed to write it?)")?
            .name("Vulcan Mouse")
            .with_keys(&buttons)?
            .with_relative_axes(&axes)?
            .build()?;

        let mut keys = AttributeSet::<KeyCode>::new();
        for code in 1u16..=248 {
            keys.insert(KeyCode(code));
        }
        let kbd = VirtualDevice::builder()?.name("Vulcan Keyboard").with_keys(&keys)?.build()?;

        Ok(Self {
            mouse,
            kbd,
            text,
            cfg,
            move_rem: (0.0, 0.0),
            scroll_rem: (0.0, 0.0),
            wheel_acc: (0, 0),
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
        })
    }

    pub fn move_by(&mut self, dx: f64, dy: f64) -> Result<()> {
        let mag = dx.hypot(dy);
        let gain = self.cfg.sensitivity * (1.0 + self.cfg.acceleration * (mag.min(60.0) / 10.0));
        let x = dx * gain + self.move_rem.0;
        let y = dy * gain + self.move_rem.1;
        let (ix, iy) = (x.trunc(), y.trunc());
        self.move_rem = (x - ix, y - iy);
        if ix == 0.0 && iy == 0.0 {
            return Ok(());
        }
        self.mouse.emit(&[
            ev(EventType::RELATIVE, RelativeAxisCode::REL_X.0, ix as i32),
            ev(EventType::RELATIVE, RelativeAxisCode::REL_Y.0, iy as i32),
        ])?;
        Ok(())
    }

    /// dy > 0 scrolls down (content moves up), dx > 0 scrolls right.
    pub fn scroll(&mut self, dx: f64, dy: f64) -> Result<()> {
        let v = -dy * self.cfg.scroll_speed + self.scroll_rem.1;
        let h = dx * self.cfg.scroll_speed + self.scroll_rem.0;
        let (vi, hi) = (v.trunc(), h.trunc());
        self.scroll_rem = (h - hi, v - vi);
        let (vi, hi) = (vi as i32, hi as i32);

        let mut events = Vec::with_capacity(4);
        if vi != 0 {
            self.wheel_acc.1 += vi;
            let notches = self.wheel_acc.1 / HI_RES_PER_NOTCH;
            self.wheel_acc.1 -= notches * HI_RES_PER_NOTCH;
            events.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL_HI_RES.0, vi));
            if notches != 0 {
                events.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_WHEEL.0, notches));
            }
        }
        if hi != 0 {
            self.wheel_acc.0 += hi;
            let notches = self.wheel_acc.0 / HI_RES_PER_NOTCH;
            self.wheel_acc.0 -= notches * HI_RES_PER_NOTCH;
            events.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL_HI_RES.0, hi));
            if notches != 0 {
                events.push(ev(EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL.0, notches));
            }
        }
        if !events.is_empty() {
            self.mouse.emit(&events)?;
        }
        Ok(())
    }

    pub fn button(&mut self, b: MouseButton, s: ButtonState) -> Result<()> {
        let code = match b {
            MouseButton::Left => KeyCode::BTN_LEFT,
            MouseButton::Right => KeyCode::BTN_RIGHT,
            MouseButton::Middle => KeyCode::BTN_MIDDLE,
        }
        .0;
        match s {
            ButtonState::Down => self.set_button(code, true),
            ButtonState::Up => self.set_button(code, false),
            ButtonState::Click => {
                self.set_button(code, true)?;
                self.set_button(code, false)
            }
        }
    }

    fn set_button(&mut self, code: u16, down: bool) -> Result<()> {
        self.mouse.emit(&[ev(EventType::KEY, code, down as i32)])?;
        if down {
            self.held_buttons.insert(code);
        } else {
            self.held_buttons.remove(&code);
        }
        Ok(())
    }

    fn set_key(&mut self, code: u16, down: bool) -> Result<()> {
        self.kbd.emit(&[ev(EventType::KEY, code, down as i32)])?;
        if down {
            self.held_keys.insert(code);
        } else {
            self.held_keys.remove(&code);
        }
        Ok(())
    }

    /// Press `key` with `mods` held. Mods already held (e.g. via a sticky `down`) are left alone.
    pub fn key(&mut self, name: &str, mods: &[String], state: KeyState) -> Result<()> {
        let key = keys::key_by_name(name).with_context(|| format!("unknown key {name:?}"))?;
        let mut mod_codes = Vec::new();
        for m in mods {
            let k = keys::modifier(m).with_context(|| format!("unknown modifier {m:?}"))?;
            mod_codes.push(k.0);
        }
        match state {
            KeyState::Down => self.set_key(key.0, true),
            KeyState::Up => self.set_key(key.0, false),
            KeyState::Press => self.press(key.0, &mod_codes),
        }
    }

    fn press(&mut self, code: u16, mods: &[u16]) -> Result<()> {
        let fresh: Vec<u16> = mods.iter().copied().filter(|m| !self.held_keys.contains(m)).collect();
        for &m in &fresh {
            self.set_key(m, true)?;
        }
        self.set_key(code, true)?;
        self.set_key(code, false)?;
        for &m in fresh.iter().rev() {
            self.set_key(m, false)?;
        }
        Ok(())
    }

    /// Type a string using the layout's reverse map. Returns characters that couldn't be typed.
    pub fn type_text(&mut self, s: &str) -> Result<Vec<char>> {
        let mut skipped = Vec::new();
        for c in s.chars() {
            let (code, shift, altgr) = match c {
                '\n' | '\r' => (KeyCode::KEY_ENTER.0, false, false),
                '\t' => (KeyCode::KEY_TAB.0, false, false),
                '\u{8}' => (KeyCode::KEY_BACKSPACE.0, false, false),
                _ => match self.text.lookup(c) {
                    Some(st) => (st.code, st.shift, st.altgr),
                    None => {
                        skipped.push(c);
                        continue;
                    }
                },
            };
            let mut mods = Vec::with_capacity(2);
            if shift {
                mods.push(KeyCode::KEY_LEFTSHIFT.0);
            }
            if altgr {
                mods.push(KeyCode::KEY_RIGHTALT.0);
            }
            self.press(code, &mods)?;
        }
        Ok(skipped)
    }

    /// Release everything held, e.g. when a phone disconnects mid-drag.
    pub fn release_all(&mut self) {
        for code in self.held_buttons.clone() {
            let _ = self.set_button(code, false);
        }
        for code in self.held_keys.clone() {
            let _ = self.set_key(code, false);
        }
        self.move_rem = (0.0, 0.0);
        self.scroll_rem = (0.0, 0.0);
        self.wheel_acc = (0, 0);
    }
}
