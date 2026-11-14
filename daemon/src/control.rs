//! Local control socket so `vulcand send` (and the Dolphin menu) can reach the running daemon.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::warn;

use crate::server::AppState;

#[derive(Serialize, Deserialize)]
struct Request {
    send: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    #[serde(default)]
    queued: usize,
    #[serde(default)]
    phone_connected: bool,
    #[serde(default)]
    error: Option<String>,
}

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("vulcan.sock")
}

pub fn spawn_listener(st: Arc<AppState>) -> Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let st = st.clone();
            tokio::spawn(async move {
                if let Err(e) = serve(stream, &st).await {
                    warn!("control socket: {e:#}");
                }
            });
        }
    });
    Ok(())
}

async fn serve(stream: UnixStream, st: &AppState) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let req: Request = serde_json::from_str(&line).context("bad request")?;
    let reply = match st.offers.add(&req.send) {
        Ok(added) => Reply {
            queued: added.len(),
            phone_connected: !st.status.borrow().connected.is_empty(),
            error: None,
        },
        Err(e) => Reply { queued: 0, phone_connected: false, error: Some(format!("{e:#}")) },
    };
    write.write_all(format!("{}\n", serde_json::to_string(&reply)?).as_bytes()).await?;
    Ok(())
}

/// `vulcand send <files>`: hand the files to the running daemon.
pub async fn send(files: &[PathBuf]) -> Result<String> {
    let files: Vec<PathBuf> = files
        .iter()
        .map(|f| f.canonicalize().with_context(|| format!("{}", f.display())))
        .collect::<Result<_>>()?;
    let stream = UnixStream::connect(socket_path()).await.context("vulcand isn't running (systemctl --user start vulcand)")?;
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{}\n", serde_json::to_string(&Request { send: files })?).as_bytes()).await?;
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let reply: Reply = serde_json::from_str(&line).context("bad reply from vulcand")?;
    if let Some(e) = reply.error {
        bail!(e);
    }
    Ok(if reply.phone_connected {
        format!("Sending {} file(s) to your phone.", reply.queued)
    } else {
        format!("Queued {} file(s); they'll download when Vulcan next opens on your phone.", reply.queued)
    })
}

/// Dolphin right-click "Send to iPhone", pointing at this binary.
pub fn install_dolphin_menu() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = directories::BaseDirs::new().context("no home")?.data_dir().join("kio/servicemenus");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("vulcan-send.desktop");
    let exe = exe.display();
    let body = format!(
        "[Desktop Entry]\nType=Service\nMimeType=application/octet-stream;all/allfiles;\nX-KDE-Protocols=file\nActions=sendToPhone\nX-KDE-ServiceTypes=KonqPopupMenu/Plugin\n\n[Desktop Action sendToPhone]\nName=Send to iPhone\nIcon=smartphone\nExec=\"{exe}\" send %F\n"
    );
    std::fs::write(&path, body)?;
    // KDE 6 only loads service menus that are executable.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}
