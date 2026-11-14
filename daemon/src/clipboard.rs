//! PC clipboard via wl-clipboard: watch for changes, read, and write text or images.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use crate::protocol::{ClipKind, ServerMsg};
use crate::server::AppState;

/// Larger clipboard images are skipped (they'd be slow to push over Wi-Fi on every copy).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    Text(String),
    Image { mime: String, data: Vec<u8> },
}

impl Clip {
    /// Identifies content so neither side re-sends what it just received.
    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        match self {
            Clip::Text(t) => {
                h.update(b"text\0");
                h.update(t.as_bytes());
            }
            Clip::Image { data, .. } => {
                h.update(b"image\0");
                h.update(data);
            }
        }
        hex::encode(&h.finalize()[..16])
    }

    pub fn to_msg(&self) -> ServerMsg {
        let hash = self.hash();
        match self {
            Clip::Text(t) => ServerMsg::Clip { kind: ClipKind::Text, text: Some(t.clone()), mime: None, data: None, hash },
            Clip::Image { mime, data } => ServerMsg::Clip {
                kind: ClipKind::Image,
                text: None,
                mime: Some(mime.clone()),
                data: Some(B64.encode(data)),
                hash,
            },
        }
    }

    /// Build from a phone message.
    pub fn from_parts(kind: ClipKind, text: Option<String>, mime: Option<String>, data: Option<String>) -> Result<Self> {
        match kind {
            ClipKind::Text => {
                let t = text.context("clipboard text missing")?;
                if t.len() > MAX_TEXT_BYTES {
                    bail!("clipboard text is too large");
                }
                Ok(Clip::Text(t))
            }
            ClipKind::Image => {
                let mime = mime.unwrap_or_else(|| "image/png".into());
                if !mime.starts_with("image/") || mime.contains(char::is_whitespace) {
                    bail!("unsupported clipboard type {mime:?}");
                }
                let data = B64.decode(data.context("clipboard image missing")?).context("bad image data")?;
                if data.len() > MAX_IMAGE_BYTES {
                    bail!("clipboard image is larger than 20 MB");
                }
                Ok(Clip::Image { mime, data })
            }
        }
    }
}

async fn output(args: &[&str]) -> Result<Vec<u8>> {
    let out = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new("wl-paste").args(args).stdin(Stdio::null()).kill_on_drop(true).output(),
    )
    .await
    .context("wl-paste timed out")??;
    if !out.status.success() {
        bail!("wl-paste failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// Current PC clipboard, preferring an image if one is offered. `None` when empty.
pub async fn read() -> Result<Option<Clip>> {
    let Ok(types) = output(&["--list-types"]).await else { return Ok(None) };
    let types = String::from_utf8_lossy(&types);
    let types: Vec<&str> = types.lines().map(str::trim).collect();
    let image = types
        .iter()
        .find(|t| **t == "image/png")
        .or_else(|| types.iter().find(|t| t.starts_with("image/")));
    if let Some(mime) = image {
        let data = output(&["--type", mime]).await?;
        if data.len() > MAX_IMAGE_BYTES {
            debug!(bytes = data.len(), "clipboard image too large to sync");
            return Ok(None);
        }
        return Ok(Some(Clip::Image { mime: mime.to_string(), data }));
    }
    if types.iter().any(|t| t.starts_with("text/") || *t == "UTF8_STRING" || *t == "STRING") {
        let data = output(&["--no-newline", "--type", "text"]).await?;
        if data.is_empty() || data.len() > MAX_TEXT_BYTES {
            return Ok(None);
        }
        return Ok(Some(Clip::Text(String::from_utf8_lossy(&data).into_owned())));
    }
    Ok(None)
}

/// Replace the PC clipboard.
pub async fn write(clip: &Clip) -> Result<()> {
    let (mime, bytes): (&str, &[u8]) = match clip {
        Clip::Text(t) => ("text/plain;charset=utf-8", t.as_bytes()),
        Clip::Image { mime, data } => (mime, data),
    };
    // wl-copy forks to keep serving the clipboard; the parent exits once it has the data.
    let mut child = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("couldn't run wl-copy")?;
    let mut stdin = child.stdin.take().context("wl-copy stdin")?;
    stdin.write_all(bytes).await?;
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await.context("wl-copy timed out")??;
    if !status.success() {
        bail!("wl-copy exited with {status}");
    }
    Ok(())
}

/// Watch the PC clipboard and broadcast new content, skipping what the phone just set.
pub fn spawn_watcher(st: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = watch_once(&st).await {
                warn!("clipboard watcher stopped: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}

async fn watch_once(st: &AppState) -> Result<()> {
    // `wl-paste --watch` runs the command on every clipboard change (and once at start).
    let mut child = Command::new("wl-paste")
        .args(["--watch", "echo", "changed"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("couldn't run wl-paste --watch")?;
    let mut lines = BufReader::new(child.stdout.take().context("wl-paste stdout")?).lines();
    while lines.next_line().await?.is_some() {
        let Ok(Some(clip)) = read().await else { continue };
        let hash = clip.hash();
        let is_new = {
            let mut mem = st.clip_memory.lock().unwrap();
            let seen = mem.last_pc.as_deref() == Some(hash.as_str());
            mem.last_pc = Some(hash.clone());
            // Skip repeats, and our own write of the phone's clipboard so it isn't echoed back.
            !seen && mem.from_phone.as_deref() != Some(hash.as_str())
        };
        if is_new {
            let _ = st.clip_tx.send(Arc::new(clip));
        }
    }
    bail!("wl-paste --watch exited")
}

#[derive(Debug, Default)]
pub struct ClipMemory {
    /// Hash of the last content the phone put on the PC clipboard.
    pub from_phone: Option<String>,
    /// Hash of the PC clipboard as last seen by the watcher.
    pub last_pc: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_hash() {
        let img = Clip::Image { mime: "image/png".into(), data: vec![1, 2, 3] };
        let ServerMsg::Clip { kind, text, mime, data, hash } = img.to_msg() else { panic!() };
        assert_eq!(hash, img.hash());
        assert_eq!(Clip::from_parts(kind, text, mime, data).unwrap(), img);

        let t = Clip::Text("hello".into());
        assert_ne!(t.hash(), Clip::Image { mime: "image/png".into(), data: b"hello".to_vec() }.hash());
        assert!(Clip::from_parts(ClipKind::Image, None, Some("text/html".into()), Some(B64.encode([1]))).is_err());
        assert!(Clip::from_parts(ClipKind::Text, None, None, None).is_err());
    }
}
