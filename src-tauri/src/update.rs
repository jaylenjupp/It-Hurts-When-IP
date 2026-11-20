// src-tauri/src/update.rs
//
// Checks GitHub Releases for a newer version of the app.
//
// Two layers:
// - `fetch_latest`: the only network call. GETs the repo's published
//   `/releases/latest` (unauthenticated, so drafts are never seen) and hands
//   back status, the rate-limit headers and the body text.
// - `evaluate`: pure. Turns that response into an `UpdateInfo` or an
//   `UpdateError`. Fully unit-tested against fixtures in `testdata/`.
//
// Platform-agnostic apart from which installer the Download button uses
// (.pkg on macOS; .exe on Windows, which is parked).

use std::cmp::Ordering;

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::build_info::utc::format_utc;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/jaylenjupp/it-hurts-when-ip/releases/latest";
const REQUEST_TIMEOUT_SECS: u64 = 10;

// The version baked in at compile time, from Cargo.toml.
// This is our single source of truth for "what version am I?"
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// GitHub's API requires a User-Agent header on every request.
const USER_AGENT: &str = concat!("ItHurtsWhenIP/", env!("CARGO_PKG_VERSION"));

/// A release asset, as shown to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    /// Bytes, as reported by GitHub.
    pub size: u64,
    pub url: String,
}

/// What we hand back to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    /// True only when the latest published release is a newer, non-pre-release
    /// semver version than this build.
    pub update_available: bool,
    /// GitHub release page — used by the Changelog button.
    pub release_url: String,
    /// Direct download URL for the platform-appropriate installer asset
    /// (used by the launch popup's Download button). None if no matching
    /// asset found.
    pub download_url: Option<String>,
    /// When GitHub published the release (ISO 8601), if given.
    pub published_at: Option<String>,
    /// The macOS installer package, if one could be picked unambiguously.
    pub pkg: Option<AssetInfo>,
    /// Download URL of `<pkg name>.sha256`, if published.
    pub sha256_url: Option<String>,
    /// When this check happened (seconds since the Unix epoch).
    pub checked_at: i64,
}

/// Why an update check failed. Serialised to the frontend as
/// `{ "kind": ..., "message": ..., [extra fields] }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Couldn't reach GitHub (no connection, DNS, timeout).
    Offline,
    /// GitHub's unauthenticated rate limit was hit. `reset_at` = when it
    /// lifts (seconds since the Unix epoch), if GitHub said.
    RateLimited { reset_at: Option<i64> },
    /// 404: the repo has no published release.
    NoReleases,
    /// Any other non-success HTTP status.
    Http(u16),
    /// The reply wasn't the JSON we expected.
    BadResponse,
    /// The latest release's tag isn't a version we understand.
    UnrecognisedTag(String),
}

impl UpdateError {
    pub fn kind(&self) -> &'static str {
        match self {
            UpdateError::Offline => "offline",
            UpdateError::RateLimited { .. } => "rate_limited",
            UpdateError::NoReleases => "no_releases",
            UpdateError::Http(_) => "http",
            UpdateError::BadResponse => "bad_response",
            UpdateError::UnrecognisedTag(_) => "unrecognised_tag",
        }
    }

    pub fn message(&self) -> String {
        match self {
            UpdateError::Offline => {
                "Can't reach GitHub. Check your internet connection.".to_string()
            }
            UpdateError::RateLimited {
                reset_at: Some(t), ..
            } if *t >= 0 => format!(
                "GitHub is limiting update checks. Try again after {} (UTC).",
                format_utc(*t as u64)
            ),
            UpdateError::RateLimited { .. } => {
                "GitHub is limiting update checks. Try again later.".to_string()
            }
            UpdateError::NoReleases => "No published release found.".to_string(),
            UpdateError::Http(status) => {
                format!("GitHub returned an error (HTTP {}).", status)
            }
            UpdateError::BadResponse => "Couldn't read GitHub's reply.".to_string(),
            UpdateError::UnrecognisedTag(tag) => {
                format!("Couldn't read the latest version (tag \"{}\").", tag)
            }
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl Serialize for UpdateError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("kind", self.kind())?;
        map.serialize_entry("message", &self.message())?;
        match self {
            UpdateError::RateLimited { reset_at } => map.serialize_entry("reset_at", reset_at)?,
            UpdateError::Http(status) => map.serialize_entry("status", status)?,
            UpdateError::UnrecognisedTag(tag) => map.serialize_entry("tag", tag)?,
            _ => {}
        }
        map.end()
    }
}

/// What the HTTP layer hands to `evaluate`.
#[derive(Debug, Clone, Default)]
pub struct RawResponse {
    pub status: u16,
    pub ratelimit_remaining: Option<String>,
    pub ratelimit_reset: Option<String>,
    pub retry_after: Option<String>,
    pub body: String,
}

/// Minimal subset of GitHub's /releases/latest response.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    #[serde(default)]
    size: u64,
    browser_download_url: String,
}

impl GithubAsset {
    fn info(&self) -> AssetInfo {
        AssetInfo {
            name: self.name.clone(),
            size: self.size,
            url: self.browser_download_url.clone(),
        }
    }
}

/// Parses a release tag: `X.Y.Z` or `vX.Y.Z` (exactly one leading `v`).
fn parse_tag(tag: &str) -> Option<semver::Version> {
    let s = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(s).ok()
}

/// The macOS installer: the asset named exactly `ItHurtsWhenIP-<latest>.pkg`,
/// else the only `.pkg` asset. Several `.pkg` and none exact → None.
fn select_pkg<'a>(assets: &'a [GithubAsset], latest: &str) -> Option<&'a GithubAsset> {
    let exact = format!("ItHurtsWhenIP-{}.pkg", latest);
    if let Some(a) = assets.iter().find(|a| a.name == exact) {
        return Some(a);
    }
    let mut pkgs = assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".pkg"));
    match (pkgs.next(), pkgs.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// The `<pkg name>.sha256` asset's download URL, if published.
fn find_sha256(assets: &[GithubAsset], pkg_name: &str) -> Option<String> {
    let want = format!("{}.sha256", pkg_name);
    assets
        .iter()
        .find(|a| a.name == want)
        .map(|a| a.browser_download_url.clone())
}

/// The Download-button URL for this platform. macOS → the selected pkg.
/// Windows (parked) → first `.exe`, as before.
fn installer_url(assets: &[GithubAsset], pkg: Option<&AssetInfo>) -> Option<String> {
    if cfg!(target_os = "macos") {
        pkg.map(|p| p.url.clone())
    } else if cfg!(target_os = "windows") {
        assets
            .iter()
            .find(|a| a.name.to_lowercase().ends_with(".exe"))
            .map(|a| a.browser_download_url.clone())
    } else {
        None
    }
}

fn parse_i64(v: &Option<String>) -> Option<i64> {
    v.as_deref().and_then(|s| s.trim().parse::<i64>().ok())
}

/// When a rate limit lifts: `retry-after` (seconds from now) wins, then
/// `x-ratelimit-reset` (epoch seconds).
fn reset_at(resp: &RawResponse, now: i64) -> Option<i64> {
    parse_i64(&resp.retry_after)
        .filter(|s| *s >= 0)
        .map(|s| now.saturating_add(s))
        .or_else(|| parse_i64(&resp.ratelimit_reset))
}

/// Maps a non-success status to an error.
fn map_status(resp: &RawResponse, now: i64) -> UpdateError {
    let remaining_zero = parse_i64(&resp.ratelimit_remaining) == Some(0);
    match resp.status {
        404 => UpdateError::NoReleases,
        429 => UpdateError::RateLimited {
            reset_at: reset_at(resp, now),
        },
        403 if remaining_zero => UpdateError::RateLimited {
            reset_at: reset_at(resp, now),
        },
        s => UpdateError::Http(s),
    }
}

/// Pure: turns GitHub's reply into what the UI shows. `now` = seconds since
/// the Unix epoch (for `checked_at` and rate-limit reset times).
pub fn evaluate(current: &str, resp: &RawResponse, now: i64) -> Result<UpdateInfo, UpdateError> {
    if !(200..300).contains(&resp.status) {
        return Err(map_status(resp, now));
    }
    let release: GithubRelease =
        serde_json::from_str(&resp.body).map_err(|_| UpdateError::BadResponse)?;

    let latest = parse_tag(&release.tag_name)
        .ok_or_else(|| UpdateError::UnrecognisedTag(release.tag_name.clone()))?;
    // Compiled-in; can't fail in practice.
    let current_v = semver::Version::parse(current).map_err(|_| UpdateError::BadResponse)?;

    // Pre-releases are never offered, whatever GitHub calls "latest".
    let is_prerelease = release.prerelease || !latest.pre.is_empty();
    // Precedence ignores build metadata (`Ord` on Version doesn't).
    let newer = latest.cmp_precedence(&current_v) == Ordering::Greater;

    let latest_str = latest.to_string();
    let pkg_asset = select_pkg(&release.assets, &latest_str);
    let pkg = pkg_asset.map(GithubAsset::info);
    let sha256_url = pkg_asset.and_then(|p| find_sha256(&release.assets, &p.name));
    let download_url = installer_url(&release.assets, pkg.as_ref());

    Ok(UpdateInfo {
        current_version: current.to_string(),
        latest_version: latest_str,
        update_available: newer && !is_prerelease,
        release_url: release.html_url,
        download_url,
        published_at: release.published_at,
        pkg,
        sha256_url,
        checked_at: now,
    })
}

/// The HTTP layer: one unauthenticated GET of the published latest release.
async fn fetch_latest() -> Result<RawResponse, UpdateError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| {
            eprintln!("[update] failed to build http client: {}", e);
            UpdateError::Offline
        })?;

    let resp = client
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            eprintln!("[update] request failed: {}", e);
            UpdateError::Offline
        })?;

    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let status = resp.status().as_u16();
    let ratelimit_remaining = header("x-ratelimit-remaining");
    let ratelimit_reset = header("x-ratelimit-reset");
    let retry_after = header("retry-after");

    let body = resp.text().await.map_err(|e| {
        eprintln!("[update] reading response failed: {}", e);
        UpdateError::Offline
    })?;

    Ok(RawResponse {
        status,
        ratelimit_remaining,
        ratelimit_reset,
        retry_after,
        body,
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Fetch the latest published release and compare it with this build.
pub async fn check_for_update() -> Result<UpdateInfo, UpdateError> {
    let resp = fetch_latest().await?;
    evaluate(CURRENT_VERSION, &resp, now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str = include_str!("testdata/gh_release_normal.json");
    const WITH_SHA256: &str = include_str!("testdata/gh_release_with_sha256.json");
    const TWO_PKGS: &str = include_str!("testdata/gh_release_two_pkgs.json");
    const NO_PKG: &str = include_str!("testdata/gh_release_no_pkg.json");
    const DEV_TAG: &str = include_str!("testdata/gh_release_dev_tag.json");
    const PRERELEASE: &str = include_str!("testdata/gh_release_prerelease.json");
    const MALFORMED: &str = include_str!("testdata/gh_release_malformed.json");
    const V1_13_1: &str = include_str!("testdata/gh_release_v1_13_1.json");

    // 2026-11-20T12:00:00Z
    const NOW: i64 = 1_795_176_000;
    const DL: &str = "https://github.com/jaylenjupp/it-hurts-when-ip/releases/download";

    fn ok(body: &str) -> RawResponse {
        RawResponse {
            status: 200,
            body: body.to_string(),
            ..RawResponse::default()
        }
    }

    fn status(code: u16) -> RawResponse {
        RawResponse {
            status: code,
            body: r#"{"message":"x"}"#.to_string(),
            ..RawResponse::default()
        }
    }

    fn release_with_tag(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{}","html_url":"https://github.com/jaylenjupp/it-hurts-when-ip/releases/tag/{}","assets":[]}}"#,
            tag, tag
        )
    }

    fn assets(names: &[&str]) -> Vec<GithubAsset> {
        names
            .iter()
            .map(|n| GithubAsset {
                name: n.to_string(),
                size: 1,
                browser_download_url: format!("{}/v1.14.0/{}", DL, n),
            })
            .collect()
    }

    // --- version comparison ---

    #[test]
    fn version_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("1.13.1", "1.14.0", true),
            ("1.13.1", "1.13.1", false),
            ("1.13.1", "1.13.0", false),
            ("1.13.1", "v1.14.0", true),
            ("1.13.1", "v1.13.1", false),
            ("1.13.1", "1.14.0-rc.1", false),
            ("1.13.1", "v1.14.0-rc.1", false),
            ("1.13.1+abc1234", "1.13.1", false),
            ("1.13.1+abc1234", "v1.14.0", true),
            ("1.13.1", "1.13.1+zzz", false),
        ];
        for (current, tag, expect) in cases {
            let info = evaluate(current, &ok(&release_with_tag(tag)), NOW)
                .unwrap_or_else(|e| panic!("{} vs {}: {:?}", current, tag, e));
            assert_eq!(info.update_available, *expect, "{} vs {}", current, tag);
            assert_eq!(info.current_version, *current);
        }
    }

    #[test]
    fn v_prefix_is_stripped_once() {
        let info = evaluate("1.13.1", &ok(&release_with_tag("v1.14.0")), NOW).unwrap();
        assert_eq!(info.latest_version, "1.14.0");
    }

    #[test]
    fn unrecognised_tags_are_errors() {
        for tag in [
            "dev-v1.13.1-20261118-3bd73ce",
            "untagged-0a1b2c3d4e",
            "latest",
            "garbage",
            "vv1.14.0",
            "1.14",
            "",
        ] {
            assert_eq!(
                evaluate("1.13.1", &ok(&release_with_tag(tag)), NOW),
                Err(UpdateError::UnrecognisedTag(tag.to_string())),
                "{}",
                tag
            );
        }
    }

    // --- asset selection ---

    #[test]
    fn pkg_exact_name_wins() {
        let a = assets(&[
            "ItHurtsWhenIP-arm64.pkg",
            "ItHurtsWhenIP-1.14.0.pkg",
            "ItHurtsWhenIP-x86_64.pkg",
        ]);
        assert_eq!(
            select_pkg(&a, "1.14.0").unwrap().name,
            "ItHurtsWhenIP-1.14.0.pkg"
        );
    }

    #[test]
    fn single_pkg_with_other_name_is_used() {
        let a = assets(&["Installer.pkg", "ItHurtsWhenIP-Setup-1.14.0.exe"]);
        assert_eq!(select_pkg(&a, "1.14.0").unwrap().name, "Installer.pkg");
    }

    #[test]
    fn two_pkgs_without_exact_match_is_none() {
        let a = assets(&["ItHurtsWhenIP-arm64.pkg", "ItHurtsWhenIP-x86_64.pkg"]);
        assert!(select_pkg(&a, "1.14.0").is_none());
    }

    #[test]
    fn exe_and_sha256_are_never_the_pkg() {
        let a = assets(&[
            "ItHurtsWhenIP-Setup-1.14.0.exe",
            "ItHurtsWhenIP-1.14.0.pkg.sha256",
        ]);
        assert!(select_pkg(&a, "1.14.0").is_none());
        assert!(select_pkg(&[], "1.14.0").is_none());
    }

    #[test]
    fn sha256_found_and_absent() {
        let a = assets(&[
            "ItHurtsWhenIP-1.14.0.pkg",
            "ItHurtsWhenIP-1.14.0.pkg.sha256",
        ]);
        assert_eq!(
            find_sha256(&a, "ItHurtsWhenIP-1.14.0.pkg").as_deref(),
            Some(format!("{}/v1.14.0/ItHurtsWhenIP-1.14.0.pkg.sha256", DL).as_str())
        );
        let b = assets(&["ItHurtsWhenIP-1.14.0.pkg", "other.sha256"]);
        assert_eq!(find_sha256(&b, "ItHurtsWhenIP-1.14.0.pkg"), None);
    }

    // --- fixtures through evaluate ---

    #[test]
    fn fixture_normal() {
        let info = evaluate("1.13.1", &ok(NORMAL), NOW).unwrap();
        assert!(info.update_available);
        assert_eq!(info.latest_version, "1.14.0");
        assert_eq!(
            info.release_url,
            "https://github.com/jaylenjupp/it-hurts-when-ip/releases/tag/v1.14.0"
        );
        assert_eq!(info.published_at.as_deref(), Some("2026-12-01T09:15:00Z"));
        let pkg = info.pkg.clone().unwrap();
        assert_eq!(pkg.name, "ItHurtsWhenIP-1.14.0.pkg");
        assert_eq!(pkg.size, 8_123_456);
        assert_eq!(pkg.url, format!("{}/v1.14.0/ItHurtsWhenIP-1.14.0.pkg", DL));
        assert_eq!(info.sha256_url, None);
        assert_eq!(info.checked_at, NOW);
        #[cfg(target_os = "macos")]
        assert_eq!(info.download_url, Some(pkg.url));
    }

    #[test]
    fn fixture_with_sha256() {
        let info = evaluate("1.13.1", &ok(WITH_SHA256), NOW).unwrap();
        assert_eq!(info.pkg.unwrap().name, "ItHurtsWhenIP-1.14.0.pkg");
        assert_eq!(
            info.sha256_url,
            Some(format!("{}/v1.14.0/ItHurtsWhenIP-1.14.0.pkg.sha256", DL))
        );
    }

    #[test]
    fn fixture_two_pkgs() {
        let info = evaluate("1.13.1", &ok(TWO_PKGS), NOW).unwrap();
        assert!(info.update_available);
        assert_eq!(info.pkg, None);
        assert_eq!(info.sha256_url, None);
        #[cfg(target_os = "macos")]
        assert_eq!(info.download_url, None);
    }

    #[test]
    fn fixture_no_pkg() {
        let info = evaluate("1.13.1", &ok(NO_PKG), NOW).unwrap();
        assert!(info.update_available);
        assert_eq!(info.pkg, None);
        #[cfg(target_os = "macos")]
        assert_eq!(info.download_url, None);
    }

    #[test]
    fn fixture_dev_tag() {
        assert_eq!(
            evaluate("1.13.1", &ok(DEV_TAG), NOW),
            Err(UpdateError::UnrecognisedTag(
                "dev-v1.13.1-20261118-3bd73ce".to_string()
            ))
        );
    }

    #[test]
    fn fixture_prerelease_is_never_offered() {
        let info = evaluate("1.13.1", &ok(PRERELEASE), NOW).unwrap();
        assert!(!info.update_available);
        assert_eq!(info.latest_version, "1.14.0-rc.1");
        assert_eq!(info.pkg.unwrap().name, "ItHurtsWhenIP-1.14.0-rc.1.pkg");
    }

    #[test]
    fn prerelease_flag_alone_blocks_offer() {
        let body = NORMAL.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_ne!(body, NORMAL);
        assert!(
            !evaluate("1.13.1", &ok(&body), NOW)
                .unwrap()
                .update_available
        );
    }

    #[test]
    fn fixture_malformed_json() {
        assert_eq!(
            evaluate("1.13.1", &ok(MALFORMED), NOW),
            Err(UpdateError::BadResponse)
        );
        assert_eq!(
            evaluate("1.13.1", &ok(r#"{"html_url":"x","assets":[]}"#), NOW),
            Err(UpdateError::BadResponse)
        );
    }

    /// The real published release as of 2026-11: v1.13.1 with a pkg and a
    /// (parked) Windows exe, no checksum.
    #[test]
    fn fixture_published_v1_13_1() {
        let info = evaluate("1.13.1", &ok(V1_13_1), NOW).unwrap();
        assert!(!info.update_available);
        assert_eq!(info.latest_version, "1.13.1");
        let pkg = info.pkg.clone().unwrap();
        assert_eq!(pkg.name, "ItHurtsWhenIP-1.13.1.pkg");
        assert_eq!(pkg.url, format!("{}/v1.13.1/ItHurtsWhenIP-1.13.1.pkg", DL));
        assert_eq!(info.sha256_url, None);
        #[cfg(target_os = "macos")]
        assert_eq!(info.download_url, Some(pkg.url));
    }

    // --- error mapping ---

    #[test]
    fn status_404_is_no_releases() {
        assert_eq!(
            evaluate("1.13.1", &status(404), NOW),
            Err(UpdateError::NoReleases)
        );
    }

    #[test]
    fn status_403_rate_limited_with_reset() {
        let r = RawResponse {
            ratelimit_remaining: Some("0".into()),
            ratelimit_reset: Some("1795179600".into()),
            ..status(403)
        };
        assert_eq!(
            evaluate("1.13.1", &r, NOW),
            Err(UpdateError::RateLimited {
                reset_at: Some(1_795_179_600)
            })
        );
    }

    #[test]
    fn status_403_retry_after_wins_over_reset() {
        let r = RawResponse {
            ratelimit_remaining: Some(" 0 ".into()),
            ratelimit_reset: Some("1795179600".into()),
            retry_after: Some("60".into()),
            ..status(403)
        };
        assert_eq!(
            evaluate("1.13.1", &r, NOW),
            Err(UpdateError::RateLimited {
                reset_at: Some(NOW + 60)
            })
        );
    }

    #[test]
    fn status_403_without_rate_limit_is_http() {
        assert_eq!(
            evaluate("1.13.1", &status(403), NOW),
            Err(UpdateError::Http(403))
        );
        let r = RawResponse {
            ratelimit_remaining: Some("12".into()),
            ..status(403)
        };
        assert_eq!(evaluate("1.13.1", &r, NOW), Err(UpdateError::Http(403)));
    }

    #[test]
    fn status_429_with_retry_after() {
        let r = RawResponse {
            retry_after: Some("120".into()),
            ..status(429)
        };
        assert_eq!(
            evaluate("1.13.1", &r, NOW),
            Err(UpdateError::RateLimited {
                reset_at: Some(NOW + 120)
            })
        );
    }

    #[test]
    fn status_429_without_headers_or_bad_headers() {
        assert_eq!(
            evaluate("1.13.1", &status(429), NOW),
            Err(UpdateError::RateLimited { reset_at: None })
        );
        let r = RawResponse {
            retry_after: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
            ratelimit_reset: Some("soon".into()),
            ..status(429)
        };
        assert_eq!(
            evaluate("1.13.1", &r, NOW),
            Err(UpdateError::RateLimited { reset_at: None })
        );
    }

    #[test]
    fn status_500_is_http() {
        assert_eq!(
            evaluate("1.13.1", &status(500), NOW),
            Err(UpdateError::Http(500))
        );
    }

    // --- serialisation ---

    #[test]
    fn error_json_shape() {
        let v = serde_json::to_value(UpdateError::RateLimited {
            reset_at: Some(1_795_179_600),
        })
        .unwrap();
        assert_eq!(v["kind"], "rate_limited");
        assert_eq!(v["reset_at"], 1_795_179_600i64);
        assert_eq!(
            v["message"],
            "GitHub is limiting update checks. Try again after 2026-11-20T13:00:00Z (UTC)."
        );

        let v = serde_json::to_value(UpdateError::RateLimited { reset_at: None }).unwrap();
        assert!(v["reset_at"].is_null());
        assert_eq!(
            v["message"],
            "GitHub is limiting update checks. Try again later."
        );

        let v = serde_json::to_value(UpdateError::Http(500)).unwrap();
        assert_eq!(v["kind"], "http");
        assert_eq!(v["status"], 500);

        let v = serde_json::to_value(UpdateError::UnrecognisedTag("latest".into())).unwrap();
        assert_eq!(v["kind"], "unrecognised_tag");
        assert_eq!(v["tag"], "latest");
        assert_eq!(
            v["message"],
            "Couldn't read the latest version (tag \"latest\")."
        );

        for (e, kind) in [
            (UpdateError::Offline, "offline"),
            (UpdateError::NoReleases, "no_releases"),
            (UpdateError::BadResponse, "bad_response"),
        ] {
            let v = serde_json::to_value(&e).unwrap();
            assert_eq!(v["kind"], kind);
            assert_eq!(v["message"], e.message());
            assert_eq!(v.as_object().unwrap().len(), 2);
        }
    }

    #[test]
    fn update_info_keeps_launch_popup_fields() {
        let v = serde_json::to_value(evaluate("1.13.1", &ok(NORMAL), NOW).unwrap()).unwrap();
        for key in [
            "current_version",
            "latest_version",
            "update_available",
            "release_url",
            "download_url",
            "published_at",
            "pkg",
            "sha256_url",
            "checked_at",
        ] {
            assert!(v.get(key).is_some(), "missing {}", key);
        }
    }
}
