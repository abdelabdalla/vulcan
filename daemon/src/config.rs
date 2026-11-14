use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Locations of everything vulcand stores on disk (~/.config/vulcan).
#[derive(Debug, Clone)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let base = directories::BaseDirs::new().context("cannot determine home directory")?;
        let dir = base.config_dir().join("vulcan");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join("config.toml")
    }
    pub fn devices(&self) -> PathBuf {
        self.dir.join("devices.toml")
    }
    pub fn cert(&self) -> PathBuf {
        self.dir.join("cert.pem")
    }
    pub fn key(&self) -> PathBuf {
        self.dir.join("key.pem")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    pub bind: String,
    pub keyboard: KeyboardConfig,
    pub pointer: PointerConfig,
    pub power: PowerConfig,
    pub commands: Vec<CommandConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    /// Allow lock / suspend / reboot / power off from the phone.
    pub enabled: bool,
    /// Shell command that locks the screen.
    pub lock: String,
}

impl Default for PowerConfig {
    fn default() -> Self {
        // Launched through Hyprland so the lock screen isn't tied to vulcand's lifetime.
        Self { enabled: true, lock: "hyprctl dispatch exec hyprlock".into() }
    }
}

/// A button on the phone's Controls tab that runs a shell command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandConfig {
    pub name: String,
    pub run: String,
    /// SF Symbol name shown on the phone.
    #[serde(default = "default_icon")]
    pub icon: String,
}

fn default_icon() -> String {
    "terminal".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// XKB layout used to turn typed characters into key presses. "auto" asks Hyprland.
    pub layout: String,
    pub variant: String,
    pub options: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PointerConfig {
    /// Base multiplier from phone points to pixels.
    pub sensitivity: f64,
    /// Extra gain for fast swipes (0 = linear).
    pub acceleration: f64,
    /// Hi-res wheel units (120 = one notch) per phone point.
    pub scroll_speed: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 47800,
            bind: "0.0.0.0".into(),
            keyboard: KeyboardConfig::default(),
            pointer: PointerConfig::default(),
            power: PowerConfig::default(),
            commands: Vec::new(),
        }
    }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self { layout: "auto".into(), variant: String::new(), options: String::new() }
    }
}

impl Default for PointerConfig {
    fn default() -> Self {
        Self { sensitivity: 1.5, acceleration: 0.4, scroll_speed: 4.0 }
    }
}

const TEMPLATE: &str = r#"# vulcand configuration. Restart the service after editing:
#   systemctl --user restart vulcand

port = 47800
bind = "0.0.0.0"

[keyboard]
# XKB layout used to map typed characters to keys. "auto" reads it from Hyprland.
layout = "auto"
variant = ""
options = ""

[pointer]
sensitivity = 1.5    # phone points -> pixels
acceleration = 0.4   # extra gain on fast swipes, 0 = linear
scroll_speed = 4.0   # wheel units per point (120 = one notch)

[power]
enabled = true                           # lock / suspend / reboot / power off from the phone
lock = "hyprctl dispatch exec hyprlock"

# Buttons on the phone's Controls tab. `icon` is an SF Symbol name.
# Launch apps through `hyprctl dispatch exec ...` so they don't belong to vulcand.
#
# [[commands]]
# name = "Workspace 1"
# icon = "1.square"
# run = "hyprctl dispatch workspace 1"
#
# [[commands]]
# name = "Browser"
# icon = "globe"
# run = "hyprctl dispatch exec zen-browser"
"#;

impl Config {
    /// Load the config, writing a commented default file on first run.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.config();
        if !path.exists() {
            std::fs::write(&path, TEMPLATE)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

/// Resolve layout/variant/options, asking Hyprland when layout is "auto".
pub fn resolve_keyboard(cfg: &KeyboardConfig) -> (String, String, String) {
    if cfg.layout != "auto" {
        return (cfg.layout.clone(), cfg.variant.clone(), cfg.options.clone());
    }
    let get = |opt: &str| -> Option<String> {
        let out = std::process::Command::new("hyprctl")
            .args(["getoption", opt, "-j"])
            .output()
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        v.get("str").and_then(|s| s.as_str()).map(|s| s.trim().to_string())
    };
    let layout = get("input:kb_layout").filter(|s| !s.is_empty()).unwrap_or_else(|| "us".into());
    // Hyprland allows "gb,us"; the virtual keyboard starts on the first layout.
    let first = |s: String| s.split(',').next().unwrap_or("").trim().to_string();
    let variant = get("input:kb_variant").map(first).unwrap_or_default();
    let options = get("input:kb_options").unwrap_or_default();
    (first(layout), variant, options)
}
