//! Non-input controls: media, volume, power and custom commands.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::config::{CommandConfig, PowerConfig};
use crate::protocol::{DisplayAction, MediaAction, PowerAction, ServerMsg, VolumeAction};

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

async fn dpms(state: &str) -> Result<()> {
    if hypr(&["dispatch", &format!("hl.dsp.dpms(\"{state}\")")]).await.is_ok() {
        return Ok(());
    }
    hypr(&["dispatch", "dpms", state]).await
}

/// Monitors off/on via Hyprland DPMS, without suspending.
pub async fn display(a: DisplayAction) -> Result<String> {
    match a {
        DisplayAction::Off => {
            // Hyprland's defaults don't wake on input; let any key press or mouse movement
            // (physical or from the phone) bring the screens back.
            hypr_option("misc", "key_press_enables_dpms", "true").await?;
            hypr_option("misc", "mouse_move_enables_dpms", "true").await?;
            dpms("off").await?;
            Ok("Screens off. Move the mouse or press a key to wake them".into())
        }
        DisplayAction::On => dpms("on").await.map(|_| "Screens on".into()),
    }
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

    #[tokio::test]
    async fn custom_commands_are_allowlisted() {
        let cmds = vec![CommandConfig { name: "ok".into(), run: "true".into(), icon: "x".into() }];
        assert_eq!(custom("ok", &cmds).await.unwrap(), "Ran ok");
        assert!(custom("rm -rf /", &cmds).await.is_err());
        let bad = vec![CommandConfig { name: "bad".into(), run: "exit 3".into(), icon: "x".into() }];
        assert!(custom("bad", &bad).await.is_err());
    }
}
