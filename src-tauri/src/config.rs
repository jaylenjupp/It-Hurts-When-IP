// Persistent app config (`config.json` in the app data folder).
//
// Path-based so it can be tested against a temp folder. `main.rs` resolves
// the real path and wraps these in Tauri commands.
//
// Migration rules:
// - Every field has a serde default, so older files (v1.13.1 and before) and
//   partial files load, and missing fields take their defaults.
// - Unknown fields are ignored (no deny_unknown_fields), so a config written
//   by a newer build still loads here.
// - A file that exists but can't be read or parsed is moved aside to
//   `config.unreadable-<UTC>.json` before anything can save over it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::build_info::utc::format_utc;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QuickSetProfile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub subnet: String,
    #[serde(default)]
    pub gateway: String,
}

impl QuickSetProfile {
    fn blank(slot: u8) -> Self {
        QuickSetProfile {
            name: format!("Quick Set {}", slot),
            ip: String::new(),
            subnet: String::new(),
            gateway: String::new(),
        }
    }
}

fn default_qs_1() -> QuickSetProfile {
    QuickSetProfile::blank(1)
}
fn default_qs_2() -> QuickSetProfile {
    QuickSetProfile::blank(2)
}
fn default_qs_3() -> QuickSetProfile {
    QuickSetProfile::blank(3)
}
fn default_qs_4() -> QuickSetProfile {
    QuickSetProfile::blank(4)
}
fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default = "default_qs_1")]
    pub quick_set_1: QuickSetProfile,
    #[serde(default = "default_qs_2")]
    pub quick_set_2: QuickSetProfile,
    #[serde(default = "default_qs_3")]
    pub quick_set_3: QuickSetProfile,
    #[serde(default = "default_qs_4")]
    pub quick_set_4: QuickSetProfile,
    #[serde(default)]
    pub update_prompt_dismissed_at: Option<i64>,
    /// Advanced options unlocked (Settings → Advanced). Off by default.
    #[serde(default)]
    pub advanced_mode: bool,
    /// Check GitHub for a newer release when the app starts. On by default.
    #[serde(default = "default_true")]
    pub check_updates_on_launch: bool,
    /// When the last successful update check happened (seconds since the
    /// Unix epoch). None until the first one.
    #[serde(default)]
    pub update_last_checked_at: Option<i64>,
}

// Hand-written so the defaults are visible in one place. New fields go in the
// struct (with a serde default) AND here.
impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            quick_set_1: default_qs_1(),
            quick_set_2: default_qs_2(),
            quick_set_3: default_qs_3(),
            quick_set_4: default_qs_4(),
            update_prompt_dismissed_at: None,
            advanced_mode: false,
            check_updates_on_launch: default_true(),
            update_last_checked_at: None,
        }
    }
}

/// What happened when the config file was loaded.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// No file yet; defaults.
    Missing,
    /// File read and parsed.
    Loaded,
    /// File was unreadable; it was moved to `backup` and defaults are used.
    Recovered { backup: PathBuf, reason: String },
    /// File was unreadable AND could not be moved aside. Defaults are used for
    /// display, but nothing may save over the file.
    RecoveryFailed { reason: String },
}

impl LoadStatus {
    fn log(&self, path: &Path) {
        match self {
            LoadStatus::Missing | LoadStatus::Loaded => {}
            LoadStatus::Recovered { backup, reason } => eprintln!(
                "[config] {} could not be loaded ({}); moved it to {} and using defaults",
                path.display(),
                reason,
                backup.display()
            ),
            LoadStatus::RecoveryFailed { reason } => eprintln!(
                "[config] {} could not be loaded and could not be moved aside ({}); \
                 using defaults and refusing to save over it",
                path.display(),
                reason
            ),
        }
    }
}

pub struct Loaded {
    pub config: AppConfig,
    pub status: LoadStatus,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Loads the config at `path`. Never fails: a missing or unreadable file
/// gives defaults (an unreadable one is moved aside first).
pub fn load_from(path: &Path) -> Loaded {
    load_from_at(path, now_secs())
}

/// `load_from` with a fixed clock (seconds since the Unix epoch) for the
/// backup file name.
pub fn load_from_at(path: &Path, now: u64) -> Loaded {
    let parsed = match fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                config: AppConfig::default(),
                status: LoadStatus::Missing,
            };
        }
        Err(e) => Err(format!("read failed: {}", e)),
        Ok(bytes) => parse(&bytes).map_err(|e| format!("not a valid config: {}", e)),
    };
    let status = match parsed {
        Ok(config) => {
            return Loaded {
                config,
                status: LoadStatus::Loaded,
            };
        }
        Err(reason) => match move_aside(path, now) {
            Ok(backup) => LoadStatus::Recovered { backup, reason },
            Err(e) => LoadStatus::RecoveryFailed {
                reason: format!("{}; backup failed: {}", reason, e),
            },
        },
    };
    status.log(path);
    Loaded {
        config: AppConfig::default(),
        status,
    }
}

/// Parses a config file. The top level must be a JSON object (serde would
/// otherwise accept `[]` as an all-defaults config).
fn parse(bytes: &[u8]) -> Result<AppConfig, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("top level is not a JSON object".to_string());
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Loads the config for a load → modify → save. Errors if the file is
/// unreadable and could not be moved aside, so it is never overwritten.
pub fn load_for_update(path: &Path) -> Result<AppConfig, String> {
    let loaded = load_from(path);
    match loaded.status {
        LoadStatus::RecoveryFailed { reason } => Err(format!(
            "Settings were not saved: the existing settings file at {} can't be read \
             and couldn't be backed up ({}). Move or delete it, then try again.",
            path.display(),
            reason
        )),
        _ => Ok(loaded.config),
    }
}

/// `config.unreadable-<yyyymmddThhmmssZ>.json`
fn backup_name(now: u64) -> String {
    let stamp: String = format_utc(now)
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .collect();
    format!("config.unreadable-{}.json", stamp)
}

/// Renames the unreadable file next to itself, adding `-N` if the name is taken.
fn move_aside(path: &Path, now: u64) -> Result<PathBuf, String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = backup_name(now);
    let stem = base.trim_end_matches(".json");
    for n in 0..100 {
        let name = if n == 0 {
            base.clone()
        } else {
            format!("{}-{}.json", stem, n)
        };
        let candidate = dir.join(name);
        if candidate.exists() {
            continue;
        }
        return fs::rename(path, &candidate)
            .map(|_| candidate)
            .map_err(|e| e.to_string());
    }
    Err("no free backup file name".to_string())
}

/// Saves atomically: temp file in the same folder, fsync, rename over `path`.
pub fn save_to(path: &Path, config: &AppConfig) -> Result<(), String> {
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        format!(
            "Couldn't save settings to {} ({}: {})",
            path.display(),
            what,
            e
        )
    };
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| fail("create folder", &e))?;
    let data = serde_json::to_string_pretty(config).map_err(|e| fail("encode", &e))?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.json".to_string());
    let tmp = dir.join(format!(".{}.tmp-{}", file_name, std::process::id()));

    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        fs::remove_file(&tmp).ok();
        return Err(fail("write", &e));
    }
    // Best effort: persist the rename itself.
    if let Ok(d) = fs::File::open(dir) {
        d.sync_all().ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const V1_13_1: &str = include_str!("testdata/config_v1_13_1.json");
    const V1_13_1_DISMISSED: &str = include_str!("testdata/config_v1_13_1_dismissed.json");
    const EXTRA_FIELD: &str = include_str!("testdata/config_extra_field.json");

    /// Temp folder removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "ihwip-config-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::remove_dir_all(&p).ok();
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn config(&self) -> PathBuf {
            self.0.join("config.json")
        }
        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
        fn backups(&self) -> Vec<String> {
            self.entries()
                .into_iter()
                .filter(|n| n.starts_with("config.unreadable-"))
                .collect()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn qs(name: &str, ip: &str, subnet: &str, gateway: &str) -> QuickSetProfile {
        QuickSetProfile {
            name: name.into(),
            ip: ip.into(),
            subnet: subnet.into(),
            gateway: gateway.into(),
        }
    }

    // 2026-11-13T10:20:30Z
    const NOW: u64 = 1_794_565_230;

    #[test]
    fn default_values() {
        let c = AppConfig::default();
        assert_eq!(c.quick_set_1, qs("Quick Set 1", "", "", ""));
        assert_eq!(c.quick_set_2, qs("Quick Set 2", "", "", ""));
        assert_eq!(c.quick_set_3, qs("Quick Set 3", "", "", ""));
        assert_eq!(c.quick_set_4, qs("Quick Set 4", "", "", ""));
        assert_eq!(c.update_prompt_dismissed_at, None);
        assert!(!c.advanced_mode);
        assert!(c.check_updates_on_launch);
        assert_eq!(c.update_last_checked_at, None);
    }

    #[test]
    fn empty_object_equals_default() {
        let c: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, AppConfig::default());
    }

    fn assert_v1_13_1_quick_sets(c: &AppConfig) {
        assert_eq!(
            c.quick_set_1,
            qs("Stage Left", "192.0.2.10", "255.255.255.0", "192.0.2.1")
        );
        assert_eq!(
            c.quick_set_2,
            qs("FOH Switch", "198.51.100.20", "255.255.255.0", "0.0.0.0")
        );
        assert_eq!(c.quick_set_3, qs("Quick Set 3", "", "", ""));
        assert_eq!(
            c.quick_set_4,
            qs("Lighting Net", "203.0.113.50", "255.255.0.0", "203.0.113.1")
        );
    }

    #[test]
    fn v1_13_1_loads_with_new_defaults() {
        let t = TempDir::new();
        fs::write(t.config(), V1_13_1).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_v1_13_1_quick_sets(&l.config);
        assert_eq!(l.config.update_prompt_dismissed_at, None);
        assert!(!l.config.advanced_mode);
        assert!(l.config.check_updates_on_launch);
        assert_eq!(l.config.update_last_checked_at, None);
        assert!(t.backups().is_empty());
    }

    #[test]
    fn v1_13_1_with_dismissed_at_loads_with_new_defaults() {
        let t = TempDir::new();
        fs::write(t.config(), V1_13_1_DISMISSED).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_v1_13_1_quick_sets(&l.config);
        assert_eq!(l.config.update_prompt_dismissed_at, Some(1_794_565_230));
        assert!(!l.config.advanced_mode);
        assert!(l.config.check_updates_on_launch);
        assert_eq!(l.config.update_last_checked_at, None);
    }

    #[test]
    fn settings_round_trip() {
        let t = TempDir::new();
        let c = AppConfig {
            advanced_mode: true,
            check_updates_on_launch: false,
            update_last_checked_at: Some(1_795_176_000),
            quick_set_2: qs("Rack", "198.51.100.7", "255.255.255.0", "0.0.0.0"),
            ..AppConfig::default()
        };
        save_to(&t.config(), &c).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_eq!(l.config, c);
        assert!(l.config.advanced_mode);
        assert!(!l.config.check_updates_on_launch);
        assert_eq!(l.config.update_last_checked_at, Some(1_795_176_000));
    }

    #[test]
    fn unknown_fields_ignored() {
        let t = TempDir::new();
        fs::write(t.config(), EXTRA_FIELD).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_eq!(
            l.config.quick_set_1,
            qs("Stage Left", "192.0.2.10", "255.255.255.0", "192.0.2.1")
        );
        assert_eq!(l.config.quick_set_4, qs("Quick Set 4", "", "", ""));
        assert!(l.config.advanced_mode);
        assert!(!l.config.check_updates_on_launch);
        assert_eq!(l.config.update_prompt_dismissed_at, None);
        assert!(t.backups().is_empty());
    }

    #[test]
    fn partial_file_keeps_what_it_has() {
        let t = TempDir::new();
        fs::write(
            t.config(),
            r#"{
                "quick_set_1": {"name": "Stage Left", "ip": "192.0.2.10", "subnet": "255.255.255.0", "gateway": "192.0.2.1"},
                "quick_set_2": {"name": "Half", "ip": "203.0.113.9"}
            }"#,
        )
        .unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_eq!(
            l.config.quick_set_1,
            qs("Stage Left", "192.0.2.10", "255.255.255.0", "192.0.2.1")
        );
        assert_eq!(l.config.quick_set_2, qs("Half", "203.0.113.9", "", ""));
        assert_eq!(l.config.quick_set_3, qs("Quick Set 3", "", "", ""));
        assert_eq!(l.config.quick_set_4, qs("Quick Set 4", "", "", ""));
        assert!(t.backups().is_empty());
    }

    #[test]
    fn corrupt_file_is_backed_up_then_save_works() {
        let t = TempDir::new();
        let bad: &[u8] = b"{\"quick_set_1\": {\"name\": \"Stage Left\", \"ip\": \"192.0.2.10\"";
        fs::write(t.config(), bad).unwrap();

        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.config, AppConfig::default());
        let backup = t.0.join("config.unreadable-20261113T102030Z.json");
        match &l.status {
            LoadStatus::Recovered { backup: b, .. } => assert_eq!(b, &backup),
            other => panic!("unexpected status {:?}", other),
        }
        assert_eq!(fs::read(&backup).unwrap(), bad);
        assert!(!t.config().exists());

        // A following save writes a valid config and keeps the backup.
        let mut c = load_for_update(&t.config()).unwrap();
        c.advanced_mode = true;
        save_to(&t.config(), &c).unwrap();
        let again = load_from_at(&t.config(), NOW);
        assert_eq!(again.status, LoadStatus::Loaded);
        assert!(again.config.advanced_mode);
        assert_eq!(fs::read(&backup).unwrap(), bad);
        assert_eq!(t.backups().len(), 1);
    }

    #[test]
    fn wrong_type_counts_as_unreadable() {
        let t = TempDir::new();
        fs::write(t.config(), r#"{"quick_set_1": 5}"#).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert!(matches!(l.status, LoadStatus::Recovered { .. }));
        assert_eq!(l.config, AppConfig::default());
        assert_eq!(t.backups().len(), 1);
    }

    #[test]
    fn empty_and_non_utf8_files_are_backed_up() {
        for bytes in [&b""[..], &b"\xff\xfe\x00junk"[..], &b"[]"[..], &b"null"[..]] {
            let t = TempDir::new();
            fs::write(t.config(), bytes).unwrap();
            let l = load_from_at(&t.config(), NOW);
            assert!(
                matches!(l.status, LoadStatus::Recovered { .. }),
                "bytes {:?} gave {:?}",
                bytes,
                l.status
            );
            assert_eq!(t.backups().len(), 1);
        }
    }

    #[test]
    fn backup_name_collision_adds_suffix() {
        let t = TempDir::new();
        fs::write(t.0.join("config.unreadable-20261113T102030Z.json"), "old").unwrap();
        fs::write(t.config(), "not json").unwrap();
        let l = load_from_at(&t.config(), NOW);
        let expected = t.0.join("config.unreadable-20261113T102030Z-1.json");
        match &l.status {
            LoadStatus::Recovered { backup, .. } => assert_eq!(backup, &expected),
            other => panic!("unexpected status {:?}", other),
        }
        assert_eq!(fs::read_to_string(&expected).unwrap(), "not json");
        assert_eq!(
            fs::read_to_string(t.0.join("config.unreadable-20261113T102030Z.json")).unwrap(),
            "old"
        );
    }

    #[test]
    fn backup_name_format() {
        assert_eq!(backup_name(NOW), "config.unreadable-20261113T102030Z.json");
        assert_eq!(backup_name(0), "config.unreadable-19700101T000000Z.json");
    }

    #[test]
    fn missing_file_gives_defaults_without_backup() {
        let t = TempDir::new();
        let l = load_from_at(&t.config(), NOW);
        assert_eq!(l.status, LoadStatus::Missing);
        assert_eq!(l.config, AppConfig::default());
        assert!(t.entries().is_empty());
        assert_eq!(load_for_update(&t.config()).unwrap(), AppConfig::default());
        assert!(t.entries().is_empty());
    }

    #[test]
    fn save_creates_parent_and_leaves_no_temp_file() {
        let t = TempDir::new();
        let path = t.0.join("nested").join("deeper").join("config.json");
        save_to(&path, &AppConfig::default()).unwrap();
        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.json".to_string()]);
        let l = load_from_at(&path, NOW);
        assert_eq!(l.status, LoadStatus::Loaded);
        assert_eq!(l.config, AppConfig::default());
    }

    #[test]
    fn save_overwrites_existing_config() {
        let t = TempDir::new();
        fs::write(t.config(), V1_13_1).unwrap();
        let mut c = load_for_update(&t.config()).unwrap();
        c.check_updates_on_launch = false;
        save_to(&t.config(), &c).unwrap();
        let l = load_from_at(&t.config(), NOW);
        assert_v1_13_1_quick_sets(&l.config);
        assert!(!l.config.check_updates_on_launch);
        assert_eq!(t.entries(), vec!["config.json".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn unmovable_unreadable_file_refuses_update() {
        use std::os::unix::fs::PermissionsExt;
        let t = TempDir::new();
        fs::write(t.config(), "not json").unwrap();
        // Read-only folder: the rename can't happen.
        fs::set_permissions(&t.0, fs::Permissions::from_mode(0o555)).unwrap();
        let l = load_from_at(&t.config(), NOW);
        let refused = load_for_update(&t.config());
        fs::set_permissions(&t.0, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(l.status, LoadStatus::RecoveryFailed { .. }),
            "got {:?}",
            l.status
        );
        assert_eq!(l.config, AppConfig::default());
        let err = refused.unwrap_err();
        assert!(err.contains("Settings were not saved"), "{}", err);
        assert_eq!(fs::read_to_string(t.config()).unwrap(), "not json");
        assert!(t.backups().is_empty());
    }

    #[test]
    fn save_error_is_reported() {
        let t = TempDir::new();
        // A file where the parent folder should be.
        let blocker = t.0.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let err = save_to(&blocker.join("config.json"), &AppConfig::default()).unwrap_err();
        assert!(err.starts_with("Couldn't save settings to"), "{}", err);
    }
}
