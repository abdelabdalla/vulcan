//! Wallpaper gallery: every picture in one folder, set through the user's own script (awww +
//! pywal), optionally rotating in order or shuffled on a timer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, watch};
use tracing::{info, warn};

use crate::config::{Paths, WallpaperConfig, expand_home};
use crate::files::{self, ApiError};
use crate::protocol::{Rotation, ServerMsg, WallpaperInfo};
use crate::server::AppState;

/// Formats awww shows as they are.
const NATIVE: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tga", "tif", "tiff", "pnm", "ff", "farbfeld"];
/// Pictures awww can't read; converted to JPEG on the way into the gallery.
const CONVERT: &[&str] = &["heic", "heif", "avif", "jxl"];

const MIN_INTERVAL: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    rotation: Rotation,
    /// Seconds between changes.
    interval: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { rotation: Rotation::Off, interval: 30 * 60 }
    }
}

pub struct Wallpapers {
    pub dir: PathBuf,
    script: PathBuf,
    state_path: PathBuf,
    settings: Mutex<Settings>,
    /// Shuffle order still to show.
    bag: Mutex<Vec<String>>,
    /// Bumped whenever the gallery, the rotation or the wallpaper changes.
    changed: watch::Sender<u64>,
    /// Restarts the rotation timer.
    reset: Notify,
    /// One wallpaper change at a time (pywal takes a moment).
    applying: tokio::sync::Mutex<()>,
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_picture(path: &Path) -> bool {
    let ext = ext_of(path);
    NATIVE.contains(&ext.as_str()) || CONVERT.contains(&ext.as_str())
}

impl Wallpapers {
    pub fn new(cfg: &WallpaperConfig, paths: &Paths) -> Arc<Self> {
        let state_path = paths.wallpaper_state();
        let settings: Settings =
            std::fs::read_to_string(&state_path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        let dir = expand_home(&cfg.dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("creating wallpaper folder {}: {e}", dir.display());
        }
        Arc::new(Self {
            dir,
            script: expand_home(&cfg.script),
            state_path,
            settings: Mutex::new(settings),
            bag: Mutex::default(),
            changed: watch::Sender::new(0),
            reset: Notify::new(),
            applying: tokio::sync::Mutex::new(()),
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn bump(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// Gallery pictures in name order.
    fn list(&self) -> Vec<WallpaperInfo> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut items: Vec<WallpaperInfo> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !NATIVE.contains(&ext_of(Path::new(&name)).as_str()) {
                    return None;
                }
                let modified = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                Some(WallpaperInfo { name, modified })
            })
            .collect();
        items.sort_by_key(|i| i.name.to_lowercase());
        items
    }

    fn names(&self) -> Vec<String> {
        self.list().into_iter().map(|i| i.name).collect()
    }

    /// The gallery picture on screen, if the wallpaper is one (asks awww).
    async fn current(&self) -> Option<String> {
        let out = tokio::process::Command::new("awww").arg("query").output().await.ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let path = text.lines().find_map(|l| l.split_once("image: ").map(|(_, p)| PathBuf::from(p.trim())))?;
        let dir = self.dir.canonicalize().ok()?;
        // Removed pictures stay on screen until the next change; they aren't in the gallery any more.
        (path.parent()? == dir && path.is_file()).then(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))?
    }

    pub async fn gallery(&self) -> ServerMsg {
        let s = *self.settings.lock().unwrap();
        let home = files::home().ok();
        let dir = home
            .and_then(|h| self.dir.strip_prefix(h).ok().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_default();
        ServerMsg::Wallpapers {
            dir,
            items: self.list(),
            current: self.current().await,
            rotation: s.rotation,
            interval: s.interval,
        }
    }

    /// Show a gallery picture and restart the rotation timer.
    pub async fn set(&self, name: &str) -> Result<String> {
        if files::sanitize_name(name) != name || !self.names().iter().any(|n| n == name) {
            bail!("{name} isn't in the wallpaper gallery");
        }
        self.apply(name).await?;
        self.reset.notify_one();
        Ok(format!("Wallpaper set to {name}"))
    }

    /// The next picture in the rotation (in order unless shuffling).
    pub async fn next(&self) -> Result<String> {
        let names = self.names();
        if names.is_empty() {
            bail!("The wallpaper gallery is empty");
        }
        let current = self.current().await;
        let rotation = self.settings.lock().unwrap().rotation;
        let pick = if rotation == Rotation::Shuffle {
            let mut bag = self.bag.lock().unwrap();
            bag.retain(|n| names.contains(n));
            if bag.is_empty() {
                let mut fresh: Vec<String> = names.iter().filter(|n| Some(*n) != current.as_ref()).cloned().collect();
                if fresh.is_empty() {
                    fresh = names.clone();
                }
                fresh.shuffle(&mut rand::rng());
                *bag = fresh;
            }
            bag.pop().expect("bag refilled above")
        } else {
            let at = current.and_then(|c| names.iter().position(|n| *n == c));
            names[at.map_or(0, |i| (i + 1) % names.len())].clone()
        };
        self.apply(&pick).await?;
        self.reset.notify_one();
        Ok(format!("Wallpaper set to {pick}"))
    }

    pub fn set_rotation(&self, rotation: Rotation, interval: Option<u64>) -> String {
        let s = {
            let mut s = self.settings.lock().unwrap();
            s.rotation = rotation;
            if let Some(i) = interval {
                s.interval = i.max(MIN_INTERVAL);
            }
            *s
        };
        if let Err(e) = std::fs::write(&self.state_path, toml::to_string(&s).unwrap_or_default()) {
            warn!("saving wallpaper settings: {e}");
        }
        self.bag.lock().unwrap().clear();
        self.reset.notify_one();
        self.bump();
        info!(rotation = ?s.rotation, interval = s.interval, "wallpaper rotation");
        match s.rotation {
            Rotation::Off => "Wallpaper rotation off".into(),
            Rotation::Order => format!("Changing wallpaper every {}", describe(s.interval)),
            Rotation::Shuffle => format!("Shuffling wallpaper every {}", describe(s.interval)),
        }
    }

    async fn apply(&self, name: &str) -> Result<()> {
        let _one = self.applying.lock().await;
        let path = self.dir.join(name);
        let run = tokio::process::Command::new("bash").arg(&self.script).arg(&path).output();
        let out = tokio::time::timeout(Duration::from_secs(60), run)
            .await
            .context("the wallpaper script took too long")?
            .with_context(|| format!("running {}", self.script.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("wallpaper script failed: {}", err.lines().last().unwrap_or("no output"));
        }
        info!(wallpaper = %name, "wallpaper set");
        self.bump();
        Ok(())
    }

    /// Copy pictures from the home folder into the gallery. A folder adds the pictures directly in it.
    pub async fn add(&self, rels: &[String]) -> Result<String> {
        let home = files::home().map_err(|e| anyhow::anyhow!(e.1))?;
        let mut sources = Vec::new();
        for rel in rels {
            let (path, _) = files::resolve(&home, rel, true).map_err(|e| anyhow::anyhow!(e.1))?;
            if path.is_dir() {
                let mut found: Vec<PathBuf> = std::fs::read_dir(&path)?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && is_picture(p))
                    .collect();
                found.sort();
                sources.extend(found);
            } else if is_picture(&path) {
                sources.push(path);
            }
        }
        if sources.is_empty() {
            bail!("No pictures there to add");
        }
        let (mut added, mut failed) = (0, 0);
        for src in &sources {
            match self.import(src, true).await {
                Ok(true) => added += 1,
                Ok(false) => {}
                Err(e) => {
                    failed += 1;
                    warn!("adding {} to the wallpapers: {e:#}", src.display());
                }
            }
        }
        self.bump();
        info!(added, failed, "pictures added to the wallpaper gallery");
        let mut msg = match added {
            0 if failed == 0 => "Those are already in the gallery".to_string(),
            0 => String::new(),
            1 => "Added 1 wallpaper".into(),
            n => format!("Added {n} wallpapers"),
        };
        if failed > 0 {
            let what = if failed == 1 { "1 picture".to_string() } else { format!("{failed} pictures") };
            let note = format!("{what} couldn't be converted (HEIC/AVIF need libheif on the PC)");
            if msg.is_empty() {
                bail!(note);
            }
            msg = format!("{msg}; {note}");
        }
        Ok(msg)
    }

    /// Bring one picture into the gallery, converting it if awww can't show it. With `copy`, the
    /// source stays put; otherwise it's a file we just received, and is replaced. Returns false
    /// when an identical file is already there.
    async fn import(&self, src: &Path, copy: bool) -> Result<bool> {
        let name = files::sanitize_name(&src.file_name().unwrap_or_default().to_string_lossy());
        if copy {
            if src.parent().and_then(|p| p.canonicalize().ok()) == self.dir.canonicalize().ok() {
                return Ok(false);
            }
            let existing = self.dir.join(&name);
            if let (Ok(a), Ok(b)) = (std::fs::metadata(src), std::fs::metadata(&existing))
                && a.len() == b.len()
            {
                return Ok(false);
            }
        }
        if CONVERT.contains(&ext_of(src).as_str()) {
            let stem = Path::new(&name).file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let dest = files::unique_path(&self.dir, &format!("{stem}.jpg"));
            let mut input = src.as_os_str().to_owned();
            input.push("[0]");
            let out = tokio::process::Command::new("magick")
                .arg(input)
                .args(["-auto-orient", "-quality", "92"])
                .arg(&dest)
                .output()
                .await
                .context("running magick")?;
            if !copy {
                let _ = tokio::fs::remove_file(src).await;
            }
            if !out.status.success() {
                bail!("couldn't convert: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
        } else if copy {
            tokio::fs::copy(src, files::unique_path(&self.dir, &name)).await?;
        }
        Ok(true)
    }

    /// Move pictures out of the gallery into the trash.
    pub async fn remove(&self, names: &[String]) -> Result<String> {
        let mut removed = 0;
        for name in names {
            if files::sanitize_name(name) != *name {
                continue;
            }
            let path = self.dir.join(name);
            if !path.is_file() {
                continue;
            }
            let trashed = tokio::process::Command::new("gio")
                .arg("trash")
                .arg(&path)
                .status()
                .await
                .is_ok_and(|s| s.success());
            if trashed || tokio::fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
        }
        self.bump();
        Ok(match removed {
            1 => "Removed 1 wallpaper".into(),
            n => format!("Removed {n} wallpapers"),
        })
    }

    pub fn spawn(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let s = *me.settings.lock().unwrap();
                if s.rotation == Rotation::Off {
                    me.reset.notified().await;
                    continue;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(s.interval.max(MIN_INTERVAL))) => {
                        if let Err(e) = me.next().await {
                            warn!("rotating wallpaper: {e:#}");
                        }
                    }
                    _ = me.reset.notified() => {}
                }
            }
        });
    }
}

/// "5 minutes", "1 hour", "90 seconds".
fn describe(secs: u64) -> String {
    let (n, unit) = if secs.is_multiple_of(86400) {
        (secs / 86400, "day")
    } else if secs.is_multiple_of(3600) {
        (secs / 3600, "hour")
    } else if secs.is_multiple_of(60) {
        (secs / 60, "minute")
    } else {
        (secs, "second")
    };
    if n == 1 { unit.to_string() } else { format!("{n} {unit}s") }
}

#[derive(Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// Phone → gallery upload (photos straight from the iPhone).
pub async fn upload(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Result<Json<serde_json::Value>, ApiError> {
    let device = files::authorize(&st, &headers)?;
    if !is_picture(Path::new(&q.name)) {
        return Err(files::err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "only pictures can be wallpapers"));
    }
    let walls = &st.wallpapers;
    let (dest, size) = files::receive(body, &walls.dir, &q.name).await?;
    walls.import(&dest, false).await.map_err(|e| files::err(StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")))?;
    walls.bump();
    info!(%device, file = %q.name, size, "wallpaper from phone");
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_intervals() {
        assert_eq!(describe(60), "minute");
        assert_eq!(describe(300), "5 minutes");
        assert_eq!(describe(3600), "hour");
        assert_eq!(describe(7200), "2 hours");
        assert_eq!(describe(86400), "day");
        assert_eq!(describe(90), "90 seconds");
    }

    #[test]
    fn picture_types() {
        assert!(is_picture(Path::new("a.JPG")));
        assert!(is_picture(Path::new("a.heic")));
        assert!(!is_picture(Path::new("a.mp4")));
        assert!(!is_picture(Path::new("notes")));
    }
}
