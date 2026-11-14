//! File transfer over HTTPS (same TLS server and pairing tokens as the WebSocket):
//! - phone → PC uploads into ~/Downloads/Vulcan
//! - PC → phone "offers" queued by `vulcand send`, the tray or Dolphin, which the phone downloads
//! - read-only browsing and download of the home folder

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

use crate::auth::DeviceStore;
use crate::protocol::OfferInfo;
use crate::server::AppState;

/// Offers the phone hasn't collected within this time are dropped.
const OFFER_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_LISTING: usize = 5000;

// MARK: Errors and auth

#[derive(Debug)]
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        let code = match e.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            std::io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(code, e.to_string())
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, msg.into())
}

/// Same pairing token as the WebSocket, sent as `Authorization: Bearer <token>`.
fn authorize(st: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing token"))?;
    let store = DeviceStore::load(&st.paths.devices()).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let name = store.verify(token).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not paired"))?.to_string();
    if st.paused() {
        return Err(err(StatusCode::LOCKED, "paused from the PC's tray"));
    }
    Ok(name)
}

// MARK: Offers (PC → phone)

struct Offer {
    info: OfferInfo,
    path: PathBuf,
    created: SystemTime,
}

pub struct Offers {
    list: Mutex<Vec<Offer>>,
    pub tx: broadcast::Sender<OfferInfo>,
}

impl Default for Offers {
    fn default() -> Self {
        Self { list: Mutex::new(Vec::new()), tx: broadcast::channel(64).0 }
    }
}

impl Offers {
    /// Queue files for the phone. Directories and unreadable paths are rejected.
    pub fn add(&self, paths: &[PathBuf]) -> Result<Vec<OfferInfo>> {
        let mut added = Vec::new();
        for path in paths {
            let path = path.canonicalize().with_context(|| format!("{}", path.display()))?;
            let meta = std::fs::metadata(&path)?;
            if !meta.is_file() {
                bail!("{} isn't a file (folders aren't supported)", path.display());
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            added.push(Offer {
                info: OfferInfo { id: hex::encode(rand::random::<[u8; 12]>()), name, size: meta.len() },
                path,
                created: SystemTime::now(),
            });
        }
        let infos: Vec<OfferInfo> = added.iter().map(|o| o.info.clone()).collect();
        {
            let mut list = self.list.lock().unwrap();
            list.retain(|o| o.created.elapsed().unwrap_or_default() < OFFER_TTL);
            list.extend(added);
        }
        for info in &infos {
            info!(name = %info.name, size = info.size, "queued file for phone");
            let _ = self.tx.send(info.clone());
        }
        Ok(infos)
    }

    pub fn pending(&self) -> Vec<OfferInfo> {
        let mut list = self.list.lock().unwrap();
        list.retain(|o| o.created.elapsed().unwrap_or_default() < OFFER_TTL);
        list.iter().map(|o| o.info.clone()).collect()
    }

    pub fn done(&self, id: &str) {
        self.list.lock().unwrap().retain(|o| o.info.id != id);
    }

    fn get(&self, id: &str) -> Option<(PathBuf, String)> {
        let list = self.list.lock().unwrap();
        list.iter().find(|o| o.info.id == id).map(|o| (o.path.clone(), o.info.name.clone()))
    }
}

pub async fn download_offer(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Response, ApiError> {
    authorize(&st, &headers)?;
    let (path, name) = st.offers.get(&id).ok_or_else(|| err(StatusCode::NOT_FOUND, "no such file offer"))?;
    file_response(&path, &name).await
}

async fn file_response(path: &Path, name: &str) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    let disposition = format!("attachment; filename*=UTF-8''{}", percent(name));
    let mut resp = Response::new(Body::from_stream(ReaderStream::new(file)));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// MARK: Uploads (phone → PC)

#[derive(Deserialize)]
pub struct UploadQuery {
    name: String,
}

#[derive(Serialize)]
pub struct UploadResult {
    name: String,
    size: u64,
}

pub fn inbox() -> PathBuf {
    let downloads = directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    downloads.join("Vulcan")
}

/// Keep only a plain file name: no directories, no leading dots, nothing empty.
pub fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    let mut out = if cleaned.is_empty() { "file".to_string() } else { cleaned.to_string() };
    while out.len() > 200 {
        out.pop();
    }
    out
}

/// `name`, or `name (1).ext`, `name (2).ext`, ... if taken.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

pub async fn upload(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Result<Json<UploadResult>, ApiError> {
    let device = authorize(&st, &headers)?;
    let dir = inbox();
    tokio::fs::create_dir_all(&dir).await?;
    let name = sanitize_name(&q.name);
    let part = dir.join(format!(".{}.vulcan-part", hex::encode(rand::random::<[u8; 8]>())));

    let mut file = tokio::fs::File::create(&part).await?;
    let mut size = 0u64;
    let mut stream = body.into_data_stream();
    let result: Result<(), ApiError> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| err(StatusCode::BAD_REQUEST, format!("upload interrupted: {e}")))?;
            size += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    let dest = unique_path(&dir, &name);
    tokio::fs::rename(&part, &dest).await?;
    let saved = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
    info!(%device, file = %saved, size, "received file from phone");
    notify_received(dest);
    Ok(Json(UploadResult { name: saved, size }))
}

/// Desktop notification with a button that shows the file in Dolphin.
fn notify_received(path: PathBuf) {
    std::thread::spawn(move || {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let out = std::process::Command::new("notify-send")
            .args(["--app-name=Vulcan", "--icon=document-save", "--action=open=Show in folder", "Received from phone", &name])
            .output();
        if let Ok(out) = out
            && String::from_utf8_lossy(&out.stdout).trim() == "open"
        {
            let _ = std::process::Command::new("dolphin").arg("--select").arg(&path).spawn();
        }
    });
}

// MARK: Browsing the home folder (read-only)

#[derive(Deserialize)]
pub struct FsQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    hidden: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Entry {
    name: String,
    dir: bool,
    size: u64,
    /// Unix seconds.
    modified: u64,
}

#[derive(Serialize)]
pub struct Listing {
    /// Relative to home, "" for home itself.
    path: String,
    entries: Vec<Entry>,
    truncated: bool,
}

fn home() -> Result<PathBuf, ApiError> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "no home directory"))
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Resolve a home-relative path, following symlinks, and refuse anything outside home
/// (or hidden, unless asked for). Returns the real path and its home-relative form.
fn resolve(home: &Path, rel: &str, hidden: bool) -> Result<(PathBuf, String), ApiError> {
    let rel = Path::new(rel.trim_start_matches('/'));
    if rel.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err(err(StatusCode::FORBIDDEN, "path must stay inside the home folder"));
    }
    let home = home.canonicalize()?;
    let real = home.join(rel).canonicalize()?;
    let inside = real.strip_prefix(&home).map_err(|_| err(StatusCode::FORBIDDEN, "path leads outside the home folder"))?;
    if !hidden && inside.components().any(|c| is_hidden(&c.as_os_str().to_string_lossy())) {
        return Err(err(StatusCode::FORBIDDEN, "hidden path (turn on Show hidden)"));
    }
    Ok((real.clone(), inside.to_string_lossy().into_owned()))
}

pub async fn fs_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Result<Json<Listing>, ApiError> {
    authorize(&st, &headers)?;
    let (dir, rel) = resolve(&home()?, &q.path, q.hidden)?;
    let mut rd = tokio::fs::read_dir(&dir).await?;
    let mut entries = Vec::new();
    let mut truncated = false;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        if !q.hidden && is_hidden(&name) {
            continue;
        }
        if entries.len() >= MAX_LISTING {
            truncated = true;
            break;
        }
        // Follow symlinks; skip broken ones.
        let Ok(meta) = tokio::fs::metadata(e.path()).await else { continue };
        let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        entries.push(Entry { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified });
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(Json(Listing { path: rel, entries, truncated }))
}

pub async fn fs_download(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Result<Response, ApiError> {
    let device = authorize(&st, &headers)?;
    let (path, rel) = resolve(&home()?, &q.path, q.hidden)?;
    if !path.is_file() {
        return Err(err(StatusCode::BAD_REQUEST, "not a file"));
    }
    info!(%device, file = %rel, "phone downloading from home folder");
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    file_response(&path, &name).await
}

/// Ask with zenity which files to send, then queue them (tray menu).
pub fn pick_and_send(st: Arc<AppState>) {
    std::thread::spawn(move || {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--multiple", "--separator=\n", "--title=Send to phone"])
            .output();
        let Ok(out) = out else {
            warn!("couldn't run zenity for the file picker");
            return;
        };
        let paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
        if paths.is_empty() {
            return;
        }
        match st.offers.add(&paths) {
            Ok(added) if st.status.borrow().connected.is_empty() => {
                let _ = std::process::Command::new("notify-send")
                    .args(["--app-name=Vulcan", "Queued for your phone", &format!("{} file(s) will download when Vulcan next opens", added.len())])
                    .spawn();
            }
            Ok(_) => {}
            Err(e) => warn!("couldn't queue files: {e:#}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize_name("photo.jpg"), "photo.jpg");
        assert_eq!(sanitize_name("../../.ssh/authorized_keys"), "authorized_keys");
        assert_eq!(sanitize_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_name("a\\b\\c.txt"), "c.txt");
        assert_eq!(sanitize_name("  "), "file");
        assert_eq!(sanitize_name("x\ny.txt"), "xy.txt");
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("vulcan-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("a (1).txt"), "").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (2).txt"));
        assert_eq!(unique_path(&dir, "b.txt"), dir.join("b.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_stays_in_home() {
        let home = std::env::temp_dir().join(format!("vulcan-home-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("vulcan-out-{}", std::process::id()));
        std::fs::create_dir_all(home.join("docs")).unwrap();
        std::fs::create_dir_all(home.join(".secret")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("escape")).unwrap();

        assert_eq!(resolve(&home, "", false).unwrap().1, "");
        assert_eq!(resolve(&home, "docs", false).unwrap().1, "docs");
        assert!(resolve(&home, "../", false).is_err());
        assert!(resolve(&home, "/etc", false).is_err(), "absolute paths are taken as home-relative");
        assert!(resolve(&home, "escape", false).is_err(), "symlink out of home is refused");
        assert!(resolve(&home, ".secret", false).is_err());
        assert!(resolve(&home, ".secret", true).is_ok());

        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }
}
