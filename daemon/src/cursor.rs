//! Make the cursor bigger while the phone is moving it, so it's easy to spot from across
//! the room. It stays big until the phone has been still for a moment, so a run of
//! swipes doesn't make it shrink and grow between each one.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{info, warn};

use crate::config::PointerConfig;

pub struct CursorBoost {
    moved: Arc<Notify>,
}

impl CursorBoost {
    /// None when `cursor_boost_size` is 0.
    pub fn spawn(cfg: &PointerConfig) -> Option<Self> {
        if cfg.cursor_boost_size == 0 {
            return None;
        }
        let theme = if cfg.cursor_theme.is_empty() { detect_theme() } else { cfg.cursor_theme.clone() };
        let (normal, big) = (cfg.cursor_size, cfg.cursor_boost_size);
        let linger = Duration::from_millis(cfg.cursor_boost_ms);
        info!(%theme, normal, big, "cursor boost enabled");
        let moved = Arc::new(Notify::new());
        let rx = moved.clone();
        tokio::spawn(async move {
            // In case a previous run stopped while the cursor was big.
            set_cursor(&theme, normal).await;
            loop {
                rx.notified().await;
                set_cursor(&theme, big).await;
                // Every move restarts the linger; shrink once the phone has been still.
                while tokio::time::timeout(linger, rx.notified()).await.is_ok() {}
                set_cursor(&theme, normal).await;
            }
        });
        Some(Self { moved })
    }

    /// Called for every pointer move from the phone. Cheap: just wakes the task.
    pub fn poke(&self) {
        self.moved.notify_one();
    }
}

async fn set_cursor(theme: &str, size: u32) {
    let out = tokio::process::Command::new("hyprctl")
        .args(["setcursor", theme, &size.to_string()])
        .output()
        .await;
    match out {
        Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "ok" => {}
        Ok(o) => warn!("hyprctl setcursor: {}", String::from_utf8_lossy(&o.stdout).trim()),
        Err(e) => warn!("hyprctl setcursor: {e}"),
    }
}

/// The session's cursor theme: Hyprland's env, then GNOME settings, then "default".
fn detect_theme() -> String {
    for var in ["HYPRCURSOR_THEME", "XCURSOR_THEME"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return v;
        }
    }
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "cursor-theme"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into())
}
