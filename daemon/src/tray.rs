use std::sync::Arc;

use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip, TrayMethods};
use tracing::{info, warn};

use crate::auth::DeviceStore;
use crate::pairing;
use crate::protocol::UsbInfo;
use crate::server::{AppState, Status};

/// StatusNotifierItem shown in Waybar's `tray` module.
pub fn spawn(st: Arc<AppState>) {
    tokio::spawn(async move {
        let tray = VulcanTray { status: st.status.borrow().clone(), devices: Vec::new(), usb: Vec::new(), st: st.clone() };
        // Waybar may not be up yet at login; wait for it instead of failing.
        let handle = match tray.assume_sni_available(true).spawn().await {
            Ok(h) => h,
            Err(e) => {
                warn!("tray icon unavailable: {e}");
                return;
            }
        };
        let mut rx = st.status.subscribe();
        while rx.changed().await.is_ok() {
            let s = rx.borrow_and_update().clone();
            if handle.update(|t| t.status = s).await.is_none() {
                break;
            }
        }
    });
}

struct VulcanTray {
    st: Arc<AppState>,
    status: Status,
    devices: Vec<String>,
    usb: Vec<UsbInfo>,
}

impl VulcanTray {
    fn reload_devices(&mut self) {
        self.devices = DeviceStore::load(&self.st.paths.devices())
            .map(|s| s.devices.into_iter().map(|d| d.name).collect())
            .unwrap_or_default();
    }

    fn state(&self) -> IconState {
        if self.status.paused {
            IconState::Paused
        } else if !self.status.connected.is_empty() {
            IconState::Connected
        } else {
            IconState::Idle
        }
    }

    fn summary(&self) -> String {
        let conn = if self.status.connected.is_empty() {
            "No phone connected".to_string()
        } else {
            format!("Connected: {}", self.status.connected.join(", "))
        };
        if self.status.paused { format!("{conn} (input paused)") } else { conn }
    }
}

impl ksni::Tray for VulcanTray {
    fn id(&self) -> String {
        "vulcan".into()
    }
    fn title(&self) -> String {
        "Vulcan".into()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        [22, 32, 48].into_iter().map(|s| draw_icon(s, self.state())).collect()
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "Vulcan".into(), description: self.summary(), ..Default::default() }
    }
    fn menu_about_to_show(&mut self) {
        self.reload_devices();
        self.usb = crate::usb::states(&self.st.cfg.usb);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let devices: Vec<MenuItem<Self>> = if self.devices.is_empty() {
            vec![StandardItem { label: "No paired devices".into(), enabled: false, ..Default::default() }.into()]
        } else {
            self.devices
                .iter()
                .map(|name| {
                    let n = name.clone();
                    StandardItem {
                        label: format!("Revoke “{name}”"),
                        icon_name: "edit-delete".into(),
                        activate: Box::new(move |t: &mut Self| {
                            let path = t.st.paths.devices();
                            match DeviceStore::load(&path).and_then(|mut s| {
                                s.revoke(&n);
                                s.save(&path)
                            }) {
                                Ok(()) => info!(device = %n, "revoked from tray"),
                                Err(e) => warn!("revoke failed: {e:#}"),
                            }
                            t.reload_devices();
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect()
        };

        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem { label: self.summary(), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Pause input".into(),
                checked: self.status.paused,
                activate: Box::new(|t: &mut Self| {
                    let p = !t.st.paused();
                    t.st.set_paused(p);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Clipboard: PC → phone automatically".into(),
                checked: self.status.runtime.clip_to_phone,
                activate: Box::new(|t: &mut Self| {
                    let v = !t.st.runtime().clip_to_phone;
                    t.st.set_clip_sync(Some(v), None);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Clipboard: phone → PC automatically".into(),
                checked: self.status.runtime.clip_to_pc,
                activate: Box::new(|t: &mut Self| {
                    let v = !t.st.runtime().clip_to_pc;
                    t.st.set_clip_sync(None, Some(v));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Send file to phone…".into(),
                icon_name: "document-send".into(),
                activate: Box::new(|t: &mut Self| crate::files::pick_and_send(t.st.clone())),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Pair new device…".into(),
                icon_name: "list-add".into(),
                activate: Box::new(|t: &mut Self| {
                    let st = &t.st;
                    match pairing::create(&st.paths, &st.cfg, &st.fingerprint, None)
                        .and_then(|p| pairing::open_qr_page(&p).map(|_| p))
                    {
                        Ok(p) => info!(device = %p.name, "pairing QR opened"),
                        Err(e) => warn!("pairing failed: {e:#}"),
                    }
                    t.reload_devices();
                }),
                ..Default::default()
            }
            .into(),
            SubMenu { label: "Devices".into(), submenu: devices, ..Default::default() }.into(),
        ];
        // Way back if a USB device was switched off from the phone.
        for dev in self.usb.iter().filter(|d| d.present) {
            let name = dev.name.clone();
            let on = dev.on;
            items.push(
                CheckmarkItem {
                    label: format!("{} connected", dev.name),
                    checked: on,
                    activate: Box::new(move |t: &mut Self| {
                        match crate::usb::set(&t.st.cfg.usb, &name, Some(!on)) {
                            Ok(msg) => info!("{msg} (tray)"),
                            Err(e) => warn!("USB toggle failed: {e:#}"),
                        }
                        t.usb = crate::usb::states(&t.st.cfg.usb);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.extend([
            StandardItem {
                label: "Open config".into(),
                icon_name: "document-edit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = std::process::Command::new("xdg-open").arg(t.st.paths.config()).spawn();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

#[derive(Clone, Copy, PartialEq)]
enum IconState {
    Idle,
    Connected,
    Paused,
}

/// Draw a small phone glyph: outline colour shows state, filled screen when connected,
/// pause bars when paused. ARGB32, network byte order.
fn draw_icon(size: i32, state: IconState) -> Icon {
    let (r, g, b) = match state {
        IconState::Idle => (0xcd, 0xd6, 0xf4),
        IconState::Connected => (0xa6, 0xe3, 0xa1),
        IconState::Paused => (0xf9, 0xe2, 0xaf),
    };
    let s = size as f32;
    let (w, h) = (s * 0.52, s * 0.88);
    let (cx, cy) = (s / 2.0, s / 2.0);
    let radius = s * 0.12;
    let stroke = (s / 11.0).max(1.5);

    // Signed distance to a rounded rectangle centred at (cx, cy).
    let rrect = |x: f32, y: f32, hw: f32, hh: f32, r: f32| {
        let qx = (x - cx).abs() - (hw - r);
        let qy = (y - cy).abs() - (hh - r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - r
    };

    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let d = rrect(x, y, w / 2.0, h / 2.0, radius);
            // Outline ring, anti-aliased.
            let mut a = (0.5 - (d.abs() - stroke / 2.0)).clamp(0.0, 1.0);
            let inside = d < -stroke / 2.0;
            if inside {
                let (sx, sy) = (x - cx, y - cy);
                match state {
                    IconState::Connected => a = a.max(0.45),
                    IconState::Paused => {
                        let bar = sy.abs() < h * 0.18 && (sx.abs() - w * 0.14).abs() < w * 0.08;
                        if bar {
                            a = 1.0;
                        }
                    }
                    IconState::Idle => {}
                }
                // Home indicator dot.
                let dot = (sx.powi(2) + (y - (cy + h * 0.36)).powi(2)).sqrt();
                a = a.max((0.5 - (dot - stroke * 0.7)).clamp(0.0, 1.0));
            }
            data.extend_from_slice(&[(a * 255.0) as u8, r, g, b]);
        }
    }
    Icon { width: size, height: size, data }
}
