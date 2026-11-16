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
pub struct ApiError(pub StatusCode, pub String);

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

pub(crate) fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, msg.into())
}

/// Same pairing token as the WebSocket, sent as `Authorization: Bearer <token>`.
pub(crate) fn authorize(st: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
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
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
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
    let (dest, size) = receive(body, &inbox(), &q.name).await?;
    let saved = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
    info!(%device, file = %saved, size, "received file from phone");
    notify_received(dest);
    Ok(Json(UploadResult { name: saved, size }))
}

/// Stream a request body into `dir` under a free version of `name`; returns where it went.
pub(crate) async fn receive(body: Body, dir: &Path, name: &str) -> Result<(PathBuf, u64), ApiError> {
    tokio::fs::create_dir_all(dir).await?;
    let name = sanitize_name(name);
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
    let dest = unique_path(dir, &name);
    tokio::fs::rename(&part, &dest).await?;
    Ok((dest, size))
}

/// Files received recently, and a counter that tells the latest arrival apart.
static RECEIVED: Mutex<(Vec<PathBuf>, u64)> = Mutex::new((Vec::new(), 0));

/// How long to wait for more files before announcing a batch (photos shared together arrive
/// back to back).
const BATCH_QUIET: Duration = Duration::from_secs(3);

/// Desktop notification with a button that shows the files in Dolphin. Files that arrive
/// close together share one notification.
fn notify_received(path: PathBuf) {
    let generation = {
        let mut r = RECEIVED.lock().unwrap();
        r.0.push(path);
        r.1 += 1;
        r.1
    };
    std::thread::spawn(move || {
        std::thread::sleep(BATCH_QUIET);
        let paths = {
            let mut r = RECEIVED.lock().unwrap();
            if r.1 != generation {
                return; // a later file will announce the batch
            }
            std::mem::take(&mut r.0)
        };
        let (title, body) = received_message(&paths);
        let out = std::process::Command::new("notify-send")
            .args(["--app-name=Vulcan", "--icon=document-save", "--action=open=Show in folder", &title, &body])
            .output();
        if let Ok(out) = out
            && String::from_utf8_lossy(&out.stdout).trim() == "open"
        {
            let _ = std::process::Command::new("dolphin").arg("--select").args(&paths).spawn();
        }
    });
}

fn received_message(paths: &[PathBuf]) -> (String, String) {
    let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
    if names.len() == 1 {
        return ("Received from phone".into(), names[0].clone());
    }
    const SHOWN: usize = 3;
    let mut body = names[..names.len().min(SHOWN)].join(", ");
    if names.len() > SHOWN {
        body.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    (format!("Received {} files from phone", names.len()), body)
}

// MARK: Browsing the home folder (read-only)

#[derive(Deserialize)]
pub struct FsQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    hidden: bool,
    /// Thumbnail edge in pixels (default 256).
    #[serde(default)]
    size: Option<u32>,
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

pub(crate) fn home() -> Result<PathBuf, ApiError> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "no home directory"))
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Resolve a home-relative path, following symlinks, and refuse anything outside home
/// (or hidden, unless asked for). Returns the real path and its home-relative form.
pub(crate) fn resolve(home: &Path, rel: &str, hidden: bool) -> Result<(PathBuf, String), ApiError> {
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

// MARK: Whole-folder downloads

const MAX_WALK: usize = 10_000;

#[derive(Serialize)]
pub struct WalkFile {
    /// Home-relative path, for /fs/download.
    path: String,
    /// Where it goes on the phone: the chosen folder's name plus the path inside it.
    rel: String,
    size: u64,
}

#[derive(Serialize)]
pub struct Walk {
    files: Vec<WalkFile>,
    total: u64,
    truncated: bool,
}

/// Every file under a folder, so the phone can download it with its structure.
/// Symlinked folders aren't followed (no cycles); symlinked files must stay inside home.
pub async fn fs_walk(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Result<Json<Walk>, ApiError> {
    authorize(&st, &headers)?;
    let home = home()?.canonicalize()?;
    let (root, _) = resolve(&home, &q.path, q.hidden)?;
    if !root.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "not a folder"));
    }
    let hidden = q.hidden;
    let walk = tokio::task::spawn_blocking(move || walk_dir(&home, &root, hidden))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(walk))
}

fn walk_dir(home: &Path, root: &Path, hidden: bool) -> Walk {
    let root_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Home".into());
    let mut out = Walk { files: Vec::new(), total: 0, truncated: false };
    let mut stack = vec![(root.to_path_buf(), root_name)];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && is_hidden(&name) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let rel = format!("{prefix}/{name}");
            let real = if ft.is_symlink() {
                match e.path().canonicalize() {
                    Ok(p) if p.starts_with(home) && p.is_file() => p,
                    _ => continue,
                }
            } else if ft.is_dir() {
                stack.push((e.path(), rel));
                continue;
            } else if ft.is_file() {
                e.path()
            } else {
                continue;
            };
            if out.files.len() >= MAX_WALK {
                out.truncated = true;
                return out;
            }
            let size = std::fs::metadata(&real).map_or(0, |m| m.len());
            let Ok(home_rel) = real.strip_prefix(home) else { continue };
            out.total += size;
            out.files.push(WalkFile { path: home_rel.to_string_lossy().into_owned(), rel, size });
        }
    }
    out
}

// MARK: Thumbnails

/// At most a few thumbnail generators at once, however fast the phone scrolls.
static THUMB_SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(3));

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "bmp", "tif", "tiff"];
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi"];

/// A ~256px preview of an image or video. Uses the freedesktop thumbnail cache (filled by
/// Dolphin) when it's fresh, otherwise renders one with ImageMagick or ffmpeg and caches it.
pub async fn fs_thumb(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Result<Response, ApiError> {
    authorize(&st, &headers)?;
    let (path, _) = resolve(&home()?, &q.path, q.hidden)?;
    let meta = tokio::fs::metadata(&path).await?;
    if !meta.is_file() {
        return Err(err(StatusCode::BAD_REQUEST, "not a file"));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let video = VIDEO_EXTS.contains(&ext.as_str());
    if !video && !IMAGE_EXTS.contains(&ext.as_str()) {
        return Err(err(StatusCode::NOT_FOUND, "no preview for this type"));
    }
    let modified = meta.modified().unwrap_or(UNIX_EPOCH);

    let px = q.size.unwrap_or(256).clamp(64, 1024);
    // The desktop's own thumbnails are 256 px; bigger requests are rendered here.
    if px <= 256
        && let Some(png) = freedesktop_thumb(&path, modified).await
    {
        return Ok(image_response(png, "image/png"));
    }
    let cache = thumb_cache_path(&path, modified, meta.len(), px);
    if let Ok(jpg) = tokio::fs::read(&cache).await {
        return Ok(image_response(jpg, "image/jpeg"));
    }
    let _slot = THUMB_SLOTS.acquire().await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let jpg = if video { video_thumb(&path, px).await } else { image_thumb(&path, px).await }
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "couldn't render a preview"))?;
    if let Some(dir) = cache.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
        let _ = tokio::fs::write(&cache, &jpg).await;
    }
    Ok(image_response(jpg, "image/jpeg"))
}

fn image_response(bytes: Vec<u8>, mime: &'static str) -> Response {
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
    resp
}

/// file:// URI as freedesktop thumbnailers hash it (path percent-encoded, '/' kept).
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b'!' | b'$' | b'&' | b'\''
            | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@' => uri.push(b as char),
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

async fn freedesktop_thumb(path: &Path, modified: SystemTime) -> Option<Vec<u8>> {
    use md5::{Digest, Md5};
    let base = directories::BaseDirs::new()?.cache_dir().join("thumbnails");
    let name = format!("{}.png", hex::encode(Md5::digest(file_uri(path).as_bytes())));
    for size in ["large", "x-large", "normal"] {
        let thumb = base.join(size).join(&name);
        let Ok(meta) = tokio::fs::metadata(&thumb).await else { continue };
        // Stale if the file changed after the thumbnail was made.
        if meta.modified().is_ok_and(|t| t >= modified) {
            return tokio::fs::read(&thumb).await.ok();
        }
    }
    None
}

fn thumb_cache_path(path: &Path, modified: SystemTime, size: u64, px: u32) -> PathBuf {
    use sha2::{Digest, Sha256};
    let secs = modified.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let key = hex::encode(Sha256::digest(format!("{}\0{secs}\0{size}\0{px}", path.display()).as_bytes()));
    let base = directories::BaseDirs::new().map_or_else(std::env::temp_dir, |b| b.cache_dir().to_path_buf());
    base.join("vulcan/thumbs").join(format!("{}.jpg", &key[..32]))
}

async fn run_thumb(mut cmd: tokio::process::Command) -> Option<Vec<u8>> {
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

async fn image_thumb(path: &Path, px: u32) -> Option<Vec<u8>> {
    // [0]: first frame only (GIFs, multi-page TIFFs).
    let mut input = path.as_os_str().to_owned();
    input.push("[0]");
    let mut cmd = tokio::process::Command::new("magick");
    cmd.arg(input).args(["-auto-orient", "-thumbnail", &format!("{px}x{px}>"), "-quality", "80", "jpg:-"]);
    run_thumb(cmd).await
}

async fn video_thumb(path: &Path, px: u32) -> Option<Vec<u8>> {
    // Start 1 s in to skip black intros; very short clips fall back to the start.
    for seek in ["1", "0"] {
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args(["-v", "error", "-ss", seek, "-i"]).arg(path).args([
            "-frames:v",
            "1",
            "-vf",
            // Pick a representative frame from the next ~3 s, not a black fade.
            &format!("thumbnail=90,scale={px}:{px}:force_original_aspect_ratio=decrease"),
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "-",
        ]);
        if let Some(jpg) = run_thumb(cmd).await {
            return Some(jpg);
        }
    }
    None
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
    fn received_message_batches() {
        let one = [PathBuf::from("/x/a.jpg")];
        assert_eq!(received_message(&one), ("Received from phone".into(), "a.jpg".into()));
        let many: Vec<PathBuf> = ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"].iter().map(|n| PathBuf::from("/x").join(n)).collect();
        assert_eq!(received_message(&many), ("Received 5 files from phone".into(), "a.jpg, b.jpg, c.jpg and 2 more".into()));
    }

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
    fn uri_for_thumbnail_cache() {
        assert_eq!(file_uri(Path::new("/home/a/My Pics/ü.jpg")), "file:///home/a/My%20Pics/%C3%BC.jpg");
        assert_eq!(file_uri(Path::new("/home/a/x(1)+y.png")), "file:///home/a/x(1)+y.png");
    }

    #[test]
    fn walk_keeps_structure_and_skips_hidden() {
        let home = std::env::temp_dir().join(format!("vulcan-walk-{}", std::process::id()));
        std::fs::create_dir_all(home.join("Pics/2024")).unwrap();
        std::fs::create_dir_all(home.join("Pics/.cache")).unwrap();
        std::fs::write(home.join("Pics/a.jpg"), "aa").unwrap();
        std::fs::write(home.join("Pics/2024/b.jpg"), "bbb").unwrap();
        std::fs::write(home.join("Pics/.cache/c"), "c").unwrap();
        std::os::unix::fs::symlink(home.join("Pics"), home.join("Pics/2024/loop")).unwrap();
        let home = home.canonicalize().unwrap();

        let w = walk_dir(&home, &home.join("Pics"), false);
        let mut rels: Vec<&str> = w.files.iter().map(|f| f.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, ["Pics/2024/b.jpg", "Pics/a.jpg"], "no hidden files, symlinked folder not followed");
        assert_eq!(w.total, 5);
        assert!(w.files.iter().any(|f| f.path == "Pics/2024/b.jpg"));
        assert_eq!(walk_dir(&home, &home.join("Pics"), true).files.len(), 3);
        std::fs::remove_dir_all(&home).unwrap();
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
