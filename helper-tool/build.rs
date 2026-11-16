// Build script: records the short git SHA of the build as IHWIP_GIT_SHA so
// `ping` can report it. Runs git as an argv (no shell) and never fails the
// build: without git, or outside a repo, the SHA is "unknown".

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let git_dir = manifest_dir.join("../.git");
    // Re-run when HEAD moves: HEAD itself (branch switch) and the branch ref
    // it points at (new commit). Only existing files are watched, because a
    // missing path would make Cargo re-run this script on every build.
    let head = git_dir.join("HEAD");
    if head.is_file() {
        println!("cargo:rerun-if-changed={}", head.display());
        if let Ok(text) = std::fs::read_to_string(&head) {
            if let Some(reference) = text.trim().strip_prefix("ref: ") {
                let ref_file = git_dir.join(reference);
                if ref_file.is_file() {
                    println!("cargo:rerun-if-changed={}", ref_file.display());
                }
            }
        }
        let packed = git_dir.join("packed-refs");
        if packed.is_file() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=IHWIP_GIT_SHA={}", sha);
}
