// Update download helpers that run system tools (macOS only):
//  - SHA-256 of a downloaded pkg via `/usr/bin/shasum -a 256 <file>`.
//  - Handing a verified pkg to Installer via `/usr/bin/open <file>`.
//
// Both run through a CommandRunner with argv only (no shell). Debug builds
// never run `open` (decided here from cfg!(debug_assertions)): they return
// the argv they would have run, so the build host never opens Installer.

use std::path::Path;

use crate::platform::runner::{CommandRunner, SystemRunner};
use crate::update_download::{parse_sha256_text, OpenResult};

const SHASUM: &str = "/usr/bin/shasum";
const OPEN: &str = "/usr/bin/open";

/// Lower-case hex SHA-256 of `path`, or a human error.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    sha256_with(&SystemRunner, path)
}

/// Opens the pkg in Installer (release builds). Debug builds only return the
/// command.
pub fn open_pkg(path: &Path) -> Result<OpenResult, String> {
    open_pkg_with(&SystemRunner, path, !cfg!(debug_assertions))
}

fn path_arg(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("The file path isn't valid text: {}", path.display()))
}

pub(crate) fn sha256_with(runner: &dyn CommandRunner, path: &Path) -> Result<String, String> {
    let p = path_arg(path)?;
    let out = runner
        .run(SHASUM, &["-a", "256", p])
        .map_err(|e| format!("Couldn't run shasum: {}", e))?;
    if out.code != Some(0) {
        let detail = out.stderr.trim();
        return Err(if detail.is_empty() {
            "shasum failed.".to_string()
        } else {
            format!("shasum failed: {}", detail)
        });
    }
    parse_sha256_text(&out.stdout).ok_or_else(|| "shasum gave no checksum.".to_string())
}

pub(crate) fn open_pkg_with(
    runner: &dyn CommandRunner,
    path: &Path,
    execute: bool,
) -> Result<OpenResult, String> {
    let p = path_arg(path)?;
    if !execute {
        return Ok(OpenResult::Debug {
            would_run: vec![OPEN.to_string(), p.to_string()],
        });
    }
    let out = runner
        .run(OPEN, &[p])
        .map_err(|e| format!("Couldn't open Installer: {}", e))?;
    if out.code != Some(0) {
        let detail = out.stderr.trim();
        return Err(if detail.is_empty() {
            "Installer couldn't be opened.".to_string()
        } else {
            format!("Installer couldn't be opened: {}", detail)
        });
    }
    Ok(OpenResult::Opened {
        path: p.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::runner::FakeRunner;

    const HASH: &str = "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b";

    fn argv(p: &str, a: &[&str]) -> (String, Vec<String>) {
        (p.to_string(), a.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn sha256_runs_shasum_argv_and_parses() {
        let fake = FakeRunner::new();
        fake.push_ok(&format!("{}  /tmp/x y.pkg\n", HASH.to_uppercase()));
        let got = sha256_with(&fake, Path::new("/tmp/x y.pkg")).unwrap();
        assert_eq!(got, HASH);
        assert_eq!(
            fake.calls(),
            vec![argv(SHASUM, &["-a", "256", "/tmp/x y.pkg"])]
        );
    }

    #[test]
    fn sha256_failure_and_garbage() {
        let fake = FakeRunner::new();
        fake.push(Some(1), "", "no such file");
        assert_eq!(
            sha256_with(&fake, Path::new("/tmp/a.pkg")).unwrap_err(),
            "shasum failed: no such file"
        );
        fake.push_ok("not a hash\n");
        assert!(sha256_with(&fake, Path::new("/tmp/a.pkg")).is_err());
        // Queue empty → the runner errors (like a missing binary).
        assert!(sha256_with(&fake, Path::new("/tmp/a.pkg"))
            .unwrap_err()
            .starts_with("Couldn't run shasum"));
    }

    /// The download's checksum step with shasum faked: match, mismatch,
    /// shasum failure.
    #[test]
    fn checksum_compare_with_fake_shasum() {
        use crate::update_download::{verify_checksum, DownloadError, PartFile};
        let dir = std::env::temp_dir().join(format!("ihwip-installer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.pkg.part");
        let p_str = p.to_str().unwrap().to_string();

        for (canned, expect) in [
            (Ok(format!("{}  {}\n", HASH, p_str)), Ok("verified")),
            (
                Ok(format!("{}  {}\n", "0".repeat(64), p_str)),
                Err(DownloadError::ChecksumMismatch),
            ),
            (
                Err("permission denied"),
                Err(DownloadError::ChecksumUnreadable(
                    "shasum failed: permission denied".into(),
                )),
            ),
        ] {
            std::fs::write(&p, b"x").unwrap();
            let fake = FakeRunner::new();
            match &canned {
                Ok(out) => fake.push_ok(out),
                Err(err) => fake.push(Some(1), "", err),
            };
            let part = PartFile::for_test(p.clone());
            let got = verify_checksum(&part, Some(HASH), |f| sha256_with(&fake, f));
            assert_eq!(got, expect);
            assert_eq!(fake.calls(), vec![argv(SHASUM, &["-a", "256", &p_str])]);
            let ok = got.is_ok();
            drop(part);
            assert!(
                !p.exists(),
                "the caller's guard removes the file (ok={})",
                ok
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_debug_returns_argv_and_runs_nothing() {
        let fake = FakeRunner::new();
        let r = open_pkg_with(&fake, Path::new("/tmp/U/ItHurtsWhenIP-1.14.0.pkg"), false).unwrap();
        assert_eq!(
            r,
            OpenResult::Debug {
                would_run: vec![
                    "/usr/bin/open".to_string(),
                    "/tmp/U/ItHurtsWhenIP-1.14.0.pkg".to_string()
                ]
            }
        );
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn open_release_path_uses_argv_with_fake() {
        let fake = FakeRunner::new();
        fake.push_ok("");
        let r = open_pkg_with(&fake, Path::new("/tmp/a b.pkg"), true).unwrap();
        assert_eq!(
            r,
            OpenResult::Opened {
                path: "/tmp/a b.pkg".to_string()
            }
        );
        assert_eq!(fake.calls(), vec![argv(OPEN, &["/tmp/a b.pkg"])]);

        fake.push(Some(1), "", "LSOpenURLsWithRole() failed");
        assert_eq!(
            open_pkg_with(&fake, Path::new("/tmp/a b.pkg"), true).unwrap_err(),
            "Installer couldn't be opened: LSOpenURLsWithRole() failed"
        );
    }

    /// This test binary is a debug build: the public wrapper must never
    /// execute (it would open Installer on the build host).
    #[cfg(debug_assertions)]
    #[test]
    fn public_open_is_debug_in_tests() {
        let r = open_pkg(Path::new("/nonexistent/ihwip-test.pkg")).unwrap();
        assert!(matches!(r, OpenResult::Debug { .. }));
    }
}
