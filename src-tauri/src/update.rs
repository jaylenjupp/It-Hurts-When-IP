// src-tauri/src/update.rs
//
// Checks GitHub Releases for a newer version of the app.
// Returns an UpdateInfo struct describing what was found.
//
// This module is intentionally platform-agnostic — the only platform-aware
// bit is which installer asset to pick (.pkg vs .exe).

use serde::{Deserialize, Serialize};

const GITHUB_OWNER: &str = "jaylenjupp";
const GITHUB_REPO: &str = "it-hurts-when-ip";
const REQUEST_TIMEOUT_SECS: u64 = 10;

// The version baked in at compile time, from Cargo.toml.
// This is our single source of truth for "what version am I?"
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// GitHub's API requires a User-Agent header on every request.
const USER_AGENT: &str = concat!("ItHurtsWhenIP/", env!("CARGO_PKG_VERSION"));

/// What we hand back to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    /// GitHub release page — used by the Changelog button.
    pub release_url: String,
    /// Direct download URL for the platform-appropriate installer asset
    /// (used by the Download button). None if no matching asset found.
    pub download_url: Option<String>,
}

/// Minimal subset of GitHub's /releases/latest response.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Picks the first asset whose filename ends with the current platform's
/// installer extension. macOS → .pkg, Windows → .exe.
fn pick_asset(assets: &[GithubAsset]) -> Option<String> {
    let ext = if cfg!(target_os = "macos") {
        ".pkg"
    } else if cfg!(target_os = "windows") {
        ".exe"
    } else {
        return None;
    };

    assets
        .iter()
        .find(|a| a.name.to_lowercase().ends_with(ext))
        .map(|a| a.browser_download_url.clone())
}

/// Fetch the latest release from GitHub and compare versions.
/// Errors are returned as plain strings — the caller decides whether to
/// surface them or silently swallow (for the launch-time check we'll
/// silently swallow).
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        GITHUB_OWNER, GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("failed to build http client: {}", e))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("network request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("github returned status {}", resp.status()));
    }

    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse github response: {}", e))?;

    // GitHub tags are conventionally prefixed with "v" (e.g. "v1.13.0").
    // semver doesn't accept the prefix, so strip it before parsing.
    let latest_str = release.tag_name.trim_start_matches('v').to_string();

    let current = semver::Version::parse(CURRENT_VERSION)
        .map_err(|e| format!("invalid current version '{}': {}", CURRENT_VERSION, e))?;
    let latest = semver::Version::parse(&latest_str)
        .map_err(|e| format!("invalid latest version '{}': {}", latest_str, e))?;

    Ok(UpdateInfo {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest_str,
        update_available: latest > current,
        release_url: release.html_url,
        download_url: pick_asset(&release.assets),
    })
}
