// Build stamp: which commit this binary was built from, and when.
//
// The values are baked in at compile time by `build.rs` (IHWIP_GIT_SHA,
// IHWIP_GIT_DIRTY, IHWIP_BUILD_UTC) so Jay can tell draft builds apart.

use serde::Serialize;

// Compiled here only for its tests; build.rs uses it to stamp IHWIP_BUILD_UTC.
#[cfg(test)]
mod utc;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// App version from Cargo.toml.
    pub version: String,
    /// Short (7-char) git SHA, or `unknown` if git wasn't available at build time.
    pub git_sha: String,
    /// True if the working tree had changes at build time, or if that couldn't
    /// be checked.
    pub dirty: bool,
    /// Build time, `YYYY-MM-DDTHH:MM:SSZ`.
    pub built_utc: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("IHWIP_GIT_SHA").to_string(),
        dirty: parse_dirty(env!("IHWIP_GIT_DIRTY")),
        built_utc: env!("IHWIP_BUILD_UTC").to_string(),
    }
}

/// Only an explicit `false` counts as clean. `unknown` (git failed) is treated
/// as dirty so an unverifiable build is never presented as clean.
fn parse_dirty(raw: &str) -> bool {
    raw != "false"
}

/// Human-readable stamp, e.g. `1.13.1 (abc1234, 2026-11-15T10:20:30Z)` or
/// `1.13.1 (abc1234-dirty, 2026-11-15T10:20:30Z)`.
#[allow(dead_code)] // for Settings → Updates and diagnostics (later phases)
pub fn stamp_label(info: &BuildInfo) -> String {
    let dirty = if info.dirty { "-dirty" } else { "" };
    format!(
        "{} ({}{}, {})",
        info.version, info.git_sha, dirty, info.built_utc
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_utc_stamp(s: &str) -> bool {
        // YYYY-MM-DDTHH:MM:SSZ
        let b = s.as_bytes();
        b.len() == 20
            && b.iter().enumerate().all(|(i, &c)| match i {
                4 | 7 => c == b'-',
                10 => c == b'T',
                13 | 16 => c == b':',
                19 => c == b'Z',
                _ => c.is_ascii_digit(),
            })
    }

    #[test]
    fn build_info_has_expected_shape() {
        let info = build_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(
            info.git_sha == "unknown"
                || (info.git_sha.len() == 7
                    && info
                        .git_sha
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))),
            "unexpected git_sha: {:?}",
            info.git_sha
        );
        assert!(
            is_utc_stamp(&info.built_utc),
            "unexpected built_utc: {:?}",
            info.built_utc
        );
    }

    #[test]
    fn utc_shape_check_rejects_bad_values() {
        assert!(is_utc_stamp("2026-11-15T10:20:30Z"));
        assert!(!is_utc_stamp("2026-11-15 10:20:30Z"));
        assert!(!is_utc_stamp("2026-11-15T10:20:30"));
        assert!(!is_utc_stamp("unknown"));
    }

    #[test]
    fn dirty_unknown_counts_as_dirty() {
        assert!(!parse_dirty("false"));
        assert!(parse_dirty("true"));
        assert!(parse_dirty("unknown"));
        assert!(parse_dirty(""));
    }

    fn sample(dirty: bool) -> BuildInfo {
        BuildInfo {
            version: "1.13.1".to_string(),
            git_sha: "abc1234".to_string(),
            dirty,
            built_utc: "2026-11-15T10:20:30Z".to_string(),
        }
    }

    #[test]
    fn stamp_label_clean() {
        assert_eq!(
            stamp_label(&sample(false)),
            "1.13.1 (abc1234, 2026-11-15T10:20:30Z)"
        );
    }

    #[test]
    fn stamp_label_dirty() {
        assert_eq!(
            stamp_label(&sample(true)),
            "1.13.1 (abc1234-dirty, 2026-11-15T10:20:30Z)"
        );
    }
}
