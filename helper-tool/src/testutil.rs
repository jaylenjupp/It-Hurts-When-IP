// Test-only helpers shared by the unit tests: a temp dir that cleans up after
// itself and a clock that always says the same time. Journal tests only ever
// write inside a `TempDir`.

use crate::journal::Clock;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ihwip-h-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A test may have made a subfolder read-only; make it removable again.
        if let Ok(list) = fs::read_dir(&self.0) {
            for entry in list.flatten() {
                if let Ok(meta) = fs::symlink_metadata(entry.path()) {
                    if meta.is_dir() {
                        let _ =
                            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Always the same time, so recorded_at is deterministic.
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_secs(&self) -> u64 {
        self.0
    }
}
