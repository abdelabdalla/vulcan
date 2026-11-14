use std::path::PathBuf;

use anyhow::Result;
use qrcode::QrCode;
use qrcode::render::{svg, unicode};

use crate::auth::DeviceStore;
use crate::config::{Config, Paths};

pub struct Pairing {
    pub name: String,
    pub url: String,
}

/// Create a new device token and the `vulcan://pair?...` URL the phone scans.
pub fn create(paths: &Paths, cfg: &Config, fingerprint: &str, name: Option<&str>) -> Result<Pairing> {
    let mut store = DeviceStore::load(&paths.devices())?;
    let name = name.map(str::to_string).unwrap_or_else(|| store.next_name("iPhone"));
    let token = store.pair(&name);
    store.save(&paths.devices())?;

    let host = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| format!("{}.local", crate::util::hostname()));
    let url = format!(
        "vulcan://pair?h={host}&p={port}&t={token}&f={fingerprint}&n={hn}&d={dn}",
        port = cfg.port,
        hn = encode(&crate::util::hostname()),
        dn = encode(&name),
    );
    Ok(Pairing { name, url })
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn terminal_qr(url: &str) -> Result<String> {
    let code = QrCode::new(url.as_bytes())?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build())
}

/// Write an HTML page with the QR code and open it (used from the tray menu).
pub fn open_qr_page(p: &Pairing) -> Result<PathBuf> {
    let code = QrCode::new(p.url.as_bytes())?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(360, 360)
        .quiet_zone(true)
        .build();
    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><title>Pair Vulcan</title>
<style>body{{font-family:system-ui,sans-serif;background:#1e1e2e;color:#cdd6f4;display:grid;place-items:center;min-height:100vh;margin:0}}
.card{{text-align:center}} .qr{{background:#fff;padding:16px;border-radius:16px;display:inline-block}} p{{opacity:.8}}</style>
<div class="card"><h1>Pair “{name}”</h1><div class="qr">{svg}</div>
<p>Open Vulcan on your iPhone and scan this code.<br>It grants control of this PC — close this page once paired.</p></div>"#,
        name = html_escape(&p.name),
    );
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join("vulcan-pair.html");
    crate::util::write_private(&path, html.as_bytes())?;
    std::process::Command::new("xdg-open").arg(&path).spawn()?;
    Ok(path)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
