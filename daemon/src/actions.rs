//! Non-input controls: media, volume, power and custom commands.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::config::{CommandConfig, PowerConfig};
use std::collections::HashMap;

use crate::protocol::{DisplayAction, MediaAction, MonitorInfo, PowerAction, ServerMsg, VolumeAction};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// Run a helper and capture stdout, failing on non-zero exit or after 3 s.
async fn run(prog: &str, args: &[&str]) -> Result<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new(prog).args(args).stdin(Stdio::null()).kill_on_drop(true).output(),
    )
    .await
    .with_context(|| format!("{prog} timed out"))?
    .with_context(|| format!("couldn't run {prog}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{prog} failed: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a user command via `sh -c` without waiting for it to finish (it may launch an app).
/// Only a quick failure (non-zero exit within half a second) is reported.
async fn shell(cmd: &str) -> Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        // Output goes to the journal: journalctl --user -u vulcand
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("couldn't start sh")?;
    match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
        Ok(Ok(status)) if !status.success() => {
            bail!("command exited with {status} (see journalctl --user -u vulcand)")
        }
        Ok(Err(e)) => Err(e.into()),
        _ => Ok(()),
    }
}

pub async fn media(a: MediaAction) -> Result<()> {
    let arg = match a {
        MediaAction::PlayPause => "play-pause",
        MediaAction::Next => "next",
        MediaAction::Previous => "previous",
    };
    run("playerctl", &[arg]).await.map(drop)
}

pub async fn now_playing() -> ServerMsg {
    let fmt = "{{status}}\t{{artist}}\t{{title}}\t{{playerName}}";
    match run("playerctl", &["metadata", "--format", fmt]).await {
        Ok(out) => parse_media(&out),
        Err(_) => ServerMsg::Media {
            status: "none".into(),
            title: String::new(),
            artist: String::new(),
            player: String::new(),
        },
    }
}

fn parse_media(out: &str) -> ServerMsg {
    let mut parts = out.trim_end_matches('\n').splitn(4, '\t');
    let mut next = || parts.next().unwrap_or("").to_string();
    let (status, artist, title, player) = (next(), next(), next(), next());
    ServerMsg::Media { status, title, artist, player }
}

pub async fn volume(a: VolumeAction, value: Option<f64>) -> Result<()> {
    match a {
        VolumeAction::Up => run("wpctl", &["set-volume", "-l", "1.0", SINK, "5%+"]).await,
        VolumeAction::Down => run("wpctl", &["set-volume", SINK, "5%-"]).await,
        VolumeAction::Mute => run("wpctl", &["set-mute", SINK, "toggle"]).await,
        VolumeAction::Set => {
            let v = value.context("volume set needs a value")?.clamp(0.0, 1.0);
            run("wpctl", &["set-volume", SINK, &format!("{v:.3}")]).await
        }
    }
    .map(drop)
}

pub async fn get_volume() -> Option<ServerMsg> {
    parse_volume(&run("wpctl", &["get-volume", SINK]).await.ok()?)
}

/// Parses `Volume: 0.45` or `Volume: 0.45 [MUTED]`.
fn parse_volume(out: &str) -> Option<ServerMsg> {
    let rest = out.trim().strip_prefix("Volume:")?.trim();
    let level = rest.split_whitespace().next()?.parse().ok()?;
    Some(ServerMsg::Volume { level, muted: rest.contains("[MUTED]") })
}

pub async fn power(a: PowerAction, cfg: &PowerConfig) -> Result<String> {
    if !cfg.enabled {
        bail!("power controls are disabled in the PC's config");
    }
    match a {
        PowerAction::Lock => shell(&cfg.lock).await.map(|_| "Locking the PC".into()),
        PowerAction::Suspend => run("systemctl", &["suspend"]).await.map(|_| "Suspending the PC".into()),
        PowerAction::Reboot => run("systemctl", &["reboot"]).await.map(|_| "Rebooting the PC".into()),
        PowerAction::Poweroff => run("systemctl", &["poweroff"]).await.map(|_| "Shutting down the PC".into()),
    }
}

/// hyprctl reports most failures on stdout with exit status 0, so require "ok".
async fn hypr(args: &[&str]) -> Result<()> {
    let out = run("hyprctl", args).await?;
    if out.trim() != "ok" {
        bail!("hyprctl {}: {}", args.join(" "), out.trim());
    }
    Ok(())
}

/// Set a Hyprland option for the running session (Lua config first, then the legacy keyword).
async fn hypr_option(section: &str, key: &str, value: &str) -> Result<()> {
    let lua = format!("hl.config({{ {section} = {{ {key} = {value} }} }})");
    if hypr(&["eval", &lua]).await.is_ok() {
        return Ok(());
    }
    hypr(&["keyword", &format!("{section}:{key}"), value]).await
}

async fn dpms(on: bool, monitor: Option<&str>) -> Result<()> {
    let state = if on { "on" } else { "off" };
    // Lua config takes a table; a bare string argument is ignored and toggles instead.
    let lua = match monitor {
        Some(m) => format!("hl.dsp.dpms({{ action = \"{state}\", monitor = \"{m}\" }})"),
        None => format!("hl.dsp.dpms({{ action = \"{state}\" }})"),
    };
    if hypr(&["dispatch", &lua]).await.is_ok() {
        return Ok(());
    }
    let mut legacy = vec!["dispatch", "dpms", state];
    legacy.extend(monitor);
    hypr(&legacy).await
}

/// Current monitors with their layout and DPMS state.
pub async fn monitors(aliases: &HashMap<String, String>) -> Result<Vec<MonitorInfo>> {
    parse_monitors(&run("hyprctl", &["monitors", "-j"]).await?, aliases)
}

fn parse_monitors(json: &str, aliases: &HashMap<String, String>) -> Result<Vec<MonitorInfo>> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).context("parsing hyprctl monitors")?;
    let mut out: Vec<MonitorInfo> = list
        .iter()
        .filter(|m| !m["disabled"].as_bool().unwrap_or(false))
        .map(|m| {
            let name = m["name"].as_str().unwrap_or("?").to_string();
            let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
            let (mut w, mut h) = (m["width"].as_f64().unwrap_or(0.0) / scale, m["height"].as_f64().unwrap_or(0.0) / scale);
            // Rotated 90° or 270° (transform 1, 3, 5, 7) swaps the logical size.
            if m["transform"].as_i64().unwrap_or(0) % 2 == 1 {
                std::mem::swap(&mut w, &mut h);
            }
            let label = aliases
                .get(&name)
                .cloned()
                .or_else(|| m["model"].as_str().filter(|s| !s.is_empty()).map(str::to_string))
                .unwrap_or_else(|| name.clone());
            MonitorInfo {
                label,
                x: m["x"].as_f64().unwrap_or(0.0),
                y: m["y"].as_f64().unwrap_or(0.0),
                w,
                h,
                on: m["dpmsStatus"].as_bool().unwrap_or(true),
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    Ok(out)
}

/// Switch one monitor (by connector) or all of them off/on via Hyprland DPMS, without suspending.
/// Returns a message for the phone and the monitors' new state.
pub async fn display(
    a: DisplayAction,
    monitor: Option<&str>,
    aliases: &HashMap<String, String>,
) -> Result<(String, Vec<MonitorInfo>)> {
    let before = monitors(aliases).await?;
    let targeted = |m: &MonitorInfo| monitor.is_none_or(|n| n == m.name);
    let targets: Vec<&MonitorInfo> = before.iter().filter(|m| targeted(m)).collect();
    if targets.is_empty() {
        bail!("no monitor named {:?}", monitor.unwrap_or(""));
    }
    let turn_on = match a {
        DisplayAction::On => true,
        DisplayAction::Off => false,
        DisplayAction::Toggle => !targets.iter().any(|m| m.on),
    };

    // Waking on input turns every monitor back on, which would undo switching off just one.
    // So only allow it when all of them end up off.
    let all_off = before.iter().all(|m| if targeted(m) { !turn_on } else { !m.on });
    let wake = if all_off { "true" } else { "false" };
    hypr_option("misc", "key_press_enables_dpms", wake).await?;
    hypr_option("misc", "mouse_move_enables_dpms", wake).await?;

    dpms(turn_on, monitor).await?;
    // hyprctl can report "ok" for a dispatcher that did nothing; check the result.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after = monitors(aliases).await?;
    if let Some(m) = after.iter().find(|m| targeted(m) && m.on != turn_on) {
        bail!("{} didn't switch {}", m.label, if turn_on { "on" } else { "off" });
    }

    let what = match monitor.and_then(|n| after.iter().find(|m| m.name == n)) {
        Some(m) => format!("{} ({})", m.label, m.name),
        None => "Screens".into(),
    };
    let msg = match (turn_on, all_off) {
        (true, _) => format!("{what} on"),
        (false, true) => "All screens off. Move the mouse or press a key to wake them".into(),
        (false, false) => format!("{what} off"),
    };
    Ok((msg, after))
}

/// Only commands defined in the config can run; the phone just names one.
pub async fn custom(id: &str, commands: &[CommandConfig]) -> Result<String> {
    let cmd = commands.iter().find(|c| c.name == id).with_context(|| format!("no command named {id:?}"))?;
    shell(&cmd.run).await?;
    Ok(format!("Ran {}", cmd.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_output() {
        assert_eq!(parse_volume("Volume: 0.50\n"), Some(ServerMsg::Volume { level: 0.5, muted: false }));
        assert_eq!(parse_volume("Volume: 0.25 [MUTED]\n"), Some(ServerMsg::Volume { level: 0.25, muted: true }));
        assert_eq!(parse_volume("garbage"), None);
    }

    #[test]
    fn media_output() {
        let m = parse_media("Playing\tSome Artist\tA Song\tspotify\n");
        assert_eq!(
            m,
            ServerMsg::Media {
                status: "Playing".into(),
                artist: "Some Artist".into(),
                title: "A Song".into(),
                player: "spotify".into()
            }
        );
    }

    #[test]
    fn monitor_layout() {
        let json = r#"[
            {"name":"DP-1","model":"LG ULTRAFINE","x":3840,"y":0,"width":3840,"height":2160,"scale":2,"transform":0,"dpmsStatus":true,"disabled":false},
            {"name":"DP-2","model":"LG ULTRAFINE","x":0,"y":0,"width":3840,"height":2160,"scale":1,"transform":1,"dpmsStatus":false,"disabled":false},
            {"name":"HDMI-A-1","model":"","x":0,"y":0,"width":1920,"height":1080,"scale":1,"transform":0,"dpmsStatus":true,"disabled":true}
        ]"#;
        let aliases = HashMap::from([("DP-2".to_string(), "Left".to_string())]);
        let m = parse_monitors(json, &aliases).unwrap();
        assert_eq!(m.len(), 2, "disabled monitors are skipped");
        assert_eq!((m[0].name.as_str(), m[0].label.as_str(), m[0].on), ("DP-2", "Left", false));
        assert_eq!((m[0].w, m[0].h), (2160.0, 3840.0), "rotated monitor swaps size");
        assert_eq!((m[1].label.as_str(), m[1].w, m[1].h), ("LG ULTRAFINE", 1920.0, 1080.0), "scaled");
    }

    #[tokio::test]
    async fn custom_commands_are_allowlisted() {
        let cmds = vec![CommandConfig { name: "ok".into(), run: "true".into(), icon: "x".into() }];
        assert_eq!(custom("ok", &cmds).await.unwrap(), "Ran ok");
        assert!(custom("rm -rf /", &cmds).await.is_err());
        let bad = vec![CommandConfig { name: "bad".into(), run: "exit 3".into(), icon: "x".into() }];
        assert!(custom("bad", &bad).await.is_err());
    }
}
