// Integration test: runs the DEBUG helper binary against a temp-dir socket
// and tools/fake-networksetup. Never the real socket, never the real
// networksetup, never root. The child is killed by a drop guard, also when a
// test panics.
//
// Debug profile only: under `cargo test --release` the binary under test
// would be the release helper, which uses the real socket path. This file
// then compiles to nothing, so no release helper is ever started.
#![cfg(debug_assertions)]

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const HELPER: &str = env!("CARGO_BIN_EXE_ithurtswhenip-helper");
const ENV_VARS: [&str; 3] = [
    "IHWIP_HELPER_SOCKET",
    "IHWIP_HELPER_JOURNAL",
    "IHWIP_NETWORKSETUP",
];

/// A fresh dir under the temp dir, removed on drop. Short name: Unix socket
/// paths are limited to 104 bytes on macOS.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ihwip-it-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Kills and reaps the helper when dropped (normal end or panic).
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fake_networksetup() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tools/fake-networksetup/fake-networksetup")
        .canonicalize()
        .expect("tools/fake-networksetup/fake-networksetup exists")
}

/// The helper command with every path inside `dir`. Any IHWIP_* the test
/// runner happens to have is cleared first.
fn helper_command(dir: &TempDir) -> Command {
    let mut cmd = Command::new(HELPER);
    for var in ENV_VARS.iter().chain(&[
        "IHWIP_FAKE_STATE",
        "IHWIP_FAKE_FIXTURES",
        "IHWIP_FAKE_FAIL",
        "IHWIP_FAKE_RESET",
    ]) {
        cmd.env_remove(var);
    }
    cmd.env("IHWIP_HELPER_SOCKET", dir.path("h.sock"))
        .env("IHWIP_HELPER_JOURNAL", dir.path("journal.json"))
        .env("IHWIP_NETWORKSETUP", fake_networksetup())
        .env("IHWIP_FAKE_STATE", dir.path("fake"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::File::create(dir.path("helper.err")).unwrap(),
        ));
    cmd
}

fn stderr_of(dir: &TempDir) -> String {
    fs::read_to_string(dir.path("helper.err")).unwrap_or_default()
}

/// Wait until the helper accepts a connection (or exits / times out).
fn wait_until_serving(child: &mut ChildGuard, dir: &TempDir) {
    let sock = dir.path("h.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("helper exited early ({}): {}", status, stderr_of(dir));
        }
        if UnixStream::connect(&sock).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "helper never started serving: {}",
            stderr_of(dir)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn request(sock: &Path, req: &Value) -> Value {
    let mut stream = UnixStream::connect(sock).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(format!("{}\n", req).as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad reply {:?}: {}", line, e))
}

#[test]
fn serves_ping_valid_and_hostile_requests_over_the_socket() {
    let dir = TempDir::new();
    let sock = dir.path("h.sock");
    let mut child = ChildGuard(helper_command(&dir).spawn().unwrap());
    wait_until_serving(&mut child, &dir);

    let ping = request(&sock, &json!({"v": 2, "cmd": "ping"}));
    assert_eq!(ping["ok"], json!("pong"));
    assert_eq!(ping["v"], json!(2));
    assert_eq!(ping["protocol"], json!(2));
    assert_eq!(ping["helper_version"], json!(env!("CARGO_PKG_VERSION")));
    assert!(ping["git_sha"].as_str().is_some_and(|s| !s.is_empty()));

    let ok = request(
        &sock,
        &json!({"v": 2, "cmd": "set_static_ip", "interface": "Dante Primary",
                "ip": "10.1.2.3", "subnet": "255.255.255.0", "gateway": ""}),
    );
    assert_eq!(ok, json!({"v": 2, "ok": "Set Dante Primary to 10.1.2.3"}));

    let v1 = request(&sock, &json!({"cmd": "set_dhcp", "interface": "Wi-Fi"}));
    assert_eq!(v1, json!({"ok": "Set Wi-Fi to DHCP"}));

    let hostile = request(
        &sock,
        &json!({"v": 2, "cmd": "set_static_ip", "interface": "'; rm -rf /'",
                "ip": "10.1.2.3", "subnet": "255.255.255.0", "gateway": ""}),
    );
    assert_eq!(hostile["code"], json!("unknown_service"));
    assert!(hostile.get("ok").is_none());

    let newline = request(
        &sock,
        &json!({"v": 2, "cmd": "set_dhcp", "interface": "Wi-Fi\nreboot"}),
    );
    assert_eq!(newline["code"], json!("invalid_service"));

    let bad_ip = request(
        &sock,
        &json!({"cmd": "set_static_ip", "interface": "Wi-Fi",
                "ip": "10.0.0.1; reboot", "subnet": "255.255.255.0", "gateway": ""}),
    );
    assert!(bad_ip["error"].is_string());

    let v3 = request(
        &sock,
        &json!({"v": 3, "cmd": "set_dhcp", "interface": "Wi-Fi"}),
    );
    assert_eq!(v3, json!({"error": "unsupported_version"}));

    // Socket is exactly 0660.
    let meta = fs::symlink_metadata(&sock).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.mode() & 0xFFF, 0o660, "mode {:o}", meta.mode());

    // The fake saw exactly: list + the three journal reads + write for each
    // valid request (first change to each service), list only for the
    // hostile service, and nothing at all for the rest.
    let log = fs::read_to_string(dir.path("fake/calls.log")).unwrap();
    assert_eq!(
        log,
        "networksetup -listallnetworkservices \n\
         networksetup -getinfo Dante\\ Primary \n\
         networksetup -getdnsservers Dante\\ Primary \n\
         networksetup -getsearchdomains Dante\\ Primary \n\
         networksetup -setmanual Dante\\ Primary 10.1.2.3 255.255.255.0 '' \n\
         networksetup -listallnetworkservices \n\
         networksetup -getinfo Wi-Fi \n\
         networksetup -getdnsservers Wi-Fi \n\
         networksetup -getsearchdomains Wi-Fi \n\
         networksetup -setdhcp Wi-Fi \n\
         networksetup -listallnetworkservices \n"
    );
    let info = fs::read_to_string(dir.path("fake/info/Dante_Primary.txt")).unwrap();
    assert!(info.contains("IP address: 10.1.2.3"), "{}", info);
}

const WIFI_FIXTURE: &str =
    include_str!("../../tools/fake-networksetup/fixtures/default/info/Wi-Fi.txt");

#[test]
fn journals_the_original_then_restores_it_over_the_socket() {
    let dir = TempDir::new();
    let sock = dir.path("h.sock");
    // The journal folder doesn't exist yet: the helper creates it.
    let journal = dir.path("state/journal.json");
    let mut cmd = helper_command(&dir);
    cmd.env("IHWIP_HELPER_JOURNAL", &journal);
    let mut child = ChildGuard(cmd.spawn().unwrap());
    wait_until_serving(&mut child, &dir);

    let set = request(
        &sock,
        &json!({"v": 2, "cmd": "set_static_ip", "interface": "Wi-Fi",
                "ip": "10.9.8.7", "subnet": "255.255.255.0", "gateway": "10.9.8.1"}),
    );
    assert_eq!(set, json!({"v": 2, "ok": "Set Wi-Fi to 10.9.8.7"}));

    // The original (the fake's fixture text) was recorded before the write.
    let text = fs::read_to_string(&journal).unwrap();
    assert!(
        text.starts_with("{\"schema\":1,\"services\":[\n{\"service\":\"Wi-Fi\",\"recorded_at\":\""),
        "{}",
        text
    );
    assert!(text.ends_with("}\n]}\n"), "{}", text);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    let entry = &parsed["services"][0];
    assert_eq!(entry["getinfo"], json!(WIFI_FIXTURE));
    assert_eq!(
        entry["dns"],
        json!("There aren't any DNS Servers set on Wi-Fi.\n")
    );
    assert_eq!(
        entry["search"],
        json!("There aren't any Search Domains set on Wi-Fi.\n")
    );
    let recorded_at = entry["recorded_at"].as_str().unwrap().to_string();
    assert_eq!(recorded_at.len(), 20, "{}", recorded_at);
    assert!(recorded_at.starts_with("20") && recorded_at.ends_with('Z'));
    assert_eq!(&recorded_at[10..11], "T");
    // 0xFFF: permission bits including setuid/setgid/sticky.
    assert_eq!(
        fs::symlink_metadata(&journal).unwrap().mode() & 0xFFF,
        0o644
    );
    assert_eq!(
        fs::symlink_metadata(dir.path("state")).unwrap().mode() & 0xFFF,
        0o755
    );

    let status = request(&sock, &json!({"v": 2, "cmd": "journal_status"}));
    assert_eq!(
        status,
        json!({"v": 2, "ok": "journal", "state": "ok",
               "services": [{"service": "Wi-Fi", "recorded_at": recorded_at}]})
    );

    let restore = json!({"v": 2, "cmd": "restore_service", "interface": "Wi-Fi"});
    assert_eq!(
        request(&sock, &restore),
        json!({"v": 2, "ok": "Restored Wi-Fi to its original settings (DHCP)."})
    );
    assert_eq!(
        fs::read_to_string(&journal).unwrap(),
        "{\"schema\":1,\"services\":[]}\n"
    );
    assert_eq!(
        request(&sock, &json!({"v": 2, "cmd": "journal_status"})),
        json!({"v": 2, "ok": "journal", "state": "ok", "services": []})
    );
    assert_eq!(request(&sock, &restore)["code"], json!("not_journaled"));

    // Exactly: record + write, then list + check + write + verify.
    assert_eq!(
        fs::read_to_string(dir.path("fake/calls.log")).unwrap(),
        "networksetup -listallnetworkservices \n\
         networksetup -getinfo Wi-Fi \n\
         networksetup -getdnsservers Wi-Fi \n\
         networksetup -getsearchdomains Wi-Fi \n\
         networksetup -setmanual Wi-Fi 10.9.8.7 255.255.255.0 10.9.8.1 \n\
         networksetup -listallnetworkservices \n\
         networksetup -getinfo Wi-Fi \n\
         networksetup -setdhcp Wi-Fi \n\
         networksetup -getinfo Wi-Fi \n"
    );
    assert_eq!(
        fs::read_to_string(dir.path("fake/info/Wi-Fi.txt")).unwrap(),
        WIFI_FIXTURE
    );
}

#[test]
fn an_unusable_journal_is_moved_aside_at_start_up() {
    let dir = TempDir::new();
    let sock = dir.path("h.sock");
    fs::write(dir.path("journal.json"), "not json").unwrap();
    let mut child = ChildGuard(helper_command(&dir).spawn().unwrap());
    wait_until_serving(&mut child, &dir);

    assert!(!dir.path("journal.json").exists());
    let moved: Vec<PathBuf> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("journal.unusable-") && n.ends_with("Z.json"))
        })
        .collect();
    assert_eq!(moved.len(), 1, "{:?}", moved);
    assert_eq!(fs::read_to_string(&moved[0]).unwrap(), "not json");
    let stderr = stderr_of(&dir);
    assert!(
        stderr.contains("was unusable (not valid JSON"),
        "{}",
        stderr
    );

    assert_eq!(
        request(&sock, &json!({"v": 2, "cmd": "journal_status"})),
        json!({"v": 2, "ok": "journal", "state": "none", "services": []})
    );
    // The helper carries on and records afresh.
    assert_eq!(
        request(
            &sock,
            &json!({"v": 2, "cmd": "set_dhcp", "interface": "Wi-Fi"})
        ),
        json!({"v": 2, "ok": "Set Wi-Fi to DHCP"})
    );
    let status = request(&sock, &json!({"v": 2, "cmd": "journal_status"}));
    assert_eq!(status["state"], json!("ok"));
    assert_eq!(status["services"][0]["service"], json!("Wi-Fi"));
    assert_eq!(fs::read_to_string(&moved[0]).unwrap(), "not json");
}

#[test]
fn malformed_and_oversized_requests_are_invalid() {
    let dir = TempDir::new();
    let sock = dir.path("h.sock");
    let mut child = ChildGuard(helper_command(&dir).spawn().unwrap());
    wait_until_serving(&mut child, &dir);

    for line in ["not json".to_string(), "x".repeat(64 * 1024 + 10)] {
        let mut stream = UnixStream::connect(&sock).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        // The helper stops reading at the cap, so the tail of an oversized
        // line may hit a closed socket; only the reply matters.
        let _ = stream.write_all(format!("{}\n", line).as_bytes());
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).unwrap();
        assert_eq!(reply, "{\"error\":\"invalid request\"}\n");
    }
    assert!(!dir.path("fake/calls.log").exists(), "nothing was run");
}

fn assert_refuses(mut cmd: Command, dir: &TempDir, expect_in_stderr: &str) {
    let mut child = ChildGuard(cmd.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "helper kept running");
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = stderr_of(dir);
    assert!(!status.success(), "helper started: {}", stderr);
    assert!(stderr.contains(expect_in_stderr), "stderr: {}", stderr);
    assert!(!dir.path("h.sock").exists(), "no socket may be created");
}

#[test]
fn debug_helper_refuses_to_start_without_each_env_var() {
    for var in ENV_VARS {
        let dir = TempDir::new();
        let mut cmd = helper_command(&dir);
        cmd.env_remove(var);
        assert_refuses(cmd, &dir, var);
    }
}

#[test]
fn debug_helper_refuses_relative_paths_and_the_real_networksetup() {
    let dir = TempDir::new();
    let mut cmd = helper_command(&dir);
    cmd.env("IHWIP_HELPER_SOCKET", "h.sock");
    assert_refuses(cmd, &dir, "absolute path");

    // Refused at start-up, before any socket exists or any request could run.
    let dir = TempDir::new();
    let mut cmd = helper_command(&dir);
    cmd.env("IHWIP_NETWORKSETUP", "/usr/sbin/networksetup");
    assert_refuses(cmd, &dir, "real networksetup");
}

#[test]
fn debug_helper_refuses_the_real_journal_folder() {
    // Refused from the path alone: nothing in that folder is opened or changed.
    let dir = TempDir::new();
    let mut cmd = helper_command(&dir);
    cmd.env(
        "IHWIP_HELPER_JOURNAL",
        "/Library/Application Support/com.ithurtswhenip.helper/journal.json",
    );
    assert_refuses(cmd, &dir, "real journal folder");
}

#[test]
fn debug_helper_leaves_a_non_socket_at_the_socket_path() {
    let dir = TempDir::new();
    fs::write(dir.path("h.sock"), "not a socket").unwrap();
    let mut child = ChildGuard(helper_command(&dir).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "helper kept running");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!status.success());
    let stderr = stderr_of(&dir);
    assert!(stderr.contains("regular file"), "{}", stderr);
    assert_eq!(
        fs::read_to_string(dir.path("h.sock")).unwrap(),
        "not a socket"
    );
}
