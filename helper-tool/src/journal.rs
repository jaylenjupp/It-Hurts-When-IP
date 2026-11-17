// The change journal (schema 1). The helper writes it as root, before the
// first change to each service, so the original settings can be put back
// later (by restore_service, by uninstall, or by the rescue script).
//
// File format (whole file at most 64 KB = 65536 bytes):
//   {"schema":1,"services":[
//   {"service":"…","recorded_at":"YYYY-MM-DDTHH:MM:SSZ","getinfo":"…","dns":"…","search":"…"},
//   …
//   ]}
// getinfo, dns and search hold the raw stdout of `networksetup -getinfo`,
// `-getdnsservers` and `-getsearchdomains` for that service.
//
// The rescue script (recovery/ithurtswhenip-rescue.command, section "journal
// (read-only)") reads this file with its own strict bash parser, so:
//  - Writer: canonical and ASCII only. Fixed key order, one entry per line.
//    `"`, `\`, newline, CR and tab use short escapes; every other control
//    character, DEL and all non-ASCII use \uXXXX (surrogate pairs above
//    U+FFFF); NUL is written as �. Its output is pinned by
//    recovery/tests/fixtures/journal-from-helper.json, which the rescue
//    script's own tests read.
//  - Reader: serde_json::Value plus the checks in `parse`. Like the bash
//    reader it tolerates unknown keys (top level and per entry; they are
//    dropped when the helper rewrites the file) and treats a missing
//    recorded_at, dns or search as "" (but each must be a string if present).
//
// Known differences from the bash reader. They only arise in files the helper
// never writes:
//  - Accepted here but refused by bash: duplicate keys (serde_json keeps the
//    last one); \u0000 in getinfo, dns, search or recorded_at; a raw DEL byte
//    inside any string except the service name; nesting deeper than 32 levels
//    (serde_json allows 128). The rescue script then lists Manual services
//    instead (safe), and the helper's next rewrite makes the file canonical.
//  - Refused here but accepted by bash: number tokens that aren't valid JSON
//    (such as 1.2.3 under an unknown key); invalid UTF-8; a service name with
//    a C1 control character (U+0080 to U+009F).

use serde_json::{Map, Value};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write as _};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA: u64 = 1;
/// The rescue script's JOURNAL_MAX_BYTES.
pub const MAX_BYTES: usize = 65536;
/// Set explicitly: the LaunchDaemon runs with Umask 077.
const DIR_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;

/// One service's original settings, as recorded before its first change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub service: String,
    pub recorded_at: String,
    pub getinfo: String,
    pub dns: String,
    pub search: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No journal file. Normal until the first change.
    None,
    Ok(Vec<Entry>),
    /// A journal file exists but can't be used; the text says why.
    Unusable(String),
}

/// Where "now" comes from, so tests get a fixed time.
pub trait Clock: Send + Sync {
    /// Seconds since 1970-01-01T00:00:00Z.
    fn now_secs(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

// --- Time (std only) ---

/// (year, month, day, hour, minute, second) in UTC.
fn civil(secs: u64) -> (i64, i64, i64, u64, u64, u64) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days since 1970-01-01 to a Gregorian date (Howard Hinnant's
    // civil_from_days; days >= 0 here, so plain division is fine).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3_600, rem / 60 % 60, rem % 60)
}

/// `YYYY-MM-DDTHH:MM:SSZ`, the recorded_at format.
pub fn utc_stamp(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
}

/// `YYYYMMDDTHHMMSSZ`, for file names.
pub fn utc_compact(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", y, mo, d, h, mi, s)
}

// --- Reading ---

/// Read and check the journal. Never follows a symlink.
pub fn load(path: &Path) -> State {
    match read_file(path) {
        Ok(None) => State::None,
        Ok(Some(bytes)) => match parse(&bytes) {
            Ok(entries) => State::Ok(entries),
            Err(why) => State::Unusable(why),
        },
        Err(why) => State::Unusable(why),
    }
}

/// The journal's bytes, or None if there is no journal file.
fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot be read: {}", e)),
    };
    if !meta.file_type().is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err("larger than 64 KB".to_string());
    }
    let file = fs::File::open(path).map_err(|e| format!("cannot be read: {}", e))?;
    // What was opened must be the regular file checked above, not something
    // swapped in since.
    let opened = file
        .metadata()
        .map_err(|e| format!("cannot be read: {}", e))?;
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return Err("changed while it was being read".to_string());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot be read: {}", e))?;
    if bytes.len() > MAX_BYTES {
        return Err("larger than 64 KB".to_string());
    }
    Ok(Some(bytes))
}

/// Parse and check journal text (see the top of this file for the rules).
pub fn parse(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not valid UTF-8".to_string())?;
    let top: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {}", e))?;
    let top = top
        .as_object()
        .ok_or_else(|| "not a JSON object".to_string())?;
    match top.get("schema") {
        None => return Err("no schema number".to_string()),
        Some(Value::Number(n)) if n.as_u64() == Some(SCHEMA) => {}
        Some(Value::Number(n)) => return Err(format!("unknown schema {}", n)),
        Some(_) => return Err("unknown schema (not a number)".to_string()),
    }
    let list = match top.get("services") {
        None => return Err("no services list".to_string()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err("schema 1 but \"services\" is not a list".to_string()),
    };
    let mut entries: Vec<Entry> = Vec::with_capacity(list.len());
    for item in list {
        let obj = item
            .as_object()
            .ok_or_else(|| "schema 1 but an entry is not an object".to_string())?;
        let service = text_field(obj, "service")?.unwrap_or_default();
        let getinfo = text_field(obj, "getinfo")?
            .ok_or_else(|| "schema 1 but an entry has no getinfo text".to_string())?;
        let entry = Entry {
            recorded_at: text_field(obj, "recorded_at")?.unwrap_or_default(),
            dns: text_field(obj, "dns")?.unwrap_or_default(),
            search: text_field(obj, "search")?.unwrap_or_default(),
            service,
            getinfo,
        };
        if entry.service.is_empty() {
            return Err("schema 1 but an entry has no service name".to_string());
        }
        if entry.service.chars().any(char::is_control) {
            return Err("schema 1 but a service name contains a control character".to_string());
        }
        if entries.iter().any(|e| e.service == entry.service) {
            return Err("schema 1 but a service is listed twice".to_string());
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// A string field: absent → None; present but not a string → error.
fn text_field(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("schema 1 but \"{}\" is not a string", key)),
    }
}

// --- Writing ---

/// The canonical file text (see the top of this file).
pub fn to_json(entries: &[Entry]) -> String {
    let mut out = String::from("{\"schema\":1,\"services\":[");
    for (i, e) in entries.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        for (j, (key, value)) in [
            ("service", &e.service),
            ("recorded_at", &e.recorded_at),
            ("getinfo", &e.getinfo),
            ("dns", &e.dns),
            ("search", &e.search),
        ]
        .into_iter()
        .enumerate()
        {
            out.push_str(if j == 0 { "{\"" } else { ",\"" });
            out.push_str(key);
            out.push_str("\":");
            push_json_string(&mut out, value);
        }
        out.push('}');
    }
    if !entries.is_empty() {
        out.push('\n');
    }
    out.push_str("]}\n");
    out
}

/// A JSON string literal using only printable ASCII.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // The bash reader refuses \u0000 (and so the whole journal).
            '\0' => out.push_str("\\ufffd"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{:04x}", unit);
                }
            }
        }
    }
    out.push('"');
}

/// Replace the journal atomically: temp file in the same folder, write,
/// sync, mode 0644, rename. Creates the folder (0755) if it is missing.
/// Refuses if the journal path is a symlink or not a regular file, and never
/// writes a file the rescue script would refuse as too large.
pub fn write(path: &Path, entries: &[Entry]) -> Result<(), String> {
    write_via(path, entries, &temp_path(path))
}

/// `write`, with the temp file name given (tests pass a known one).
fn write_via(path: &Path, entries: &[Entry], tmp: &Path) -> Result<(), String> {
    let text = to_json(entries);
    if text.len() > MAX_BYTES {
        return Err("the journal would be larger than 64 KB".to_string());
    }
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => return Err(format!("{} has no folder", path.display())),
    };
    ensure_dir(dir)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(format!("{} is not a regular file", path.display()))
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not check {}: {}", path.display(), e)),
    }

    let mut file = create_temp(tmp)?;
    let written = write_temp(&mut file, text.as_bytes());
    drop(file);
    // rename replaces the journal's directory entry; it never follows a link.
    if let Err(e) = written.and_then(|_| fs::rename(tmp, path)) {
        let _ = fs::remove_file(tmp);
        return Err(format!("could not write {}: {}", path.display(), e));
    }
    // Best effort: make the rename itself durable.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Create the temp file with create_new (O_CREAT|O_EXCL), which fails on
/// anything already at that name, a symlink included, so it is never followed
/// or reused. A crash or power loss between create and rename can leave a
/// temp file behind, and a later helper with the same pid would pick the same
/// name: a leftover regular file is removed and creation retried once, so it
/// can never block the journal. Anything else there (symlink, folder…) is
/// refused.
fn create_temp(tmp: &Path) -> Result<fs::File, String> {
    let create = || {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(tmp)
    };
    let failed = |e: io::Error| format!("could not create {}: {}", tmp.display(), e);
    match create() {
        Ok(file) => Ok(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match fs::symlink_metadata(tmp) {
            Ok(meta) if meta.file_type().is_file() => {
                // unlink removes the name itself; it never follows a link.
                fs::remove_file(tmp).map_err(|e| {
                    format!("could not remove the leftover {}: {}", tmp.display(), e)
                })?;
                create().map_err(failed)
            }
            _ => Err(failed(e)),
        },
        Err(e) => Err(failed(e)),
    }
}

fn write_temp(file: &mut fs::File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()?;
    // Umask 077 left it 0600: set the mode on the open file (fchmod).
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
}

/// The journal folder must be a real folder (not a symlink). If it is
/// missing it is created 0755; an existing folder's mode is left alone.
fn ensure_dir(dir: &Path) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!("{} is not a folder", dir.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(DIR_MODE)
                .create(dir)
                .map_err(|e| format!("could not create {}: {}", dir.display(), e))?;
            fs::set_permissions(dir, fs::Permissions::from_mode(DIR_MODE))
                .map_err(|e| format!("could not set the mode of {}: {}", dir.display(), e))
        }
        Err(e) => Err(format!("could not check {}: {}", dir.display(), e)),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "journal".to_string());
    path.with_file_name(format!(
        "{}.tmp-{}-{}",
        name,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

// --- Start-up ---

/// Start-up only. If the journal exists but is unusable, rename it to
/// `<stem>.unusable-<YYYYMMDDTHHMMSSZ>.json` in the same folder (or `…-2.json`,
/// `…-3.json`… if that name is taken). Nothing is deleted or overwritten.
/// Returns the new path and why it was unusable; Ok(None) if the journal is
/// fine or missing.
pub fn quarantine(path: &Path, now_secs: u64) -> Result<Option<(PathBuf, String)>, String> {
    let why = match load(path) {
        State::Unusable(why) => why,
        State::None | State::Ok(_) => return Ok(None),
    };
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "journal".to_string());
    let stamp = utc_compact(now_secs);
    for n in 1..=999u32 {
        let name = if n == 1 {
            format!("{}.unusable-{}.json", stem, stamp)
        } else {
            format!("{}.unusable-{}-{}.json", stem, stamp, n)
        };
        let target = path.with_file_name(name);
        match fs::symlink_metadata(&target) {
            Ok(_) => continue,
            // Check, then rename: the folder is root-only and nothing else
            // writes there while the helper starts.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return match fs::rename(path, &target) {
                    Ok(()) => Ok(Some((target, why))),
                    Err(e) => Err(format!(
                        "it is unusable ({}) but could not be moved to {}: {}",
                        why,
                        target.display(),
                        e
                    )),
                };
            }
            Err(e) => {
                return Err(format!(
                    "it is unusable ({}) but {} could not be checked: {}",
                    why,
                    target.display(),
                    e
                ))
            }
        }
    }
    Err(format!(
        "it is unusable ({}) but there is no free name to move it to",
        why
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::os::unix::fs::symlink;

    fn entry(service: &str, getinfo: &str) -> Entry {
        Entry {
            service: service.to_string(),
            recorded_at: "2026-11-16T09:30:00Z".to_string(),
            getinfo: getinfo.to_string(),
            dns: String::new(),
            search: String::new(),
        }
    }

    /// Permission bits including setuid/setgid/sticky.
    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0xFFF
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // --- time ---

    #[test]
    fn utc_stamps_match_date_u() {
        // Reference values from `date -u -r <secs>` on the Mac Studio.
        for (secs, stamp, compact) in [
            (0, "1970-01-01T00:00:00Z", "19700101T000000Z"),
            (951_782_400, "2000-02-29T00:00:00Z", "20000229T000000Z"),
            (1_709_251_199, "2024-02-29T23:59:59Z", "20240229T235959Z"),
            (1_772_323_199, "2026-02-28T23:59:59Z", "20260228T235959Z"),
            (1_794_821_400, "2026-11-16T09:30:00Z", "20261116T093000Z"),
            (1_798_761_599, "2026-12-31T23:59:59Z", "20261231T235959Z"),
            (4_102_444_799, "2099-12-31T23:59:59Z", "20991231T235959Z"),
        ] {
            assert_eq!(utc_stamp(secs), stamp, "{}", secs);
            assert_eq!(utc_compact(secs), compact, "{}", secs);
        }
    }

    // --- writer ---

    #[test]
    fn writer_is_canonical_ascii() {
        assert_eq!(to_json(&[]), "{\"schema\":1,\"services\":[]}\n");
        let e = Entry {
            service: "Caf\u{e9} \"A\", B\\".to_string(),
            recorded_at: "2026-11-16T09:30:00Z".to_string(),
            getinfo: "L1\nL2\r\n\tx\u{7f}\u{85}\u{1}/".to_string(),
            dns: "There aren't any\0".to_string(),
            search: "\u{1f39b} rack".to_string(),
        };
        let text = to_json(&[e.clone(), entry("Wi-Fi", "DHCP Configuration\n")]);
        assert_eq!(
            text,
            "{\"schema\":1,\"services\":[\n\
             {\"service\":\"Caf\\u00e9 \\\"A\\\", B\\\\\",\"recorded_at\":\"2026-11-16T09:30:00Z\",\
             \"getinfo\":\"L1\\nL2\\r\\n\\tx\\u007f\\u0085\\u0001/\",\
             \"dns\":\"There aren't any\\ufffd\",\"search\":\"\\ud83c\\udf9b rack\"},\n\
             {\"service\":\"Wi-Fi\",\"recorded_at\":\"2026-11-16T09:30:00Z\",\
             \"getinfo\":\"DHCP Configuration\\n\",\"dns\":\"\",\"search\":\"\"}\n\
             ]}\n"
        );
        assert!(text
            .bytes()
            .all(|b| b == b'\n' || (b' '..=b'~').contains(&b)));

        // Round trip: everything but NUL comes back unchanged.
        let back = parse(text.as_bytes()).unwrap();
        let mut want = e;
        want.dns = "There aren't any\u{fffd}".to_string();
        assert_eq!(back[0], want);
        assert_eq!(back[1], entry("Wi-Fi", "DHCP Configuration\n"));
    }

    // --- reader ---

    #[test]
    fn parser_accepts_what_the_bash_reader_accepts() {
        let ok = |text: &str| parse(text.as_bytes()).unwrap_or_else(|e| panic!("{}: {}", text, e));
        assert_eq!(ok("{\"schema\":1,\"services\":[]}"), vec![]);
        // Unknown keys anywhere, any key order, whitespace, missing optionals.
        let got = ok("\r\n { \"services\" : [ { \"extra\":{\"a\":[1,2,{\"b\":null}],\"c\":true}, \
                      \"getinfo\":\"DHCP Configuration\", \"service\":\"Wi-Fi\", \"n\":-1.5e3 } ] , \
                      \"writer\":\"helper 2.0\", \"schema\" : 1 }\n");
        assert_eq!(
            got,
            vec![Entry {
                service: "Wi-Fi".to_string(),
                recorded_at: String::new(),
                getinfo: "DHCP Configuration".to_string(),
                dns: String::new(),
                search: String::new(),
            }]
        );
        // Escapes and non-ASCII, raw or escaped.
        let got = ok("{\"schema\":1,\"services\":[{\"service\":\"A \\\"q\\\" \\\\ \\/ caf\\u00e9 \u{e9} \
                      \\ud83d\\ude00\",\"getinfo\":\"\",\"recorded_at\":\"t\",\"dns\":\"\",\"search\":\"\"}]}");
        assert_eq!(got[0].service, "A \"q\" \\ / caf\u{e9} \u{e9} \u{1f600}");
        assert_eq!(got[0].getinfo, "");
    }

    #[test]
    fn parser_refuses_bad_journals() {
        let cases: &[(&str, &[u8])] = &[
            ("not valid UTF-8", b"{\"schema\":1,\"services\":[{\"service\":\"\xff\",\"getinfo\":\"\"}]}"),
            ("not valid JSON", b""),
            ("not valid JSON", b"{\"schema\":1,\"services\":[{\"service\":\"Dante"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[]} x"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[]}{}"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[{\"service\":\"A\nB\",\"getinfo\":\"\"}]}"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[{\"service\":\"A\\ud83d x\",\"getinfo\":\"\"}]}"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[{\"service\":\"A\\x41\",\"getinfo\":\"\"}]}"),
            ("not valid JSON", b"{\"schema\":1,\"services\":[],\"x\":nope}"),
            ("not valid JSON", b"{\"schema\":1,\x00\"services\":[]}"),
            ("not a JSON object", b"[1]"),
            ("not a JSON object", b"\"journal\""),
            ("no schema number", b"{\"services\":[]}"),
            ("unknown schema 2", b"{\"schema\":2,\"services\":[]}"),
            ("unknown schema 1.0", b"{\"schema\":1.0,\"services\":[]}"),
            ("unknown schema -1", b"{\"schema\":-1,\"services\":[]}"),
            ("unknown schema (not a number)", b"{\"schema\":\"1\",\"services\":[]}"),
            ("unknown schema (not a number)", b"{\"schema\":null,\"services\":[]}"),
            ("no services list", b"{\"schema\":1}"),
            ("schema 1 but \"services\" is not a list", b"{\"schema\":1,\"services\":{}}"),
            ("schema 1 but an entry is not an object", b"{\"schema\":1,\"services\":[\"Wi-Fi\"]}"),
            ("schema 1 but an entry has no service name", b"{\"schema\":1,\"services\":[{\"getinfo\":\"\"}]}"),
            ("schema 1 but an entry has no service name", b"{\"schema\":1,\"services\":[{\"service\":\"\",\"getinfo\":\"\"}]}"),
            ("schema 1 but \"service\" is not a string", b"{\"schema\":1,\"services\":[{\"service\":5,\"getinfo\":\"\"}]}"),
            ("schema 1 but an entry has no getinfo text", b"{\"schema\":1,\"services\":[{\"service\":\"A\"}]}"),
            ("schema 1 but \"getinfo\" is not a string", b"{\"schema\":1,\"services\":[{\"service\":\"A\",\"getinfo\":null}]}"),
            ("schema 1 but \"dns\" is not a string", b"{\"schema\":1,\"services\":[{\"service\":\"A\",\"getinfo\":\"\",\"dns\":null}]}"),
            ("schema 1 but \"search\" is not a string", b"{\"schema\":1,\"services\":[{\"service\":\"A\",\"getinfo\":\"\",\"search\":[]}]}"),
            ("schema 1 but \"recorded_at\" is not a string", b"{\"schema\":1,\"services\":[{\"service\":\"A\",\"getinfo\":\"\",\"recorded_at\":0}]}"),
            ("schema 1 but a service name contains a control character", b"{\"schema\":1,\"services\":[{\"service\":\"A\\nrm -rf /\",\"getinfo\":\"\"}]}"),
            ("schema 1 but a service name contains a control character", b"{\"schema\":1,\"services\":[{\"service\":\"A\\u0000\",\"getinfo\":\"\"}]}"),
            ("schema 1 but a service name contains a control character", b"{\"schema\":1,\"services\":[{\"service\":\"A\x7f\",\"getinfo\":\"\"}]}"),
            ("schema 1 but a service is listed twice", b"{\"schema\":1,\"services\":[{\"service\":\"A\",\"getinfo\":\"\"},{\"service\":\"A\",\"getinfo\":\"\"}]}"),
        ];
        for (want, text) in cases {
            let why = parse(text)
                .err()
                .unwrap_or_else(|| panic!("accepted: {:?}", String::from_utf8_lossy(text)));
            assert!(
                why.starts_with(want),
                "{:?}: got {:?}, want {:?}",
                String::from_utf8_lossy(text),
                why,
                want
            );
        }
    }

    // --- load ---

    #[test]
    fn load_states() {
        let dir = TempDir::new();
        let path = dir.join("journal.json");
        assert_eq!(load(&path), State::None);
        assert_eq!(load(&dir.join("missing/journal.json")), State::None);

        fs::write(&path, to_json(&[entry("Wi-Fi", "DHCP Configuration\n")])).unwrap();
        assert_eq!(
            load(&path),
            State::Ok(vec![entry("Wi-Fi", "DHCP Configuration\n")])
        );

        fs::write(&path, "{\"schema\":2,\"services\":[]}").unwrap();
        assert_eq!(load(&path), State::Unusable("unknown schema 2".to_string()));

        // Exactly 64 KB is fine; one byte more is not.
        let mut text = "{\"schema\":1,\"services\":[]}".to_string();
        text.push_str(&" ".repeat(MAX_BYTES - text.len()));
        fs::write(&path, &text).unwrap();
        assert_eq!(load(&path), State::Ok(vec![]));
        text.push(' ');
        fs::write(&path, &text).unwrap();
        assert_eq!(
            load(&path),
            State::Unusable("larger than 64 KB".to_string())
        );

        // A symlink to a good journal and a folder are both refused.
        let real = dir.join("real.json");
        fs::write(&real, "{\"schema\":1,\"services\":[]}").unwrap();
        let link = dir.join("link.json");
        symlink(&real, &link).unwrap();
        assert_eq!(
            load(&link),
            State::Unusable("not a regular file".to_string())
        );
        let folder = dir.join("folder.json");
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            load(&folder),
            State::Unusable("not a regular file".to_string())
        );

        // The journal's folder is a file.
        let file = dir.join("file");
        fs::write(&file, "x").unwrap();
        match load(&file.join("journal.json")) {
            State::Unusable(why) => assert!(why.starts_with("cannot be read"), "{}", why),
            other => panic!("{:?}", other),
        }
    }

    // --- write ---

    #[test]
    fn write_creates_folder_0755_and_file_0644_atomically() {
        let dir = TempDir::new();
        let folder = dir.join("com.ithurtswhenip.helper");
        let path = folder.join("journal.json");
        let entries = vec![entry("Wi-Fi", "DHCP Configuration\n")];
        write(&path, &entries).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), to_json(&entries));
        assert_eq!(mode(&folder), 0o755);
        assert_eq!(mode(&path), 0o644);
        assert_eq!(names_in(&folder), vec!["journal.json"]);

        // Rewriting replaces the file and leaves no temp file behind.
        write(&path, &[]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"schema\":1,\"services\":[]}\n"
        );
        assert_eq!(mode(&path), 0o644);
        assert_eq!(names_in(&folder), vec!["journal.json"]);

        // An existing folder keeps its mode.
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        write(&path, &entries).unwrap();
        assert_eq!(mode(&folder), 0o700);
    }

    #[test]
    fn write_refuses_links_folders_and_oversize() {
        let dir = TempDir::new();
        let path = dir.join("journal.json");
        let entries = vec![entry("Wi-Fi", "DHCP Configuration\n")];

        // Symlink at the journal path: refused, link and target untouched.
        let target = dir.join("target.txt");
        fs::write(&target, "keep").unwrap();
        symlink(&target, &path).unwrap();
        let why = write(&path, &entries).unwrap_err();
        assert!(why.contains("not a regular file"), "{}", why);
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        fs::remove_file(&path).unwrap();

        // A folder at the journal path.
        fs::create_dir(&path).unwrap();
        assert!(write(&path, &entries)
            .unwrap_err()
            .contains("not a regular file"));
        fs::remove_dir(&path).unwrap();

        // The journal folder is a symlink to a folder.
        let real_dir = dir.join("real-dir");
        fs::create_dir(&real_dir).unwrap();
        let link_dir = dir.join("link-dir");
        symlink(&real_dir, &link_dir).unwrap();
        let why = write(&link_dir.join("journal.json"), &entries).unwrap_err();
        assert!(why.contains("is not a folder"), "{}", why);
        assert!(names_in(&real_dir).is_empty());

        // Exactly 64 KB is written; one byte more is refused and the
        // existing journal stays as it was.
        let base = to_json(&[entry("Wi-Fi", "")]).len();
        let fits = vec![entry("Wi-Fi", &"x".repeat(MAX_BYTES - base))];
        assert_eq!(to_json(&fits).len(), MAX_BYTES);
        write(&path, &fits).unwrap();
        let too_big = vec![entry("Wi-Fi", &"x".repeat(MAX_BYTES - base + 1))];
        let why = write(&path, &too_big).unwrap_err();
        assert_eq!(why, "the journal would be larger than 64 KB");
        assert_eq!(fs::read_to_string(&path).unwrap(), to_json(&fits));
    }

    #[test]
    fn a_leftover_temp_file_never_blocks_the_journal() {
        let dir = TempDir::new();
        let path = dir.join("journal.json");
        let tmp = dir.join("journal.json.tmp-123-0");
        let entries = vec![entry("Wi-Fi", "DHCP Configuration\n")];

        // A regular file left by a crash between create and rename: removed,
        // and the write goes ahead.
        fs::write(&tmp, "half-written journal").unwrap();
        write_via(&path, &entries, &tmp).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), to_json(&entries));
        assert_eq!(load(&path), State::Ok(entries.clone()));
        assert_eq!(mode(&path), 0o644);
        assert_eq!(names_in(dir.path()), vec!["journal.json"]);
    }

    #[test]
    fn a_link_or_folder_at_the_temp_name_is_refused() {
        let dir = TempDir::new();
        let path = dir.join("journal.json");
        let tmp = dir.join("journal.json.tmp-123-0");
        let old = vec![entry("Wi-Fi", "DHCP Configuration\n")];
        write(&path, &old).unwrap();
        let new = vec![entry("Dante Primary", "Manual Configuration\n")];

        // A symlink at the temp name: refused, link and target untouched,
        // journal unchanged.
        let target = dir.join("target.txt");
        fs::write(&target, "keep").unwrap();
        symlink(&target, &tmp).unwrap();
        let why = write_via(&path, &new, &tmp).unwrap_err();
        assert!(why.starts_with("could not create"), "{}", why);
        assert!(fs::symlink_metadata(&tmp).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        assert_eq!(load(&path), State::Ok(old.clone()));
        fs::remove_file(&tmp).unwrap();

        // A folder at the temp name: refused and left alone.
        fs::create_dir(&tmp).unwrap();
        let why = write_via(&path, &new, &tmp).unwrap_err();
        assert!(why.starts_with("could not create"), "{}", why);
        assert!(tmp.is_dir());
        assert_eq!(load(&path), State::Ok(old));
    }

    #[test]
    fn write_fails_cleanly_when_the_folder_cannot_be_used() {
        let dir = TempDir::new();
        let entries = vec![entry("Wi-Fi", "DHCP Configuration\n")];

        // Folder is a file.
        let file = dir.join("file");
        fs::write(&file, "x").unwrap();
        let why = write(&file.join("journal.json"), &entries).unwrap_err();
        assert!(why.contains("is not a folder"), "{}", why);
        assert_eq!(fs::read_to_string(&file).unwrap(), "x");

        // Parent of the folder is missing.
        let why = write(&dir.join("a/b/journal.json"), &entries).unwrap_err();
        assert!(why.starts_with("could not create"), "{}", why);

        // Read-only folder: nothing written, no temp file left.
        let ro = dir.join("ro");
        fs::create_dir(&ro).unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        let why = write(&ro.join("journal.json"), &entries).unwrap_err();
        assert!(why.starts_with("could not create"), "{}", why);
        assert!(names_in(&ro).is_empty());
    }

    // --- quarantine ---

    #[test]
    fn quarantine_moves_an_unusable_journal_aside_without_overwriting() {
        let dir = TempDir::new();
        let path = dir.join("journal.json");
        let now = 1_794_821_400; // 2026-11-16T09:30:00Z

        // Missing or usable: left alone.
        assert_eq!(quarantine(&path, now), Ok(None));
        fs::write(&path, "{\"schema\":1,\"services\":[]}").unwrap();
        assert_eq!(quarantine(&path, now), Ok(None));
        assert_eq!(names_in(dir.path()), vec!["journal.json"]);

        fs::write(&path, "{\"schema\":1,\"services\":[{\"service\":\"Da").unwrap();
        let (moved, why) = quarantine(&path, now).unwrap().unwrap();
        assert_eq!(moved, dir.join("journal.unusable-20261116T093000Z.json"));
        assert!(why.starts_with("not valid JSON"), "{}", why);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(&moved).unwrap(),
            "{\"schema\":1,\"services\":[{\"service\":\"Da"
        );

        // A second unusable journal in the same second gets a new name.
        fs::write(&path, "{\"schema\":7,\"services\":[]}").unwrap();
        let (second, why) = quarantine(&path, now).unwrap().unwrap();
        assert_eq!(second, dir.join("journal.unusable-20261116T093000Z-2.json"));
        assert_eq!(why, "unknown schema 7");
        assert_eq!(
            fs::read_to_string(&moved).unwrap(),
            "{\"schema\":1,\"services\":[{\"service\":\"Da"
        );
        assert_eq!(
            fs::read_to_string(&second).unwrap(),
            "{\"schema\":7,\"services\":[]}"
        );
        assert_eq!(
            names_in(dir.path()),
            vec![
                "journal.unusable-20261116T093000Z-2.json",
                "journal.unusable-20261116T093000Z.json"
            ]
        );

        // A symlink is renamed itself, never followed.
        let target = dir.join("target.txt");
        fs::write(&target, "keep").unwrap();
        symlink(&target, &path).unwrap();
        let (third, _) = quarantine(&path, now + 1).unwrap().unwrap();
        assert!(fs::symlink_metadata(&third)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }
}
