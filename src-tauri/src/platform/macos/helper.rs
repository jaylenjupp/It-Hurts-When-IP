// Client side of the privileged helper's socket protocol (v2), plus the
// helper status shown in the main window's banner.
//
// The wire format is defined in helper-tool/src/protocol.rs (its header
// comment is authoritative). Everything at the top of this file is pure:
// request building, reply parsing and the status classifier, unit-tested
// with canned replies.
//
// The only code that touches the socket is the `socket` module at the bottom,
// compiled into release builds only (`not(any(debug_assertions, test))`).
// Debug and test builds never open the helper socket (decision D4): the build
// host runs a real root helper.

// In debug (non-test) builds most of this file is only reached from the
// release-only code or the tests, so it would read as dead code there.
#![cfg_attr(all(debug_assertions, not(test)), allow(dead_code))]

use serde::Serialize;
use serde_json::{json, Map, Value};

/// Helper protocol version this app speaks.
pub const PROTOCOL_VERSION: u64 = 2;

/// Installed helper files. Checked with `Path::exists` only (read-only).
pub const HELPER_BINARY: &str = "/Library/PrivilegedHelperTools/com.ithurtswhenip.helper";
pub const HELPER_PLIST: &str = "/Library/LaunchDaemons/com.ithurtswhenip.helper.plist";

const REINSTALL: &str = "Reinstall It Hurts When IP from the latest .pkg.";

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    Ok,
    /// Helper binary or LaunchDaemon plist not installed.
    Missing,
    /// Files present but the socket is absent, refuses, or doesn't answer
    /// properly.
    NotRunning,
    /// Connecting failed with "permission denied" (not an admin account).
    NoPermission,
    /// A pre-v2 helper (answers "unknown command" to ping).
    OldHelper,
    /// Helper speaks v2 but its version or protocol differs from the app's.
    VersionMismatch,
    /// Debug builds only: the helper is never contacted.
    #[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
    DevBuild,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct HelperStatus {
    pub state: HelperState,
    /// Plain text for the banner (empty when `ok`).
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
}

impl HelperStatus {
    fn new(state: HelperState, message: impl Into<String>) -> Self {
        HelperStatus {
            state,
            message: message.into(),
            helper_version: None,
            git_sha: None,
        }
    }
}

/// How connecting to the helper socket went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    Connected,
    /// No socket file (helper not running).
    NoSocket,
    /// Socket file present but nobody listening.
    Refused,
    /// The socket exists but this account may not use it.
    PermissionDenied,
    /// Anything else (timeout, odd errno), with the error text.
    Failed(String),
}

/// Map a connect error to a `ConnectOutcome`.
pub fn connect_outcome(err: &std::io::Error) -> ConnectOutcome {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::NotFound => ConnectOutcome::NoSocket,
        ErrorKind::ConnectionRefused => ConnectOutcome::Refused,
        ErrorKind::PermissionDenied => ConnectOutcome::PermissionDenied,
        _ => ConnectOutcome::Failed(err.to_string()),
    }
}

// --- Messages ---

fn msg_missing() -> String {
    format!("The background helper is missing. {}", REINSTALL)
}
fn msg_not_running() -> String {
    format!("The background helper is not running. {}", REINSTALL)
}
fn msg_not_answering() -> String {
    format!(
        "The background helper isn't answering properly. {}",
        REINSTALL
    )
}
fn msg_old_helper() -> String {
    format!("The background helper is out of date. {}", REINSTALL)
}
fn msg_version_mismatch() -> String {
    format!(
        "The background helper is a different version from this app. {}",
        REINSTALL
    )
}
const MSG_NO_PERMISSION: &str = "This Mac account isn't an Administrator, so it can't change \
     network settings. Sign in as an admin user, or ask whoever manages this Mac.";

/// Map what the files check, the connect and the ping reply showed to a
/// status. Never returns `DevBuild`.
pub fn classify(
    files_present: bool,
    connect: ConnectOutcome,
    ping_reply: Option<&str>,
    app_version: &str,
) -> HelperStatus {
    if !files_present {
        return HelperStatus::new(HelperState::Missing, msg_missing());
    }
    match connect {
        ConnectOutcome::Connected => {}
        ConnectOutcome::NoSocket | ConnectOutcome::Refused | ConnectOutcome::Failed(_) => {
            return HelperStatus::new(HelperState::NotRunning, msg_not_running());
        }
        ConnectOutcome::PermissionDenied => {
            return HelperStatus::new(HelperState::NoPermission, MSG_NO_PERMISSION);
        }
    }
    let line = match ping_reply.map(str::trim) {
        Some(l) if !l.is_empty() => l,
        // Connected but no reply (timeout / closed): not answering.
        _ => return HelperStatus::new(HelperState::NotRunning, msg_not_answering()),
    };
    let reply: Value = match serde_json::from_str(line) {
        Ok(v @ Value::Object(_)) => v,
        _ => return HelperStatus::new(HelperState::NotRunning, msg_not_answering()),
    };

    if let Some(err) = reply.get("error").and_then(Value::as_str) {
        let has_v = reply.get("v").is_some();
        return if !has_v && err == "unknown command" {
            // v1.13.1 ignores "v" and doesn't know ping.
            HelperStatus::new(HelperState::OldHelper, msg_old_helper())
        } else if err == "unsupported_version" || has_v {
            // Doesn't speak v2, or a v2 helper refused ping.
            HelperStatus::new(HelperState::VersionMismatch, msg_version_mismatch())
        } else {
            HelperStatus::new(HelperState::NotRunning, msg_not_answering())
        };
    }

    let is_pong = reply.get("ok").and_then(Value::as_str) == Some("pong");
    let helper_version = reply.get("helper_version").and_then(Value::as_str);
    let protocol = reply.get("protocol").and_then(Value::as_u64);
    let (true, Some(helper_version), Some(protocol)) = (is_pong, helper_version, protocol) else {
        return HelperStatus::new(HelperState::NotRunning, msg_not_answering());
    };
    let git_sha = reply
        .get("git_sha")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut status = if protocol == PROTOCOL_VERSION && helper_version == app_version {
        HelperStatus::new(HelperState::Ok, "")
    } else {
        // Older or newer: we can't tell which side is out of date.
        HelperStatus::new(HelperState::VersionMismatch, msg_version_mismatch())
    };
    status.helper_version = Some(helper_version.to_string());
    status.git_sha = git_sha;
    status
}

/// Debug and test builds: the helper is never contacted. Release builds
/// cannot call this (it isn't compiled in).
#[cfg(any(debug_assertions, test))]
pub fn dev_build_status(fake_set: bool) -> HelperStatus {
    let message = if fake_set {
        "Development build: network changes go to the fake (IHWIP_NETWORKSETUP)."
    } else {
        "Development build: network changes are refused (IHWIP_NETWORKSETUP not set)."
    };
    HelperStatus::new(HelperState::DevBuild, message)
}

// --- Requests and replies ---

/// A v2 request: `{"v":2,"cmd":<cmd>, <fields>…}`.
pub fn v2_request(cmd: &str, fields: &[(&str, &str)]) -> Value {
    let mut obj = Map::new();
    obj.insert("v".to_string(), json!(PROTOCOL_VERSION));
    obj.insert("cmd".to_string(), json!(cmd));
    for (k, v) in fields {
        obj.insert((*k).to_string(), json!(v));
    }
    Value::Object(obj)
}

pub fn ping_request() -> Value {
    v2_request("ping", &[])
}

/// One reply line → Ok(text) or Err(full helper error text). Works for v1
/// (`{"ok"}` / `{"error"}`) and v2 (same plus `v` and `code`).
pub fn parse_reply(line: &str) -> Result<String, String> {
    let parsed: Value = serde_json::from_str(line.trim())
        .map_err(|e| format!("Invalid response from helper: {}", e))?;
    if let Some(ok) = parsed.get("ok").and_then(Value::as_str) {
        Ok(ok.to_string())
    } else if let Some(err) = parsed.get("error").and_then(Value::as_str) {
        Err(err.to_string())
    } else {
        Err("Unknown response from helper".to_string())
    }
}

// --- Socket (release builds only) ---

#[cfg(not(any(debug_assertions, test)))]
pub mod socket {
    use super::{connect_outcome, ping_request, ConnectOutcome};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const HELPER_SOCKET: &str = "/var/run/com.ithurtswhenip.helper.sock";
    const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

    /// Send one request, return the raw reply line.
    fn exchange(
        stream: &UnixStream,
        request: &serde_json::Value,
    ) -> Result<String, std::io::Error> {
        let mut writer = stream;
        writer.write_all(format!("{}\n", request).as_bytes())?;
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        Ok(response)
    }

    /// A write request. No timeout: a write may legitimately take a while.
    pub fn send(request: serde_json::Value) -> Result<String, String> {
        let stream = UnixStream::connect(HELPER_SOCKET)
            .map_err(|e| format!("Could not connect to helper: {}", e))?;
        let response =
            exchange(&stream, &request).map_err(|e| format!("Helper did not answer: {}", e))?;
        super::parse_reply(&response)
    }

    /// Connect and ping with a 2 s read/write timeout.
    pub fn probe() -> (ConnectOutcome, Option<String>) {
        query(&ping_request())
    }

    /// Connect and send one read-only request (ping, journal_status) with a
    /// 2 s read/write timeout. Never use this for writes (see `send`).
    pub fn query(request: &serde_json::Value) -> (ConnectOutcome, Option<String>) {
        let stream = match UnixStream::connect(HELPER_SOCKET) {
            Ok(s) => s,
            Err(e) => return (connect_outcome(&e), None),
        };
        let timeouts = stream
            .set_read_timeout(Some(PROBE_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(PROBE_TIMEOUT)));
        if timeouts.is_err() {
            return (ConnectOutcome::Connected, None);
        }
        (ConnectOutcome::Connected, exchange(&stream, request).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "1.13.1";
    const PONG: &str =
        r#"{"v":2,"ok":"pong","helper_version":"1.13.1","protocol":2,"git_sha":"abc1234"}"#;

    fn ping(reply: &str) -> HelperStatus {
        classify(true, ConnectOutcome::Connected, Some(reply), APP)
    }

    // --- classify: the seven states ---

    #[test]
    fn ok_when_pong_matches_app() {
        let s = ping(PONG);
        assert_eq!(s.state, HelperState::Ok);
        assert_eq!(s.message, "");
        assert_eq!(s.helper_version.as_deref(), Some("1.13.1"));
        assert_eq!(s.git_sha.as_deref(), Some("abc1234"));
        // Trailing newline as read from the socket.
        assert_eq!(ping(&format!("{PONG}\n")).state, HelperState::Ok);
    }

    #[test]
    fn missing_when_files_absent_whatever_else() {
        for connect in [
            ConnectOutcome::Connected,
            ConnectOutcome::NoSocket,
            ConnectOutcome::PermissionDenied,
        ] {
            let s = classify(false, connect, Some(PONG), APP);
            assert_eq!(s.state, HelperState::Missing);
            assert_eq!(
                s.message,
                "The background helper is missing. Reinstall It Hurts When IP from the latest .pkg."
            );
        }
    }

    #[test]
    fn not_running_when_socket_absent_refused_or_failed() {
        for connect in [
            ConnectOutcome::NoSocket,
            ConnectOutcome::Refused,
            ConnectOutcome::Failed("timed out".into()),
        ] {
            let s = classify(true, connect, None, APP);
            assert_eq!(s.state, HelperState::NotRunning);
            assert_eq!(
                s.message,
                "The background helper is not running. Reinstall It Hurts When IP from the latest .pkg."
            );
        }
    }

    #[test]
    fn no_permission_when_connect_denied() {
        let s = classify(true, ConnectOutcome::PermissionDenied, None, APP);
        assert_eq!(s.state, HelperState::NoPermission);
        assert_eq!(
            s.message,
            "This Mac account isn't an Administrator, so it can't change network settings. \
             Sign in as an admin user, or ask whoever manages this Mac."
        );
    }

    #[test]
    fn old_helper_when_ping_is_unknown_command_v1() {
        let s = ping(r#"{"error":"unknown command"}"#);
        assert_eq!(s.state, HelperState::OldHelper);
        assert_eq!(
            s.message,
            "The background helper is out of date. Reinstall It Hurts When IP from the latest .pkg."
        );
    }

    #[test]
    fn version_mismatch_for_older_newer_or_other_protocol() {
        let want = "The background helper is a different version from this app. \
                    Reinstall It Hurts When IP from the latest .pkg.";
        for reply in [
            r#"{"v":2,"ok":"pong","helper_version":"1.13.0","protocol":2,"git_sha":"x"}"#,
            r#"{"v":2,"ok":"pong","helper_version":"1.14.0","protocol":2,"git_sha":"x"}"#,
            r#"{"v":2,"ok":"pong","helper_version":"1.13.1","protocol":3,"git_sha":"x"}"#,
        ] {
            let s = ping(reply);
            assert_eq!(s.state, HelperState::VersionMismatch, "{reply}");
            assert_eq!(s.message, want);
            assert!(s.helper_version.is_some());
        }
        assert_eq!(
            ping(r#"{"error":"unsupported_version"}"#).state,
            HelperState::VersionMismatch
        );
        assert_eq!(
            ping(r#"{"v":2,"error":"unknown command","code":"unknown_command"}"#).state,
            HelperState::VersionMismatch
        );
    }

    #[test]
    fn dev_build_messages() {
        let s = dev_build_status(true);
        assert_eq!(s.state, HelperState::DevBuild);
        assert!(s.message.contains("go to the fake"), "{}", s.message);
        let s = dev_build_status(false);
        assert_eq!(s.state, HelperState::DevBuild);
        assert!(s.message.contains("refused"), "{}", s.message);
    }

    // --- classify: junk ---

    #[test]
    fn junk_or_no_reply_is_not_running_not_answering() {
        let want = "The background helper isn't answering properly. \
                    Reinstall It Hurts When IP from the latest .pkg.";
        let junk = [
            "",
            "   \n",
            "not json",
            "{\"ok\":",
            "[]",
            "\"pong\"",
            "{}",
            r#"{"ok":1}"#,
            r#"{"ok":"pong"}"#,
            r#"{"v":2,"ok":"pong","helper_version":1,"protocol":2}"#,
            r#"{"v":2,"ok":"pong","helper_version":"1.13.1","protocol":"2"}"#,
            r#"{"v":2,"ok":"done","helper_version":"1.13.1","protocol":2}"#,
            r#"{"error":"something else"}"#,
            r#"{"error":5}"#,
        ];
        for reply in junk {
            let s = ping(reply);
            assert_eq!(s.state, HelperState::NotRunning, "{reply:?}");
            assert_eq!(s.message, want, "{reply:?}");
        }
        let s = classify(true, ConnectOutcome::Connected, None, APP);
        assert_eq!(s.state, HelperState::NotRunning);
        assert_eq!(s.message, want);
    }

    #[test]
    fn classify_never_returns_dev_build() {
        let connects = [
            ConnectOutcome::Connected,
            ConnectOutcome::NoSocket,
            ConnectOutcome::Refused,
            ConnectOutcome::PermissionDenied,
            ConnectOutcome::Failed("x".into()),
        ];
        let replies = [
            None,
            Some(PONG),
            Some(r#"{"error":"unknown command"}"#),
            Some("junk"),
        ];
        for files in [true, false] {
            for c in &connects {
                for r in replies {
                    let s = classify(files, c.clone(), r, APP);
                    assert_ne!(s.state, HelperState::DevBuild);
                }
            }
        }
    }

    #[test]
    fn status_serializes_snake_case_and_skips_none() {
        let v = serde_json::to_value(classify(true, ConnectOutcome::NoSocket, None, APP)).unwrap();
        assert_eq!(v["state"], "not_running");
        assert!(v.get("helper_version").is_none());
        assert!(v.get("git_sha").is_none());

        let v = serde_json::to_value(ping(PONG)).unwrap();
        assert_eq!(
            v,
            json!({"state":"ok","message":"","helper_version":"1.13.1","git_sha":"abc1234"})
        );
        let v = serde_json::to_value(classify(true, ConnectOutcome::PermissionDenied, None, APP))
            .unwrap();
        assert_eq!(v["state"], "no_permission");
        assert_eq!(
            serde_json::to_value(ping(r#"{"error":"unknown command"}"#)).unwrap()["state"],
            "old_helper"
        );
        assert_eq!(
            serde_json::to_value(dev_build_status(true)).unwrap()["state"],
            "dev_build"
        );
    }

    #[test]
    fn helper_file_paths_match_the_installed_layout() {
        assert_eq!(
            HELPER_BINARY,
            "/Library/PrivilegedHelperTools/com.ithurtswhenip.helper"
        );
        assert_eq!(
            HELPER_PLIST,
            "/Library/LaunchDaemons/com.ithurtswhenip.helper.plist"
        );
    }

    // --- connect errors ---

    #[test]
    fn connect_errors_map_to_outcomes() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            connect_outcome(&Error::from(ErrorKind::NotFound)),
            ConnectOutcome::NoSocket
        );
        assert_eq!(
            connect_outcome(&Error::from(ErrorKind::ConnectionRefused)),
            ConnectOutcome::Refused
        );
        assert_eq!(
            connect_outcome(&Error::from(ErrorKind::PermissionDenied)),
            ConnectOutcome::PermissionDenied
        );
        // Raw errnos as the OS reports them (macOS: ENOENT 2, EACCES 13, ECONNREFUSED 61).
        assert_eq!(
            connect_outcome(&Error::from_raw_os_error(2)),
            ConnectOutcome::NoSocket
        );
        assert_eq!(
            connect_outcome(&Error::from_raw_os_error(13)),
            ConnectOutcome::PermissionDenied
        );
        assert_eq!(
            connect_outcome(&Error::from_raw_os_error(61)),
            ConnectOutcome::Refused
        );
        assert!(matches!(
            connect_outcome(&Error::from(ErrorKind::TimedOut)),
            ConnectOutcome::Failed(_)
        ));
    }

    // --- requests ---

    #[test]
    fn v2_request_shapes() {
        assert_eq!(ping_request(), json!({"v":2,"cmd":"ping"}));
        assert_eq!(
            v2_request("set_dhcp", &[("interface", "USB 10/100/1000 LAN")]),
            json!({"v":2,"cmd":"set_dhcp","interface":"USB 10/100/1000 LAN"})
        );
        assert_eq!(
            v2_request(
                "set_static_ip",
                &[
                    ("interface", "Dante Primary"),
                    ("ip", "192.168.10.50"),
                    ("subnet", "255.255.255.0"),
                    ("gateway", ""),
                ]
            ),
            json!({
                "v":2,
                "cmd":"set_static_ip",
                "interface":"Dante Primary",
                "ip":"192.168.10.50",
                "subnet":"255.255.255.0",
                "gateway":""
            })
        );
        // "v" is the integer 2 (the helper rejects "2" and 2.0).
        assert!(ping_request()["v"].is_u64());
    }

    // --- replies ---

    #[test]
    fn parse_reply_v1_and_v2() {
        assert_eq!(
            parse_reply(r#"{"ok":"Set Wi-Fi to DHCP"}"#),
            Ok("Set Wi-Fi to DHCP".to_string())
        );
        assert_eq!(
            parse_reply(r#"{"error":"bad thing"}"#),
            Err("bad thing".to_string())
        );
        assert_eq!(
            parse_reply("{\"v\":2,\"ok\":\"Set Wi-Fi to DHCP\"}\n"),
            Ok("Set Wi-Fi to DHCP".to_string())
        );
        let long = "Could not record the original settings of Wi-Fi, so nothing was changed. \
                    Reinstall It Hurts When IP from the latest .pkg; if that doesn't help, \
                    run the rescue script.";
        let line = json!({"v":2,"error":long,"code":"journal_failed"}).to_string();
        assert_eq!(parse_reply(&line), Err(long.to_string()));
    }

    #[test]
    fn parse_reply_ping_unknown_and_garbage() {
        assert_eq!(parse_reply(PONG), Ok("pong".to_string()));
        assert_eq!(
            parse_reply(r#"{"error":"unknown command"}"#),
            Err("unknown command".to_string())
        );
        assert_eq!(
            parse_reply(r#"{"error":"unsupported_version"}"#),
            Err("unsupported_version".to_string())
        );
        assert_eq!(
            parse_reply("{}"),
            Err("Unknown response from helper".to_string())
        );
        assert_eq!(
            parse_reply(r#"{"ok":1}"#),
            Err("Unknown response from helper".to_string())
        );
        for garbage in ["", "not json", "{\"ok\":"] {
            let err = parse_reply(garbage).unwrap_err();
            assert!(err.starts_with("Invalid response from helper"), "{err}");
        }
    }
}
