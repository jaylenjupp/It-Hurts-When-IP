use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// Shared with the app crate, where it is unit-tested (see src/build_info.rs).
#[path = "src/build_info/utc.rs"]
mod utc;

fn main() {
    stamp_build();
    tauri_build::build()
}

/// Bakes the build stamp into the app; src/build_info.rs reads it with env!.
fn stamp_build() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let repo_root = manifest_dir.parent().unwrap_or(&manifest_dir);

    let sha = git(repo_root, &["rev-parse", "--short=7", "HEAD"])
        .and_then(|out| short_sha(&out))
        .unwrap_or_else(|| "unknown".to_string());

    // --no-optional-locks stops `git status` from refreshing .git/index, which
    // would bump its mtime and make the rerun-if-changed below fire every build.
    let dirty = match git(repo_root, &["--no-optional-locks", "status", "--porcelain"]) {
        Some(out) if out.trim().is_empty() => "false",
        Some(_) => "true",
        None => "unknown",
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    println!("cargo:rustc-env=IHWIP_GIT_SHA={}", sha);
    println!("cargo:rustc-env=IHWIP_GIT_DIRTY={}", dirty);
    println!("cargo:rustc-env=IHWIP_BUILD_UTC={}", utc::format_utc(now));

    watch_git(&repo_root.join(".git"));

    // Unstaged edits to app or frontend sources also refresh the dirty flag.
    for dir in [manifest_dir.join("src"), repo_root.join("src")] {
        if dir.is_dir() {
            println!("cargo:rerun-if-changed={}", dir.display());
        }
    }
    // Value unused; scripts/build-macos.sh sets a fresh one per run to force a new stamp.
    println!("cargo:rerun-if-env-changed=IHWIP_BUILD_NONCE");
}

/// Runs git with an argv array (no shell) in `dir`. None if git is missing or fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// First 7 chars of `git rev-parse --short=7` output, if it looks like a SHA.
/// (git prints more than 7 chars when 7 would be ambiguous.)
fn short_sha(out: &str) -> Option<String> {
    let out = out.trim();
    let is_sha = out.len() >= 7
        && out
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    is_sha.then(|| out[..7].to_string())
}

/// Re-run this script when HEAD moves (commit, checkout) or the index changes
/// (staging), so the stamp stays current without re-running on every build.
fn watch_git(git_dir: &Path) {
    let head = git_dir.join("HEAD");
    let mut paths = vec![head.clone(), git_dir.join("index")];
    // The branch ref HEAD points to, e.g. "ref: refs/heads/dev".
    if let Ok(contents) = fs::read_to_string(&head) {
        if let Some(reference) = contents.trim().strip_prefix("ref: ") {
            paths.push(git_dir.join(reference));
        }
    }
    // Only watch files that exist: cargo re-runs the script on every build if a
    // rerun-if-changed path is missing (e.g. no .git in a source tarball, or a
    // packed ref).
    for path in paths.iter().filter(|p| p.is_file()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
