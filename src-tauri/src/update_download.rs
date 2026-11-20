// src-tauri/src/update_download.rs
//
// Downloads the latest published installer for Settings → Updates, checks it,
// and remembers the one verified file the app may hand to Installer.
//
// Flow (driven by the `update_download` command in main.rs):
//   fresh update check → `pick` (needs an update and a pkg; URL rules; size
//   cap) → optional `.sha256` fetch → `stream_to_part` (`<name>.part` in the
//   Downloads folder, progress, cancel, size checks) → `verify_checksum`
//   → `finalize` (never overwrites: `<name>`, `<stem> (1).<ext>`, …) →
//   `DownloadState::set_verified`.
//
// Safety rules:
//  - URLs come only from GitHub's reply, never from the frontend, and must be
//    `https://github.com/jaylenjupp/it-hurts-when-ip/releases/download/<tag>/<name>`.
//    Redirects are followed only to https, at most 10.
//  - Every exit after the `.part` file exists removes it (`PartFile` guard,
//    also on panic).
//  - Opening Installer is platform code (`platform::open_pkg`); debug builds
//    never open it.
//
// Platform-neutral: the checksum and open steps are passed in as functions.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::update::{AssetInfo, UpdateError, UpdateInfo, USER_AGENT};

/// Largest installer we accept (bytes).
pub const MAX_PKG_BYTES: u64 = 200 * 1024 * 1024;
/// Largest `.sha256` file we read (bytes).
const MAX_SHA256_BYTES: usize = 4096;
/// Redirects allowed per request.
const MAX_REDIRECTS: usize = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// No bytes for this long → the download is abandoned.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const SHA256_TIMEOUT: Duration = Duration::from_secs(10);
/// At most ~10 progress events per second.
const PROGRESS_GAP: Duration = Duration::from_millis(100);
/// Name tried after `<stem> (999).<ext>` is taken: give up.
const MAX_NAME_SUFFIX: u32 = 999;

/// Tauri event name for download progress.
pub const PROGRESS_EVENT: &str = "update-progress";

// --- Errors ---

/// Why a download or open failed. Serialised as `{kind, message[, path]}`.
/// Errors from the fresh update check are passed through unchanged
/// (`offline`, `rate_limited`, `no_releases`, `http`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Check(UpdateError),
    Offline,
    Stalled,
    Http(u16),
    NoUpdate,
    NoPkg,
    UrlRefused(&'static str),
    TooLarge(u64),
    SizeMismatch { expected: u64, got: u64 },
    ChecksumMismatch,
    ChecksumUnreadable(String),
    Disk { path: String, detail: String },
    Cancelled,
    Busy,
    NotDownloaded,
    OpenFailed(String),
}

impl DownloadError {
    pub fn kind(&self) -> &'static str {
        match self {
            DownloadError::Check(e) => e.kind(),
            DownloadError::Offline | DownloadError::Stalled => "offline",
            DownloadError::Http(_) => "http",
            DownloadError::NoUpdate => "no_update",
            DownloadError::NoPkg => "no_pkg",
            DownloadError::UrlRefused(_) => "url_refused",
            DownloadError::TooLarge(_) => "too_large",
            DownloadError::SizeMismatch { .. } => "size_mismatch",
            DownloadError::ChecksumMismatch => "checksum_mismatch",
            DownloadError::ChecksumUnreadable(_) => "checksum_unreadable",
            DownloadError::Disk { .. } => "disk",
            DownloadError::Cancelled => "cancelled",
            DownloadError::Busy => "busy",
            DownloadError::NotDownloaded => "not_downloaded",
            DownloadError::OpenFailed(_) => "open_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            DownloadError::Check(e) => e.message(),
            DownloadError::Offline => {
                "Can't reach GitHub. Check your internet connection.".to_string()
            }
            DownloadError::Stalled => "The download stalled (no data for 30 seconds). \
                 Check your internet connection and try again."
                .to_string(),
            DownloadError::Http(s) => format!("GitHub returned an error (HTTP {}).", s),
            DownloadError::NoUpdate => "There is no newer version to download.".to_string(),
            DownloadError::NoPkg => {
                "The latest release has no macOS installer to download.".to_string()
            }
            DownloadError::UrlRefused(why) => {
                format!("The download address was refused ({}).", why)
            }
            DownloadError::TooLarge(size) => format!(
                "The installer is too large ({} MB; the limit is {} MB).",
                size.div_ceil(1024 * 1024),
                MAX_PKG_BYTES / (1024 * 1024)
            ),
            DownloadError::SizeMismatch { expected, got } => format!(
                "The download was the wrong size ({} bytes instead of {}). It was deleted. Try again.",
                got, expected
            ),
            DownloadError::ChecksumMismatch => {
                "The download doesn't match its published checksum. It was deleted. Try again."
                    .to_string()
            }
            DownloadError::ChecksumUnreadable(why) => format!(
                "The published checksum couldn't be checked ({}). Nothing was kept.",
                why
            ),
            DownloadError::Disk { path, detail } => {
                format!("Couldn't save the download ({}): {}", path, detail)
            }
            DownloadError::Cancelled => "Download cancelled.".to_string(),
            DownloadError::Busy => "A download is already running.".to_string(),
            DownloadError::NotDownloaded => {
                "There is no verified download to open. Download the update again.".to_string()
            }
            DownloadError::OpenFailed(why) => why.clone(),
        }
    }

    fn disk(path: &Path, e: impl std::fmt::Display) -> Self {
        DownloadError::Disk {
            path: path.display().to_string(),
            detail: e.to_string(),
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl Serialize for DownloadError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let DownloadError::Check(e) = self {
            return e.serialize(serializer);
        }
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("kind", self.kind())?;
        map.serialize_entry("message", &self.message())?;
        match self {
            DownloadError::Disk { path, .. } => map.serialize_entry("path", path)?,
            DownloadError::Http(status) => map.serialize_entry("status", status)?,
            _ => {}
        }
        map.end()
    }
}

// --- Results the UI receives ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadResult {
    pub path: String,
    pub size: u64,
    /// "verified" or "not_published" (size check only).
    pub checksum: &'static str,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenResult {
    /// Release builds: Installer was asked to open the pkg.
    Opened { path: String },
    /// Debug builds: nothing ran; this is what a release build would run.
    Debug { would_run: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub received: u64,
    pub total: u64,
}

// --- URL rules ---

/// The pkg and `.sha256` URLs must be GitHub release downloads of this repo:
/// `https://github.com/jaylenjupp/it-hurts-when-ip/releases/download/<tag>/<name>`
/// (owner and repo compared case-insensitively). No user info, port, query
/// or fragment.
pub fn url_allowed(s: &str) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(s).map_err(|_| "not a valid address")?;
    if url.scheme() != "https" {
        return Err("not https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("contains a user name");
    }
    if url.port().is_some() {
        return Err("unexpected port");
    }
    if url.host_str() != Some("github.com") {
        return Err("not github.com");
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("unexpected query");
    }
    // `Url` has already resolved any `.`/`..` segments.
    let segs: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();
    let ok = segs.len() == 6
        && segs[0].eq_ignore_ascii_case("jaylenjupp")
        && segs[1].eq_ignore_ascii_case("it-hurts-when-ip")
        && segs[2] == "releases"
        && segs[3] == "download"
        && !segs[4].is_empty()
        && !segs[5].is_empty();
    if ok {
        Ok(())
    } else {
        Err("not a release download of this app")
    }
}

/// Redirect rule: https only, at most MAX_REDIRECTS. `redirect_number` is
/// 1 for the first redirect (reqwest's `attempt.previous().len()`).
pub fn redirect_allowed(next: &reqwest::Url, redirect_number: usize) -> Result<(), &'static str> {
    if redirect_number > MAX_REDIRECTS {
        return Err("too many redirects");
    }
    if next.scheme() != "https" {
        return Err("redirect to a non-https address");
    }
    Ok(())
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        match redirect_allowed(attempt.url(), attempt.previous().len()) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    })
}

/// The asset name becomes a file name in the Downloads folder: a plain
/// `.pkg` name only.
fn check_name(name: &str) -> Result<(), DownloadError> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
        && name.to_ascii_lowercase().ends_with(".pkg")
        && name.len() <= 200;
    if ok {
        Ok(())
    } else {
        Err(DownloadError::UrlRefused("unexpected file name"))
    }
}

/// Pure: what to download, from a fresh check. Needs a newer version and a
/// pkg; both URLs must pass `url_allowed`; size within the cap.
pub fn pick(info: &UpdateInfo) -> Result<(AssetInfo, Option<String>), DownloadError> {
    if !info.update_available {
        return Err(DownloadError::NoUpdate);
    }
    let pkg = info.pkg.clone().ok_or(DownloadError::NoPkg)?;
    check_name(&pkg.name)?;
    url_allowed(&pkg.url).map_err(DownloadError::UrlRefused)?;
    if let Some(u) = &info.sha256_url {
        url_allowed(u).map_err(DownloadError::UrlRefused)?;
    }
    if pkg.size > MAX_PKG_BYTES {
        return Err(DownloadError::TooLarge(pkg.size));
    }
    if pkg.size == 0 {
        return Err(DownloadError::SizeMismatch {
            expected: 0,
            got: 0,
        });
    }
    Ok((pkg, info.sha256_url.clone()))
}

// --- Checksum file ---

/// First whitespace-separated token that is exactly 64 hex digits, lower
/// case. Accepts `<hash>`, `<hash>  <file>` and `SHA256 (<file>) = <hash>`.
pub fn parse_sha256_text(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|t| t.to_ascii_lowercase())
}

// --- Byte source ---

/// Where the bytes come from. The app uses the HTTP response; tests feed
/// fake chunks.
pub trait ChunkSource {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, DownloadError>;
}

pub struct ReqwestSource(reqwest::Response);

impl ChunkSource for ReqwestSource {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, DownloadError> {
        self.0
            .chunk()
            .await
            .map(|c| c.map(|b| b.to_vec()))
            .map_err(map_transport)
    }
}

fn map_transport(e: reqwest::Error) -> DownloadError {
    eprintln!("[update] download error: {}", e);
    if e.is_redirect() {
        DownloadError::UrlRefused("redirect refused")
    } else if e.is_timeout() {
        DownloadError::Stalled
    } else {
        DownloadError::Offline
    }
}

pub fn build_client() -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .redirect(redirect_policy())
        .build()
        .map_err(|e| {
            eprintln!("[update] failed to build http client: {}", e);
            DownloadError::Offline
        })
}

/// Starts the pkg request. A declared length that differs from the asset size
/// is refused before anything is written.
pub async fn open_pkg_stream(
    client: &reqwest::Client,
    url: &str,
    size: u64,
) -> Result<ReqwestSource, DownloadError> {
    url_allowed(url).map_err(DownloadError::UrlRefused)?;
    let resp = client.get(url).send().await.map_err(map_transport)?;
    if !resp.status().is_success() {
        return Err(DownloadError::Http(resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len != size {
            return Err(DownloadError::SizeMismatch {
                expected: size,
                got: len,
            });
        }
    }
    Ok(ReqwestSource(resp))
}

/// Fetches and parses `<pkg>.sha256` (≤ 4 KB).
pub async fn fetch_sha256(client: &reqwest::Client, url: &str) -> Result<String, DownloadError> {
    url_allowed(url).map_err(DownloadError::UrlRefused)?;
    let unreadable = |s: &str| DownloadError::ChecksumUnreadable(s.to_string());
    let mut resp = client
        .get(url)
        .timeout(SHA256_TIMEOUT)
        .send()
        .await
        .map_err(map_transport)?;
    if !resp.status().is_success() {
        return Err(DownloadError::ChecksumUnreadable(format!(
            "HTTP {}",
            resp.status().as_u16()
        )));
    }
    if resp.content_length().unwrap_or(0) > MAX_SHA256_BYTES as u64 {
        return Err(unreadable("checksum file too large"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(map_transport)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_SHA256_BYTES {
            return Err(unreadable("checksum file too large"));
        }
    }
    let text = String::from_utf8(body).map_err(|_| unreadable("checksum file isn't text"))?;
    parse_sha256_text(&text).ok_or_else(|| unreadable("no checksum in the file"))
}

// --- Writing the file ---

/// A `.part` file that is deleted when dropped (every error path, cancel and
/// panic), unless `finalize` has linked it to its final name.
#[derive(Debug)]
pub struct PartFile {
    path: PathBuf,
}

impl PartFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub fn for_test(path: PathBuf) -> Self {
        PartFile { path }
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Emits at most once per `PROGRESS_GAP`.
struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < PROGRESS_GAP => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// The Downloads folder must already exist; it is never created.
fn check_dir(dir: &Path) -> Result<(), DownloadError> {
    match fs::metadata(dir) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(DownloadError::disk(dir, "not a folder")),
        Err(e) => Err(DownloadError::disk(dir, e)),
    }
}

/// Removes a `.part` left by an earlier run (a file or a symlink itself;
/// a folder is an error).
fn remove_leftover(path: &Path) -> Result<(), DownloadError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DownloadError::disk(path, e)),
        Ok(m) if m.is_dir() => Err(DownloadError::disk(path, "a folder has this name")),
        Ok(_) => fs::remove_file(path).map_err(|e| DownloadError::disk(path, e)),
    }
}

fn create_part(path: &Path) -> Result<fs::File, DownloadError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o644);
    }
    opts.open(path).map_err(|e| DownloadError::disk(path, e))
}

/// Streams `src` into `<dir>/<name>.part`. Checks the cancel flag between
/// chunks, refuses more bytes than `size`, and checks the final size.
/// `progress(received, total)` is throttled, plus one final call.
pub async fn stream_to_part<S: ChunkSource>(
    src: &mut S,
    dir: &Path,
    name: &str,
    size: u64,
    cancel: &AtomicBool,
    mut now: impl FnMut() -> Instant,
    mut progress: impl FnMut(u64, u64),
) -> Result<PartFile, DownloadError> {
    if size > MAX_PKG_BYTES {
        return Err(DownloadError::TooLarge(size));
    }
    check_name(name)?;
    check_dir(dir)?;
    let path = dir.join(format!("{}.part", name));
    remove_leftover(&path)?;
    let mut file = create_part(&path)?;
    let part = PartFile { path };

    let mut received: u64 = 0;
    let mut throttle = Throttle { last: None };
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let Some(chunk) = src.next_chunk().await? else {
            break;
        };
        received = received.saturating_add(chunk.len() as u64);
        if received > size {
            return Err(DownloadError::SizeMismatch {
                expected: size,
                got: received,
            });
        }
        file.write_all(&chunk)
            .map_err(|e| DownloadError::disk(&part.path, e))?;
        if throttle.ready(now()) {
            progress(received, size);
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(DownloadError::Cancelled);
    }
    file.sync_all()
        .map_err(|e| DownloadError::disk(&part.path, e))?;
    drop(file);
    let on_disk = fs::metadata(&part.path)
        .map_err(|e| DownloadError::disk(&part.path, e))?
        .len();
    if received != size || on_disk != size {
        return Err(DownloadError::SizeMismatch {
            expected: size,
            got: received.max(on_disk),
        });
    }
    progress(received, size);
    Ok(part)
}

/// "verified" when `expected` is given and matches, "not_published" when
/// there is no checksum. The caller drops `part` on error (file deleted).
pub fn verify_checksum(
    part: &PartFile,
    expected: Option<&str>,
    hasher: impl FnOnce(&Path) -> Result<String, String>,
) -> Result<&'static str, DownloadError> {
    let Some(expected) = expected else {
        return Ok("not_published");
    };
    let actual = hasher(&part.path).map_err(DownloadError::ChecksumUnreadable)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok("verified")
    } else {
        Err(DownloadError::ChecksumMismatch)
    }
}

/// `name`, then `<stem> (1).<ext>`, `(2)`, … (n = 0 is the plain name).
pub fn candidate_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({}){}", &name[..i], n, &name[i..]),
        _ => format!("{} ({})", name, n),
    }
}

/// Gives the `.part` its final name without ever overwriting: a hard link
/// fails atomically if the name exists, then the next name is tried. The
/// `.part` is removed afterwards.
pub fn finalize(part: PartFile, dir: &Path, name: &str) -> Result<PathBuf, DownloadError> {
    for n in 0..=MAX_NAME_SUFFIX {
        let target = dir.join(candidate_name(name, n));
        match fs::hard_link(&part.path, &target) {
            Ok(()) => {
                drop(part);
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(DownloadError::disk(&target, e)),
        }
    }
    Err(DownloadError::disk(
        &dir.join(name),
        "too many files with this name",
    ))
}

// --- App state ---

#[derive(Debug, Clone, PartialEq, Eq)]
struct Verified {
    path: PathBuf,
    size: u64,
}

/// One download at a time, its cancel flag, and the last verified file.
#[derive(Default)]
pub struct DownloadState {
    busy: AtomicBool,
    cancel: AtomicBool,
    verified: Mutex<Option<Verified>>,
}

/// Clears `busy` on every exit.
pub struct BusyGuard<'a>(&'a DownloadState);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

impl DownloadState {
    /// Starts a download: refuses a second one, clears the cancel flag and
    /// forgets the previous verified file.
    pub fn begin(&self) -> Result<BusyGuard<'_>, DownloadError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(DownloadError::Busy);
        }
        self.cancel.store(false, Ordering::SeqCst);
        *self.verified.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(BusyGuard(self))
    }

    /// Asks the running download to stop. False if none is running.
    pub fn cancel(&self) -> bool {
        if self.busy.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub fn set_verified(&self, path: PathBuf, size: u64) {
        *self.verified.lock().unwrap_or_else(|e| e.into_inner()) = Some(Verified { path, size });
    }

    /// Opens only the stored verified file, and only if it is still a regular
    /// file of the same size; otherwise it is forgotten.
    pub fn open_verified(
        &self,
        opener: impl FnOnce(&Path) -> Result<OpenResult, String>,
    ) -> Result<OpenResult, DownloadError> {
        let mut slot = self.verified.lock().unwrap_or_else(|e| e.into_inner());
        let Some(v) = slot.clone() else {
            return Err(DownloadError::NotDownloaded);
        };
        let still_there = fs::symlink_metadata(&v.path)
            .map(|m| m.is_file() && m.len() == v.size)
            .unwrap_or(false);
        if !still_there {
            *slot = None;
            return Err(DownloadError::NotDownloaded);
        }
        drop(slot);
        opener(&v.path).map_err(DownloadError::OpenFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    const DL: &str = "https://github.com/jaylenjupp/it-hurts-when-ip/releases/download";
    const NAME: &str = "ItHurtsWhenIP-1.14.0.pkg";

    /// Temp folder removed on drop. Never the real Downloads folder.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "ihwip-download-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::remove_dir_all(&p).ok();
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    struct FakeSource {
        chunks: VecDeque<Result<Vec<u8>, DownloadError>>,
        served: usize,
        /// After serving this many chunks, set the flag (a user pressing
        /// Cancel mid-way).
        cancel_after: Option<(usize, Arc<AtomicBool>)>,
    }

    impl FakeSource {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            FakeSource {
                chunks: chunks.into_iter().map(Ok).collect(),
                served: 0,
                cancel_after: None,
            }
        }
    }

    impl ChunkSource for FakeSource {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, DownloadError> {
            if let Some((n, flag)) = &self.cancel_after {
                if self.served == *n {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            match self.chunks.pop_front() {
                None => Ok(None),
                Some(r) => {
                    self.served += 1;
                    r.map(Some)
                }
            }
        }
    }

    /// Clock that moves `step_ms` forward on every call.
    fn clock(step_ms: u64) -> impl FnMut() -> Instant {
        let mut t = Instant::now();
        move || {
            t += Duration::from_millis(step_ms);
            t
        }
    }

    fn run<S: ChunkSource>(
        src: &mut S,
        dir: &Path,
        size: u64,
        cancel: &AtomicBool,
    ) -> (Result<PartFile, DownloadError>, Vec<(u64, u64)>) {
        let mut seen = Vec::new();
        let r = tauri::async_runtime::block_on(stream_to_part(
            src,
            dir,
            NAME,
            size,
            cancel,
            clock(30),
            |a, b| seen.push((a, b)),
        ));
        (r, seen)
    }

    fn chunks(n: usize, len: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| vec![i as u8; len]).collect()
    }

    fn info(available: bool, pkg: Option<AssetInfo>, sha: Option<&str>) -> UpdateInfo {
        UpdateInfo {
            current_version: "1.13.1".into(),
            latest_version: "1.14.0".into(),
            update_available: available,
            release_url: "x".into(),
            download_url: None,
            published_at: None,
            pkg,
            sha256_url: sha.map(str::to_string),
            checked_at: 0,
        }
    }

    fn asset(name: &str, size: u64, url: &str) -> AssetInfo {
        AssetInfo {
            name: name.into(),
            size,
            url: url.into(),
        }
    }

    // --- URL rules ---

    #[test]
    fn url_rule_table() {
        let good = [
            format!("{}/v1.14.0/{}", DL, NAME),
            format!("{}/v1.14.0/{}.sha256", DL, NAME),
            format!(
                "https://github.com/jaylenjupp/It-Hurts-When-IP/releases/download/v1.14.0/{}",
                NAME
            ),
            format!(
                "https://GitHub.com/JaylenJupp/it-hurts-when-ip/releases/download/v1.14.0/{}",
                NAME
            ),
            format!(
                "https://github.com:443/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
        ];
        for u in &good {
            assert_eq!(url_allowed(u), Ok(()), "{}", u);
        }
        let bad = [
            format!(
                "http://github.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://objects.githubusercontent.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://github.com.evil.example/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://evilgithub.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://github.com/someone/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://github.com/jaylenjupp/other-repo/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://github.com/jaylenjupp/it-hurts-when-ip/releases/tag/v1/{}",
                NAME
            ),
            "https://github.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/".to_string(),
            format!(
                "https://github.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/../../../x/{}",
                NAME
            ),
            format!(
                "https://github.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/a/{}",
                NAME
            ),
            format!(
                "https://user@github.com/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!(
                "https://github.com:8443/jaylenjupp/it-hurts-when-ip/releases/download/v1/{}",
                NAME
            ),
            format!("{}/v1/{}?x=1", DL, NAME),
            format!("{}/v1/{}#frag", DL, NAME),
            "file:///Users/x/a.pkg".to_string(),
            "not a url".to_string(),
            String::new(),
        ];
        for u in &bad {
            assert!(url_allowed(u).is_err(), "should refuse {}", u);
        }
    }

    #[test]
    fn redirect_rule() {
        let https = reqwest::Url::parse("https://objects.githubusercontent.com/x").unwrap();
        let http = reqwest::Url::parse("http://objects.githubusercontent.com/x").unwrap();
        assert_eq!(redirect_allowed(&https, 1), Ok(()));
        assert_eq!(redirect_allowed(&https, 10), Ok(()));
        assert_eq!(redirect_allowed(&https, 11), Err("too many redirects"));
        assert_eq!(
            redirect_allowed(&http, 1),
            Err("redirect to a non-https address")
        );
    }

    #[test]
    fn pick_rules() {
        let url = format!("{}/v1.14.0/{}", DL, NAME);
        let sha = format!("{}/v1.14.0/{}.sha256", DL, NAME);
        let p = asset(NAME, 10, &url);

        assert_eq!(
            pick(&info(false, Some(p.clone()), None)),
            Err(DownloadError::NoUpdate)
        );
        assert_eq!(pick(&info(true, None, None)), Err(DownloadError::NoPkg));
        assert_eq!(
            pick(&info(true, Some(p.clone()), Some(&sha))),
            Ok((p.clone(), Some(sha.clone())))
        );
        assert_eq!(
            pick(&info(true, Some(p.clone()), None)),
            Ok((p.clone(), None))
        );
        assert!(matches!(
            pick(&info(
                true,
                Some(asset(NAME, 10, "http://github.com/x")),
                None
            )),
            Err(DownloadError::UrlRefused(_))
        ));
        assert!(matches!(
            pick(&info(
                true,
                Some(p.clone()),
                Some("https://example.com/x.sha256")
            )),
            Err(DownloadError::UrlRefused(_))
        ));
        assert_eq!(
            pick(&info(
                true,
                Some(asset(NAME, MAX_PKG_BYTES + 1, &url)),
                None
            )),
            Err(DownloadError::TooLarge(MAX_PKG_BYTES + 1))
        );
        for bad in ["../x.pkg", ".hidden.pkg", "a/b.pkg", "x.dmg", ""] {
            assert!(
                matches!(
                    pick(&info(true, Some(asset(bad, 10, &url)), None)),
                    Err(DownloadError::UrlRefused(_))
                ),
                "{}",
                bad
            );
        }
    }

    // --- sha256 parsing ---

    #[test]
    fn sha256_text_parsing() {
        let h = "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b";
        assert_eq!(parse_sha256_text(h).as_deref(), Some(h));
        assert_eq!(
            parse_sha256_text(&format!("{}  {}\n", h, NAME)).as_deref(),
            Some(h)
        );
        assert_eq!(
            parse_sha256_text(&h.to_uppercase()).as_deref(),
            Some(h),
            "uppercase is lowered"
        );
        assert_eq!(
            parse_sha256_text(&format!("SHA256 ({}) = {}", NAME, h)).as_deref(),
            Some(h)
        );
        assert_eq!(parse_sha256_text(&h[..63]), None);
        assert_eq!(parse_sha256_text(&format!("{}0", h)), None);
        assert_eq!(parse_sha256_text(&format!("{}g", &h[..63])), None);
        assert_eq!(parse_sha256_text("garbage <html>"), None);
        assert_eq!(parse_sha256_text(""), None);
    }

    // --- streaming ---

    #[test]
    fn stream_success_writes_bytes_and_throttles_progress() {
        let dir = TempDir::new();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(chunks(10, 7));
        let (r, seen) = run(&mut src, &dir.0, 70, &cancel);
        let part = r.unwrap();
        assert_eq!(part.path(), dir.0.join(format!("{}.part", NAME)));
        let bytes = fs::read(part.path()).unwrap();
        assert_eq!(bytes, chunks(10, 7).concat());
        // Clock: 30 ms per chunk → emits at 30, 150, 270 ms, then the final.
        assert_eq!(seen, vec![(7, 70), (35, 70), (63, 70), (70, 70)]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(part.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode & !0o644, 0, "mode {:o}", mode);
        }
        drop(part);
        assert!(dir.entries().is_empty(), "guard removes the .part");
    }

    #[test]
    fn stream_short_is_size_mismatch_and_part_removed() {
        let dir = TempDir::new();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(chunks(3, 10));
        let (r, _) = run(&mut src, &dir.0, 40, &cancel);
        assert_eq!(
            r.unwrap_err(),
            DownloadError::SizeMismatch {
                expected: 40,
                got: 30
            }
        );
        assert!(dir.entries().is_empty());
    }

    #[test]
    fn stream_long_is_size_mismatch_and_part_removed() {
        let dir = TempDir::new();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(chunks(5, 10));
        let (r, _) = run(&mut src, &dir.0, 40, &cancel);
        assert_eq!(
            r.unwrap_err(),
            DownloadError::SizeMismatch {
                expected: 40,
                got: 50
            }
        );
        assert_eq!(src.served, 5);
        assert!(dir.entries().is_empty());
    }

    #[test]
    fn stream_cancel_midway_removes_part() {
        let dir = TempDir::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut src = FakeSource::new(chunks(10, 10));
        src.cancel_after = Some((4, cancel.clone()));
        let (r, _) = run(&mut src, &dir.0, 100, &cancel);
        assert_eq!(r.unwrap_err(), DownloadError::Cancelled);
        assert!(
            src.served <= 5,
            "stops between chunks, served {}",
            src.served
        );
        assert!(dir.entries().is_empty());
    }

    #[test]
    fn stream_error_removes_part() {
        let dir = TempDir::new();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(chunks(2, 10));
        src.chunks.push_back(Err(DownloadError::Stalled));
        let (r, _) = run(&mut src, &dir.0, 100, &cancel);
        assert_eq!(r.unwrap_err(), DownloadError::Stalled);
        assert!(dir.entries().is_empty());
    }

    #[test]
    fn leftover_part_is_replaced() {
        let dir = TempDir::new();
        let leftover = dir.0.join(format!("{}.part", NAME));
        fs::write(&leftover, b"junk from last time, longer than the new file").unwrap();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(vec![b"abc".to_vec()]);
        let (r, _) = run(&mut src, &dir.0, 3, &cancel);
        let part = r.unwrap();
        assert_eq!(fs::read(part.path()).unwrap(), b"abc");
    }

    #[test]
    fn leftover_part_folder_is_disk_error() {
        let dir = TempDir::new();
        fs::create_dir(dir.0.join(format!("{}.part", NAME))).unwrap();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(vec![b"abc".to_vec()]);
        let (r, _) = run(&mut src, &dir.0, 3, &cancel);
        assert_eq!(r.unwrap_err().kind(), "disk");
        assert_eq!(src.served, 0);
    }

    #[test]
    fn too_large_creates_nothing() {
        let dir = TempDir::new();
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(vec![b"abc".to_vec()]);
        let (r, _) = run(&mut src, &dir.0, MAX_PKG_BYTES + 1, &cancel);
        assert_eq!(r.unwrap_err(), DownloadError::TooLarge(MAX_PKG_BYTES + 1));
        assert!(dir.entries().is_empty());
        assert_eq!(src.served, 0);
    }

    #[test]
    fn missing_downloads_folder_is_disk_error_and_not_created() {
        let dir = TempDir::new();
        let missing = dir.0.join("Downloads");
        let cancel = AtomicBool::new(false);
        let mut src = FakeSource::new(vec![b"abc".to_vec()]);
        let (r, _) = run(&mut src, &missing, 3, &cancel);
        let e = r.unwrap_err();
        assert_eq!(e.kind(), "disk");
        assert!(e.message().contains(&missing.display().to_string()));
        assert!(!missing.exists());

        let file = dir.0.join("afile");
        fs::write(&file, b"x").unwrap();
        let mut src = FakeSource::new(vec![b"abc".to_vec()]);
        let (r, _) = run(&mut src, &file, 3, &cancel);
        assert_eq!(r.unwrap_err().kind(), "disk");
    }

    // --- checksum ---

    #[test]
    fn verify_checksum_outcomes() {
        let dir = TempDir::new();
        let p = dir.0.join("x.part");
        fs::write(&p, b"x").unwrap();
        let part = PartFile::for_test(p.clone());
        let h = "ab".repeat(32);
        assert_eq!(
            verify_checksum(&part, None, |_| panic!("not called")),
            Ok("not_published")
        );
        assert_eq!(
            verify_checksum(&part, Some(&h.to_uppercase()), |_| Ok(h.clone())),
            Ok("verified")
        );
        assert_eq!(
            verify_checksum(&part, Some(&h), |_| Ok("cd".repeat(32))),
            Err(DownloadError::ChecksumMismatch)
        );
        assert_eq!(
            verify_checksum(&part, Some(&h), |_| Err("shasum failed.".into())),
            Err(DownloadError::ChecksumUnreadable("shasum failed.".into()))
        );
        drop(part);
        assert!(!p.exists());
    }

    // --- final name ---

    #[test]
    fn candidate_names() {
        assert_eq!(candidate_name(NAME, 0), NAME);
        assert_eq!(candidate_name(NAME, 1), "ItHurtsWhenIP-1.14.0 (1).pkg");
        assert_eq!(candidate_name(NAME, 12), "ItHurtsWhenIP-1.14.0 (12).pkg");
        assert_eq!(candidate_name("noext", 2), "noext (2)");
    }

    #[test]
    fn finalize_picks_free_name_and_never_overwrites() {
        let dir = TempDir::new();
        fs::write(dir.0.join(NAME), b"mine-0").unwrap();
        fs::write(dir.0.join("ItHurtsWhenIP-1.14.0 (1).pkg"), b"mine-1").unwrap();
        let p = dir.0.join(format!("{}.part", NAME));
        fs::write(&p, b"new").unwrap();
        let out = finalize(PartFile::for_test(p.clone()), &dir.0, NAME).unwrap();
        assert_eq!(out, dir.0.join("ItHurtsWhenIP-1.14.0 (2).pkg"));
        assert_eq!(fs::read(&out).unwrap(), b"new");
        assert_eq!(fs::read(dir.0.join(NAME)).unwrap(), b"mine-0");
        assert_eq!(
            fs::read(dir.0.join("ItHurtsWhenIP-1.14.0 (1).pkg")).unwrap(),
            b"mine-1"
        );
        assert!(!p.exists(), ".part removed");
        assert_eq!(dir.entries().len(), 3);
    }

    #[test]
    fn finalize_plain_name_when_free() {
        let dir = TempDir::new();
        let p = dir.0.join(format!("{}.part", NAME));
        fs::write(&p, b"new").unwrap();
        let out = finalize(PartFile::for_test(p), &dir.0, NAME).unwrap();
        assert_eq!(out, dir.0.join(NAME));
        assert_eq!(dir.entries(), vec![NAME.to_string()]);
    }

    // --- state ---

    #[test]
    fn one_download_at_a_time() {
        let s = DownloadState::default();
        assert!(!s.cancel(), "nothing running");
        let g = s.begin().unwrap();
        assert_eq!(s.begin().err(), Some(DownloadError::Busy));
        assert_eq!(
            DownloadError::Busy.message(),
            "A download is already running."
        );
        assert!(s.cancel());
        assert!(s.cancel_flag().load(Ordering::SeqCst));
        drop(g);
        let _g = s.begin().unwrap();
        assert!(
            !s.cancel_flag().load(Ordering::SeqCst),
            "begin clears cancel"
        );
    }

    #[test]
    fn open_refuses_unknown_missing_or_changed_file() {
        let dir = TempDir::new();
        let s = DownloadState::default();
        let never = |_: &Path| -> Result<OpenResult, String> { panic!("must not open") };
        assert_eq!(s.open_verified(never), Err(DownloadError::NotDownloaded));

        let f = dir.0.join(NAME);
        fs::write(&f, b"12345").unwrap();
        s.set_verified(f.clone(), 5);
        let r = s.open_verified(|p| {
            Ok(OpenResult::Debug {
                would_run: vec!["/usr/bin/open".into(), p.display().to_string()],
            })
        });
        assert_eq!(
            r,
            Ok(OpenResult::Debug {
                would_run: vec!["/usr/bin/open".into(), f.display().to_string()]
            })
        );

        // Changed size → refused and forgotten.
        fs::write(&f, b"123456").unwrap();
        assert_eq!(s.open_verified(never), Err(DownloadError::NotDownloaded));
        fs::write(&f, b"12345").unwrap();
        assert_eq!(s.open_verified(never), Err(DownloadError::NotDownloaded));

        // Missing file.
        s.set_verified(dir.0.join("gone.pkg"), 5);
        assert_eq!(s.open_verified(never), Err(DownloadError::NotDownloaded));

        // A folder in its place.
        let d = dir.0.join("dir.pkg");
        fs::create_dir(&d).unwrap();
        s.set_verified(d, 0);
        assert_eq!(s.open_verified(never), Err(DownloadError::NotDownloaded));

        // Opener failure.
        s.set_verified(f, 5);
        assert_eq!(
            s.open_verified(|_| Err("Installer couldn't be opened.".into())),
            Err(DownloadError::OpenFailed(
                "Installer couldn't be opened.".into()
            ))
        );
    }

    #[test]
    fn begin_forgets_verified_file() {
        let dir = TempDir::new();
        let f = dir.0.join(NAME);
        fs::write(&f, b"1").unwrap();
        let s = DownloadState::default();
        s.set_verified(f, 1);
        drop(s.begin().unwrap());
        assert_eq!(
            s.open_verified(|_| panic!("must not open")),
            Err(DownloadError::NotDownloaded)
        );
    }

    // --- JSON ---

    #[test]
    fn error_json_shapes() {
        let cases: Vec<(DownloadError, &str)> = vec![
            (DownloadError::Offline, "offline"),
            (DownloadError::Stalled, "offline"),
            (DownloadError::NoUpdate, "no_update"),
            (DownloadError::NoPkg, "no_pkg"),
            (DownloadError::UrlRefused("not https"), "url_refused"),
            (DownloadError::TooLarge(MAX_PKG_BYTES + 1), "too_large"),
            (
                DownloadError::SizeMismatch {
                    expected: 2,
                    got: 1,
                },
                "size_mismatch",
            ),
            (DownloadError::ChecksumMismatch, "checksum_mismatch"),
            (
                DownloadError::ChecksumUnreadable("x".into()),
                "checksum_unreadable",
            ),
            (DownloadError::Cancelled, "cancelled"),
            (DownloadError::Busy, "busy"),
            (DownloadError::NotDownloaded, "not_downloaded"),
            (DownloadError::OpenFailed("nope".into()), "open_failed"),
        ];
        for (e, kind) in cases {
            let v = serde_json::to_value(&e).unwrap();
            assert_eq!(v["kind"], kind);
            assert_eq!(v["message"], e.message());
            assert!(!e.message().is_empty());
            assert_eq!(v.as_object().unwrap().len(), 2, "{:?}", e);
        }

        let e = DownloadError::Disk {
            path: "/Users/x/Downloads/a.pkg.part".into(),
            detail: "No space left on device".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "disk");
        assert_eq!(v["path"], "/Users/x/Downloads/a.pkg.part");
        assert_eq!(
            v["message"],
            "Couldn't save the download (/Users/x/Downloads/a.pkg.part): No space left on device"
        );

        let v = serde_json::to_value(DownloadError::Http(502)).unwrap();
        assert_eq!(v["kind"], "http");
        assert_eq!(v["status"], 502);

        // Check errors pass through unchanged.
        let v = serde_json::to_value(DownloadError::Check(UpdateError::RateLimited {
            reset_at: None,
        }))
        .unwrap();
        assert_eq!(v["kind"], "rate_limited");
        assert!(v["reset_at"].is_null());

        assert_eq!(
            DownloadError::TooLarge(MAX_PKG_BYTES + 1).message(),
            "The installer is too large (201 MB; the limit is 200 MB)."
        );
    }

    #[test]
    fn result_json_shapes() {
        let v = serde_json::to_value(DownloadResult {
            path: "/d/a.pkg".into(),
            size: 3,
            checksum: "not_published",
            version: "1.14.0".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"path":"/d/a.pkg","size":3,"checksum":"not_published","version":"1.14.0"})
        );
        let v = serde_json::to_value(OpenResult::Debug {
            would_run: vec!["/usr/bin/open".into(), "/d/a.pkg".into()],
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"debug","would_run":["/usr/bin/open","/d/a.pkg"]})
        );
        let v = serde_json::to_value(OpenResult::Opened {
            path: "/d/a.pkg".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"kind":"opened","path":"/d/a.pkg"}));
        let v = serde_json::to_value(Progress {
            received: 1,
            total: 2,
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"received":1,"total":2}));
    }
}
