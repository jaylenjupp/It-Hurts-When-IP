// Socket protocol: one JSON object per line in, one per line out.
//
// v1 (no "v", or "v":1): set_static_ip {interface, ip, subnet, gateway} and
//   set_dhcp {interface}. Replies exactly as v1.13.1: {"ok":"<text>"} or
//   {"error":"<text>"}.
// v2 ("v":2): the same two commands plus ping, journal_status and
//   restore_service {interface}. Replies:
//   ok    {"v":2,"ok":"<text>"}
//   error {"v":2,"error":"<plain message>","code":"<stable code>"}
//   ping  {"v":2,"ok":"pong","helper_version":"…","protocol":2,"git_sha":"…"}
//   journal_status {"v":2,"ok":"journal","state":"none"|"ok"|"unusable",
//                   "reason":"…" (only when unusable),
//                   "services":[{"service":"…","recorded_at":"…"}] ([] unless ok)}
//   restore_service ok text "Restored <svc> to its original settings (…)." or
//                   "<svc> already matches its original settings (…).", maybe
//                   followed by " Warning: …" if the journal couldn't be
//                   updated; error codes include not_journaled,
//                   journal_unusable, unsupported_original, verify_failed.
// Before the first set_* to a service (v1 or v2) the helper records its
// original settings in the journal; if it can't, nothing is changed and the
// reply is an error (v2 code "journal_failed").
// Any other "v" → {"error":"unsupported_version"}, nothing is run.
// Malformed, non-object or oversized request → {"error":"invalid request"}.
// Unknown cmd → "unknown command" (v1 text, so old and new app/helper can
// recognise each other).

use crate::error::{ErrCode, HelperError};
use crate::journal::{self, Clock, State};
use crate::netsetup::{self, Write};
use crate::runner::CommandRunner;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub const PROTOCOL_VERSION: u64 = 2;
/// Longest request line accepted, in bytes (without the newline).
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Short git SHA of the build, set by build.rs ("unknown" if git wasn't there).
pub const GIT_SHA: &str = match option_env!("IHWIP_GIT_SHA") {
    Some(sha) => sha,
    None => "unknown",
};

/// What a request needs to run: the command runner, the networksetup path,
/// the change journal file and the clock for its timestamps.
pub struct Ctx<'a> {
    pub runner: &'a dyn CommandRunner,
    pub networksetup: &'a str,
    pub journal: &'a Path,
    pub clock: &'a dyn Clock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    V1,
    V2,
}

#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    TooLarge,
    NotUtf8,
}

/// Read one request line (without its newline), at most `MAX_REQUEST_BYTES`.
/// A line ended by EOF instead of a newline is accepted, as v1.13.1 did.
pub fn read_request(reader: impl Read) -> Result<String, ReadError> {
    let mut limited = BufReader::new(reader.take(MAX_REQUEST_BYTES as u64 + 1));
    let mut buf = Vec::new();
    limited.read_until(b'\n', &mut buf).map_err(ReadError::Io)?;
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.len() > MAX_REQUEST_BYTES {
        return Err(ReadError::TooLarge);
    }
    String::from_utf8(buf).map_err(|_| ReadError::NotUtf8)
}

pub fn invalid_request() -> String {
    json!({"error": "invalid request"}).to_string()
}

/// Handle one request line and return the reply line (without newline).
pub fn handle_line(line: &str, ctx: &Ctx) -> String {
    let request: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(_) => return invalid_request(),
    };
    if !request.is_object() {
        return invalid_request();
    }
    // Only the integers 1 and 2 are versions; 1.0, "2", null, -1… are not.
    let version = match request.get("v") {
        None => Version::V1,
        Some(v) => match v.as_u64() {
            Some(1) => Version::V1,
            Some(2) => Version::V2,
            _ => return json!({"error": "unsupported_version"}).to_string(),
        },
    };
    let cmd = match request.get("cmd").and_then(Value::as_str) {
        Some(c) => c,
        None => {
            return reply_err(
                version,
                &HelperError::new(ErrCode::MissingField, "missing cmd"),
            )
        }
    };
    match (version, cmd) {
        (_, "set_static_ip") => reply(version, set_static_ip(&request, ctx)),
        (_, "set_dhcp") => reply(version, set_dhcp(&request, ctx)),
        (Version::V2, "ping") => ping_reply(),
        (Version::V2, "journal_status") => journal_status(ctx),
        (Version::V2, "restore_service") => reply(version, restore_service(&request, ctx)),
        _ => reply_err(
            version,
            &HelperError::new(ErrCode::UnknownCommand, "unknown command"),
        ),
    }
}

fn set_static_ip(request: &Value, ctx: &Ctx) -> Result<String, HelperError> {
    let service = field(request, "interface")?;
    let ip = field(request, "ip")?;
    let subnet = field(request, "subnet")?;
    let gateway = field(request, "gateway")?;
    let write = Write::set_manual(service, ip, subnet, gateway)?;
    netsetup::apply(ctx, &write)
}

fn set_dhcp(request: &Value, ctx: &Ctx) -> Result<String, HelperError> {
    let service = field(request, "interface")?;
    let write = Write::set_dhcp(service)?;
    netsetup::apply(ctx, &write)
}

fn restore_service(request: &Value, ctx: &Ctx) -> Result<String, HelperError> {
    let service = field(request, "interface")?;
    netsetup::restore(ctx, service)
}

/// Read-only. No lock: the journal is only ever replaced by an atomic
/// rename, so this sees either the old or the new file.
fn journal_status(ctx: &Ctx) -> String {
    match journal::load(ctx.journal) {
        State::None => json!({
            "v": PROTOCOL_VERSION, "ok": "journal", "state": "none", "services": [],
        }),
        State::Ok(entries) => json!({
            "v": PROTOCOL_VERSION, "ok": "journal", "state": "ok",
            "services": entries
                .iter()
                .map(|e| json!({"service": e.service, "recorded_at": e.recorded_at}))
                .collect::<Vec<_>>(),
        }),
        State::Unusable(reason) => json!({
            "v": PROTOCOL_VERSION, "ok": "journal", "state": "unusable",
            "reason": reason, "services": [],
        }),
    }
    .to_string()
}

/// A required string field. Absent or null → missing; any other non-string
/// type → invalid. Never defaults to "".
fn field<'a>(request: &'a Value, name: &str) -> Result<&'a str, HelperError> {
    match request.get(name) {
        None | Some(Value::Null) => Err(HelperError::new(
            ErrCode::MissingField,
            format!("Missing field {:?}.", name),
        )),
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(HelperError::new(
            ErrCode::InvalidField,
            format!("Field {:?} must be a string.", name),
        )),
    }
}

fn reply(version: Version, result: Result<String, HelperError>) -> String {
    match result {
        Ok(text) => match version {
            Version::V1 => json!({"ok": text}).to_string(),
            Version::V2 => json!({"v": PROTOCOL_VERSION, "ok": text}).to_string(),
        },
        Err(e) => reply_err(version, &e),
    }
}

fn reply_err(version: Version, e: &HelperError) -> String {
    eprintln!("request refused: {} {:?}", e.code.as_str(), e.message);
    match version {
        Version::V1 => json!({"error": e.message}).to_string(),
        Version::V2 => json!({
            "v": PROTOCOL_VERSION,
            "error": e.message,
            "code": e.code.as_str(),
        })
        .to_string(),
    }
}

fn ping_reply() -> String {
    json!({
        "v": PROTOCOL_VERSION,
        "ok": "pong",
        "helper_version": HELPER_VERSION,
        "protocol": PROTOCOL_VERSION,
        "git_sha": GIT_SHA,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Entry;
    use crate::runner::FakeRunner;
    use crate::testutil::{FixedClock, TempDir};
    use std::fs;

    const NS: &str = "/fake/networksetup";
    const SERVICES: &str =
        include_str!("../../tools/fake-networksetup/fixtures/default/services.txt");
    const UNKNOWN_STDOUT: &str = include_str!(
        "../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.stdout.txt"
    );
    /// 2026-11-16T09:30:00Z
    const T0: u64 = 1_794_821_400;
    const GETINFO_DHCP: &str = "DHCP Configuration\nIP address: 10.20.30.40\n\
                                Subnet mask: 255.255.252.0\nRouter: 10.20.28.1\n";

    /// Send one request with the journal at `journal`.
    fn send_in(journal: &Path, fake: &FakeRunner, request: &Value) -> Value {
        let ctx = Ctx {
            runner: fake,
            networksetup: NS,
            journal,
            clock: &FixedClock(T0),
        };
        let reply = handle_line(&request.to_string(), &ctx);
        serde_json::from_str(&reply).expect("reply is JSON")
    }

    /// Send one request with a fresh, empty journal folder.
    fn send(fake: &FakeRunner, request: &Value) -> Value {
        let dir = TempDir::new();
        send_in(&dir.join("journal.json"), fake, request)
    }

    fn send_raw(fake: &FakeRunner, line: &str) -> Value {
        let dir = TempDir::new();
        let ctx = Ctx {
            runner: fake,
            networksetup: NS,
            journal: &dir.join("journal.json"),
            clock: &FixedClock(T0),
        };
        serde_json::from_str(&handle_line(line, &ctx)).expect("reply is JSON")
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn call(args: &[&str]) -> (String, Vec<String>) {
        (NS.to_string(), strings(args))
    }

    /// Queue the three reads that record a service's original settings.
    fn push_reads(fake: &FakeRunner) -> &FakeRunner {
        fake.push_ok(GETINFO_DHCP)
            .push_ok("There aren't any DNS Servers set.\n")
            .push_ok("There aren't any Search Domains set.\n")
    }

    /// The calls that record `svc` before its first change.
    fn reads(svc: &str) -> Vec<(String, Vec<String>)> {
        vec![
            call(&["-getinfo", svc]),
            call(&["-getdnsservers", svc]),
            call(&["-getsearchdomains", svc]),
        ]
    }

    /// list, the three recording reads, then `write`.
    fn first_write_calls(svc: &str, write: &[&str]) -> Vec<(String, Vec<String>)> {
        let mut calls = vec![call(&["-listallnetworkservices"])];
        calls.extend(reads(svc));
        calls.push(call(write));
        calls
    }

    fn static_req(v: Option<Value>, svc: &str, ip: &str, mask: &str, gw: &str) -> Value {
        let mut r = json!({
            "cmd": "set_static_ip",
            "interface": svc,
            "ip": ip,
            "subnet": mask,
            "gateway": gw,
        });
        if let Some(v) = v {
            r["v"] = v;
        }
        r
    }

    fn dhcp_req(v: Option<Value>, svc: &str) -> Value {
        let mut r = json!({"cmd": "set_dhcp", "interface": svc});
        if let Some(v) = v {
            r["v"] = v;
        }
        r
    }

    /// The fake may only have seen the read-only service list, never a -set*.
    fn assert_no_write(fake: &FakeRunner, what: &str) {
        for (_, args) in fake.calls() {
            assert!(
                !args.first().is_some_and(|a| a.starts_with("-set")),
                "{}: a write ran: {:?}",
                what,
                args
            );
            assert_eq!(args, strings(&["-listallnetworkservices"]), "{}", what);
        }
    }

    // --- valid requests: exact argv ---

    #[test]
    fn v1_set_static_ip_with_gateway() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        let reply = send(
            &fake,
            &static_req(
                None,
                "Dante Primary",
                "192.168.10.50",
                "255.255.255.0",
                "192.168.10.1",
            ),
        );
        assert_eq!(reply, json!({"ok": "Set Dante Primary to 192.168.10.50"}));
        assert_eq!(
            fake.calls(),
            first_write_calls(
                "Dante Primary",
                &[
                    "-setmanual",
                    "Dante Primary",
                    "192.168.10.50",
                    "255.255.255.0",
                    "192.168.10.1"
                ]
            )
        );
    }

    #[test]
    fn v1_set_static_ip_without_gateway_passes_empty_fourth_arg() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        let reply = send(
            &fake,
            &static_req(
                Some(json!(1)),
                "USB 10/100/1000 LAN",
                "10.0.0.5",
                "255.0.0.0",
                "",
            ),
        );
        assert_eq!(reply, json!({"ok": "Set USB 10/100/1000 LAN to 10.0.0.5"}));
        assert_eq!(
            fake.calls()[4],
            call(&[
                "-setmanual",
                "USB 10/100/1000 LAN",
                "10.0.0.5",
                "255.0.0.0",
                ""
            ])
        );
    }

    #[test]
    fn v1_set_dhcp() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        assert_eq!(
            send(&fake, &dhcp_req(None, "Wi-Fi")),
            json!({"ok": "Set Wi-Fi to DHCP"})
        );
        assert_eq!(
            fake.calls(),
            first_write_calls("Wi-Fi", &["-setdhcp", "Wi-Fi"])
        );
    }

    #[test]
    fn v2_set_static_ip_with_and_without_gateway() {
        for (gw, last) in [("10.20.0.1", "10.20.0.1"), ("", "")] {
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            push_reads(&fake).push_ok("");
            let reply = send(
                &fake,
                &static_req(Some(json!(2)), "Wi-Fi", "10.20.0.7", "255.255.0.0", gw),
            );
            assert_eq!(reply, json!({"v": 2, "ok": "Set Wi-Fi to 10.20.0.7"}));
            assert_eq!(
                fake.calls(),
                first_write_calls(
                    "Wi-Fi",
                    &["-setmanual", "Wi-Fi", "10.20.0.7", "255.255.0.0", last]
                )
            );
        }
    }

    #[test]
    fn v2_set_dhcp() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        assert_eq!(
            send(&fake, &dhcp_req(Some(json!(2)), "Thunderbolt Bridge")),
            json!({"v": 2, "ok": "Set Thunderbolt Bridge to DHCP"})
        );
        assert_eq!(fake.calls()[4], call(&["-setdhcp", "Thunderbolt Bridge"]));
    }

    #[test]
    fn odd_gateways_and_link_local_ip_are_accepted() {
        // P2-D3: 0.0.0.0 and out-of-subnet gateways must keep working.
        for (ip, gw) in [
            ("192.168.10.50", "0.0.0.0"),
            ("192.168.10.50", "10.99.0.1"),
            ("169.254.10.20", ""),
        ] {
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            push_reads(&fake).push_ok("");
            let reply = send(
                &fake,
                &static_req(Some(json!(2)), "Dante Primary", ip, "255.255.255.0", gw),
            );
            assert_eq!(reply["ok"], json!(format!("Set Dante Primary to {}", ip)));
            assert_eq!(
                fake.calls()[4],
                call(&["-setmanual", "Dante Primary", ip, "255.255.255.0", gw])
            );
        }
    }

    // --- ping ---

    #[test]
    fn v2_ping_reports_versions_and_runs_nothing() {
        let fake = FakeRunner::new();
        let reply = send(&fake, &json!({"v": 2, "cmd": "ping"}));
        assert_eq!(
            reply,
            json!({
                "v": 2,
                "ok": "pong",
                "helper_version": env!("CARGO_PKG_VERSION"),
                "protocol": 2,
                "git_sha": GIT_SHA,
            })
        );
        assert!(!GIT_SHA.is_empty());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn v1_ping_is_an_unknown_command() {
        let fake = FakeRunner::new();
        assert_eq!(
            send(&fake, &json!({"cmd": "ping"})),
            json!({"error": "unknown command"})
        );
        assert!(fake.calls().is_empty());
    }

    // --- journal ---

    fn entry(svc: &str, getinfo: &str, at: &str) -> Entry {
        Entry {
            service: svc.to_string(),
            recorded_at: at.to_string(),
            getinfo: getinfo.to_string(),
            dns: String::new(),
            search: String::new(),
        }
    }

    #[test]
    fn journal_status_none_ok_and_unusable() {
        let dir = TempDir::new();
        let journal = dir.join("journal.json");
        let fake = FakeRunner::new();
        let status = json!({"v": 2, "cmd": "journal_status"});

        assert_eq!(
            send_in(&journal, &fake, &status),
            json!({"v": 2, "ok": "journal", "state": "none", "services": []})
        );

        journal::write(
            &journal,
            &[
                entry("Wi-Fi", GETINFO_DHCP, "2026-11-01T10:00:00Z"),
                entry(
                    "Show \"A\", B",
                    "DHCP Configuration\n",
                    "2026-11-02T10:00:00Z",
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            send_in(&journal, &fake, &status),
            json!({"v": 2, "ok": "journal", "state": "ok", "services": [
                {"service": "Wi-Fi", "recorded_at": "2026-11-01T10:00:00Z"},
                {"service": "Show \"A\", B", "recorded_at": "2026-11-02T10:00:00Z"},
            ]})
        );

        fs::write(&journal, "{\"schema\":2,\"services\":[]}").unwrap();
        assert_eq!(
            send_in(&journal, &fake, &status),
            json!({"v": 2, "ok": "journal", "state": "unusable",
                   "reason": "unknown schema 2", "services": []})
        );
        // Read-only: nothing run, the unusable file left as it was.
        assert!(fake.calls().is_empty());
        assert_eq!(
            fs::read_to_string(&journal).unwrap(),
            "{\"schema\":2,\"services\":[]}"
        );
    }

    #[test]
    fn v1_has_no_journal_commands() {
        let fake = FakeRunner::new();
        for cmd in ["journal_status", "restore_service"] {
            for v in [None, Some(json!(1))] {
                let mut req = json!({"cmd": cmd, "interface": "Wi-Fi"});
                if let Some(v) = v {
                    req["v"] = v;
                }
                assert_eq!(send(&fake, &req), json!({"error": "unknown command"}));
            }
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn v1_and_v2_set_requests_now_journal() {
        let dir = TempDir::new();
        let journal = dir.join("journal.json");
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        assert_eq!(
            send_in(&journal, &fake, &dhcp_req(None, "Wi-Fi")),
            json!({"ok": "Set Wi-Fi to DHCP"})
        );
        fake.push_ok(SERVICES);
        push_reads(&fake).push_ok("");
        assert_eq!(
            send_in(
                &journal,
                &fake,
                &static_req(Some(json!(2)), "Dante Primary", "10.0.0.5", "255.0.0.0", "")
            ),
            json!({"v": 2, "ok": "Set Dante Primary to 10.0.0.5"})
        );
        match journal::load(&journal) {
            State::Ok(entries) => {
                let names: Vec<&str> = entries.iter().map(|e| e.service.as_str()).collect();
                assert_eq!(names, vec!["Wi-Fi", "Dante Primary"]);
                assert_eq!(entries[0].getinfo, GETINFO_DHCP);
                assert_eq!(entries[0].recorded_at, "2026-11-16T09:30:00Z");
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn journal_failed_reply_in_v1_and_v2() {
        let want = "Nothing was changed. The helper could not record this service's original \
                    settings (networksetup -getinfo failed: boom). Reinstall It Hurts When IP \
                    from the latest .pkg, then try again.";
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES).push(Some(1), "", "boom\n");
        assert_eq!(
            send(&fake, &dhcp_req(None, "Wi-Fi")),
            json!({"error": want})
        );
        assert_no_write_after_reads(&fake);

        let fake = FakeRunner::new();
        fake.push_ok(SERVICES).push(Some(1), "", "boom\n");
        assert_eq!(
            send(
                &fake,
                &static_req(Some(json!(2)), "Wi-Fi", "10.0.0.5", "255.0.0.0", "")
            ),
            json!({"v": 2, "error": want, "code": "journal_failed"})
        );
        assert_no_write_after_reads(&fake);
    }

    fn assert_no_write_after_reads(fake: &FakeRunner) {
        assert_eq!(
            fake.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Wi-Fi"])
            ]
        );
    }

    #[test]
    fn restore_service_replies() {
        let dir = TempDir::new();
        let journal = dir.join("journal.json");
        let fake = FakeRunner::new();
        journal::write(
            &journal,
            &[entry("Wi-Fi", GETINFO_DHCP, "2026-11-01T10:00:00Z")],
        )
        .unwrap();
        fake.push_ok(SERVICES)
            .push_ok("Manual Configuration\nIP address: 10.1.1.1\nSubnet mask: 255.0.0.0\n")
            .push_ok("")
            .push_ok(GETINFO_DHCP);
        let restore = json!({"v": 2, "cmd": "restore_service", "interface": "Wi-Fi"});
        assert_eq!(
            send_in(&journal, &fake, &restore),
            json!({"v": 2, "ok": "Restored Wi-Fi to its original settings (DHCP)."})
        );
        assert_eq!(
            fake.calls(),
            vec![
                call(&["-listallnetworkservices"]),
                call(&["-getinfo", "Wi-Fi"]),
                call(&["-setdhcp", "Wi-Fi"]),
                call(&["-getinfo", "Wi-Fi"]),
            ]
        );
        assert_eq!(
            send_in(&journal, &fake, &json!({"v": 2, "cmd": "journal_status"})),
            json!({"v": 2, "ok": "journal", "state": "ok", "services": []})
        );

        // Restoring it again: it is no longer journaled.
        let reply = send_in(&journal, &fake, &restore);
        assert_eq!(reply["code"], json!("not_journaled"));
        assert_eq!(reply["v"], json!(2));
        assert!(reply.get("ok").is_none());
        assert_eq!(fake.calls().len(), 4, "nothing more was run");

        // Field checks before anything runs.
        let reply = send_in(&journal, &fake, &json!({"v": 2, "cmd": "restore_service"}));
        assert_eq!(reply["code"], json!("missing_field"));
        let reply = send_in(
            &journal,
            &fake,
            &json!({"v": 2, "cmd": "restore_service", "interface": "Wi-Fi\nreboot"}),
        );
        assert_eq!(reply["code"], json!("invalid_service"));
        assert_eq!(fake.calls().len(), 4);
    }

    // --- hostile and invalid input: refused, no write ---

    #[test]
    fn hostile_set_static_ip_is_refused_without_a_write() {
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            // service, ip, mask, gateway, v2 code
            (
                "'; rm -rf /'",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "unknown_service",
            ),
            (
                "x'; rm -rf /'",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "unknown_service",
            ),
            (
                "Wi-Fi\nreboot",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "invalid_service",
            ),
            (
                "Wi-Fi\n",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "invalid_service",
            ),
            (
                "\"Wi-Fi\"",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "unknown_service",
            ),
            ("Wi-Fi'", "10.0.0.1", "255.255.255.0", "", "unknown_service"),
            ("", "10.0.0.1", "255.255.255.0", "", "invalid_service"),
            ("Nope", "10.0.0.1", "255.255.255.0", "", "unknown_service"),
            ("Wi-Fi ", "10.0.0.1", "255.255.255.0", "", "unknown_service"),
            (
                "*Old Adapter",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "unknown_service",
            ),
            (
                "Old Adapter",
                "10.0.0.1",
                "255.255.255.0",
                "",
                "service_disabled",
            ),
            ("Wi-Fi", "1.2.3", "255.255.255.0", "", "invalid_ip"),
            ("Wi-Fi", "256.1.1.1", "255.255.255.0", "", "invalid_ip"),
            ("Wi-Fi", " 10.0.0.1", "255.255.255.0", "", "invalid_ip"),
            (
                "Wi-Fi",
                "10.0.0.1; reboot",
                "255.255.255.0",
                "",
                "invalid_ip",
            ),
            ("Wi-Fi", "127.0.0.1", "255.0.0.0", "", "invalid_ip"),
            ("Wi-Fi", "0.0.0.0", "255.255.255.0", "", "invalid_ip"),
            ("Wi-Fi", "10.0.0.1", "255.0.255.0", "", "invalid_subnet"),
            ("Wi-Fi", "10.0.0.1", "0.0.0.0", "", "invalid_subnet"),
            (
                "Wi-Fi",
                "10.0.0.1",
                "255.255.255.0",
                "10.0.0.254; reboot",
                "invalid_gateway",
            ),
            (
                "Wi-Fi",
                "10.0.0.1",
                "255.255.255.0",
                "1.2.3",
                "invalid_gateway",
            ),
            ("Wi-Fi", "10.0.0.1", "255.255.255.0", " ", "invalid_gateway"),
        ];
        for &(svc, ip, mask, gw, code) in cases {
            let what = format!("{:?} {:?} {:?} {:?}", svc, ip, mask, gw);

            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            let reply = send(&fake, &static_req(Some(json!(2)), svc, ip, mask, gw));
            assert_eq!(reply["code"], json!(code), "{}", what);
            assert_eq!(reply["v"], json!(2), "{}", what);
            assert!(reply["error"].is_string(), "{}", what);
            assert!(reply.get("ok").is_none(), "{}", what);
            assert_no_write(&fake, &what);

            // v1 refuses the same input, in the v1 reply shape.
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            let reply = send(&fake, &static_req(None, svc, ip, mask, gw));
            let obj = reply.as_object().unwrap();
            assert_eq!(obj.len(), 1, "{}: v1 reply has only \"error\"", what);
            assert!(reply["error"].is_string(), "{}", what);
            assert_no_write(&fake, &what);
        }
    }

    #[test]
    fn hostile_set_dhcp_is_refused_without_a_write() {
        for (svc, code) in [
            ("'; rm -rf /'", "unknown_service"),
            ("x'; rm -rf /'", "unknown_service"),
            ("Wi-Fi\nreboot", "invalid_service"),
            ("", "invalid_service"),
            ("Nope", "unknown_service"),
            ("Old Adapter", "service_disabled"),
        ] {
            for v in [None, Some(json!(2))] {
                let fake = FakeRunner::new();
                fake.push_ok(SERVICES);
                let reply = send(&fake, &dhcp_req(v.clone(), svc));
                assert!(reply["error"].is_string(), "{:?}", svc);
                if v.is_some() {
                    assert_eq!(reply["code"], json!(code), "{:?}", svc);
                }
                assert_no_write(&fake, svc);
            }
        }
    }

    #[test]
    fn disabled_service_gets_a_plain_message() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        let reply = send(&fake, &dhcp_req(None, "Old Adapter"));
        let msg = reply["error"].as_str().unwrap();
        assert!(msg.contains("\"Old Adapter\" is disabled"), "{}", msg);
    }

    #[test]
    fn shape_errors_are_refused_before_networksetup_runs() {
        let fake = FakeRunner::new();
        send(
            &fake,
            &static_req(Some(json!(2)), "Wi-Fi", "256.1.1.1", "255.255.255.0", ""),
        );
        send(&fake, &dhcp_req(None, "bad\nname"));
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn missing_and_non_string_fields_are_errors() {
        for name in ["interface", "ip", "subnet", "gateway"] {
            let mut req = static_req(Some(json!(2)), "Wi-Fi", "10.0.0.1", "255.255.255.0", "");
            req.as_object_mut().unwrap().remove(name);
            let fake = FakeRunner::new();
            let reply = send(&fake, &req);
            assert_eq!(reply["code"], json!("missing_field"), "{}", name);
            assert!(fake.calls().is_empty(), "{}", name);

            for bad in [Value::Null, json!(5), json!(["10.0.0.1"]), json!(true)] {
                let mut req = static_req(Some(json!(2)), "Wi-Fi", "10.0.0.1", "255.255.255.0", "");
                req[name] = bad.clone();
                let fake = FakeRunner::new();
                let reply = send(&fake, &req);
                let want = if bad.is_null() {
                    "missing_field"
                } else {
                    "invalid_field"
                };
                assert_eq!(reply["code"], json!(want), "{} = {}", name, bad);
                assert!(fake.calls().is_empty());
            }
        }

        let fake = FakeRunner::new();
        let reply = send(&fake, &json!({"cmd": "set_dhcp"}));
        assert_eq!(reply, json!({"error": "Missing field \"interface\"."}));
        assert!(fake.calls().is_empty());
    }

    // --- networksetup failures ---

    #[test]
    fn stdout_only_write_error_is_a_failure_in_v1_and_v2() {
        for code in [Some(0), Some(4)] {
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            push_reads(&fake).push(code, UNKNOWN_STDOUT, "");
            let reply = send(&fake, &dhcp_req(None, "Wi-Fi"));
            let msg = reply["error"].as_str().expect("v1 error");
            assert!(
                msg.contains("** Error: The parameters were not valid."),
                "{}",
                msg
            );
            assert!(reply.get("ok").is_none());

            let fake = FakeRunner::new();
            fake.push_ok(SERVICES);
            push_reads(&fake).push(code, UNKNOWN_STDOUT, "");
            let reply = send(
                &fake,
                &static_req(Some(json!(2)), "Wi-Fi", "10.0.0.1", "255.255.255.0", ""),
            );
            assert_eq!(reply["code"], json!("command_failed"));
            assert!(reply["error"]
                .as_str()
                .unwrap()
                .contains("not a recognized network service"));
        }
    }

    #[test]
    fn exit_code_without_output_falls_back_to_the_code() {
        let fake = FakeRunner::new();
        fake.push_ok(SERVICES);
        push_reads(&fake).push(Some(3), "", "");
        assert_eq!(
            send(&fake, &dhcp_req(None, "Wi-Fi")),
            json!({"error": "networksetup exited with code 3"})
        );
    }

    // --- versions, framing, unknown commands ---

    #[test]
    fn unsupported_versions_run_nothing() {
        for v in [
            json!(3),
            json!("2"),
            json!("1"),
            json!(-1),
            json!(0),
            Value::Null,
            json!(1.0),
            json!(2.0),
            json!(true),
            json!([2]),
            json!({"v": 2}),
        ] {
            let fake = FakeRunner::new();
            fake.push_ok(SERVICES).push_ok("");
            let reply = send(
                &fake,
                &static_req(Some(v.clone()), "Wi-Fi", "10.0.0.1", "255.255.255.0", ""),
            );
            assert_eq!(reply, json!({"error": "unsupported_version"}), "v = {}", v);
            assert!(fake.calls().is_empty(), "v = {}", v);
        }
    }

    #[test]
    fn unknown_and_missing_commands() {
        let fake = FakeRunner::new();
        assert_eq!(
            send(
                &fake,
                &json!({"cmd": "restore_service", "interface": "Wi-Fi"})
            ),
            json!({"error": "unknown command"})
        );
        assert_eq!(
            send(&fake, &json!({"v": 2, "cmd": "rm"})),
            json!({"v": 2, "error": "unknown command", "code": "unknown_command"})
        );
        assert_eq!(
            send(&fake, &json!({"interface": "Wi-Fi"})),
            json!({"error": "missing cmd"})
        );
        assert_eq!(
            send(&fake, &json!({"v": 2, "cmd": 7})),
            json!({"v": 2, "error": "missing cmd", "code": "missing_field"})
        );
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn malformed_and_non_object_requests_are_invalid() {
        let fake = FakeRunner::new();
        for line in [
            "",
            "{",
            "not json",
            "[1]",
            "\"set_dhcp\"",
            "42",
            "null",
            "{\"cmd\":\"set_dhcp\"}x",
        ] {
            assert_eq!(
                send_raw(&fake, line),
                json!({"error": "invalid request"}),
                "{:?}",
                line
            );
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn read_request_takes_one_line_up_to_the_cap() {
        let r = read_request(&b"{\"cmd\":\"ping\"}\nrest"[..]).unwrap();
        assert_eq!(r, "{\"cmd\":\"ping\"}");
        // EOF without newline is accepted.
        assert_eq!(read_request(&b"abc"[..]).unwrap(), "abc");
        assert_eq!(read_request(&b""[..]).unwrap(), "");

        let max = "a".repeat(MAX_REQUEST_BYTES);
        assert_eq!(read_request(format!("{}\n", max).as_bytes()).unwrap(), max);
        assert_eq!(read_request(max.as_bytes()).unwrap(), max);

        let over = "a".repeat(MAX_REQUEST_BYTES + 1);
        assert!(matches!(
            read_request(over.as_bytes()),
            Err(ReadError::TooLarge)
        ));
        assert!(matches!(
            read_request(format!("{}\n", over).as_bytes()),
            Err(ReadError::TooLarge)
        ));
        assert!(matches!(
            read_request(&b"\xff\xfe\n"[..]),
            Err(ReadError::NotUtf8)
        ));
    }
}
