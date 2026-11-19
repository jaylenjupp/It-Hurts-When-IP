// Read-only diagnostics for Settings → Maintenance → "Copy diagnostics…".
//
// Everything here only reads: `sw_vers`, `uname -m`, `networksetup
// -listallnetworkservices` and `-getinfo <service>`, each through a
// `CommandRunner` with an argv array (never a shell). The helper is asked for
// its journal status (v2 `journal_status`) in release builds only, with the
// same 2 s timeout as the status ping; debug and test builds never contact it.
//
// The result is a struct (for a later detail view) plus a plain-text
// rendering (`text`) that the UI shows and copies. A failed read never fails
// the whole report: it shows "unknown" or says what couldn't be read.

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use super::helper::{HelperState, HelperStatus};
use super::parse;
use crate::build_info::{self, BuildInfo};
use crate::platform::runner::{CommandRunner, SystemRunner};

const SW_VERS: &str = "/usr/bin/sw_vers";
const UNAME: &str = "/usr/bin/uname";
const UNKNOWN: &str = "unknown";

// --- Model ---

/// Config facts `main.rs` passes in (the app data dir is Tauri's business).
pub struct DiagnosticsConfig {
    pub config_path: PathBuf,
    pub advanced_mode: bool,
    pub check_updates_on_launch: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    pub generated_utc: String,
    pub app: AppInfo,
    pub system: SystemInfo,
    pub helper: HelperStatus,
    pub journal: JournalInfo,
    pub network_services: ServicesInfo,
    pub selected_service: SelectedInfo,
    pub settings: SettingsInfo,
    /// Plain-text rendering of all of the above (what "Copy" copies).
    pub text: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub version: String,
    pub git_sha: String,
    pub dirty: bool,
    pub built_utc: String,
    /// "debug" or "release".
    pub build_type: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub macos_version: String,
    pub macos_build: String,
    pub arch: String,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    /// Debug/test build: the helper is never contacted.
    DevBuild,
    /// The helper isn't usable (missing, not running, no permission, old).
    NotAvailable,
    /// The helper answered: nothing recorded yet.
    None,
    Ok,
    Unusable,
    /// Asked, but no usable answer (timeout, error reply, junk).
    Unreadable,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub service: String,
    pub recorded_at: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct JournalInfo {
    pub state: JournalState,
    /// Unusable: the helper's reason. NotAvailable: the helper state.
    /// Unreadable: what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub services: Vec<JournalEntry>,
}

impl JournalInfo {
    fn new(state: JournalState, detail: Option<String>) -> Self {
        JournalInfo {
            state,
            detail,
            services: vec![],
        }
    }

    fn unreadable(detail: impl Into<String>) -> Self {
        JournalInfo::new(JournalState::Unreadable, Some(detail.into()))
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub enabled: bool,
    /// Not in the app's interface list (disabled, or a VPN/tunnel-like name).
    pub hidden_in_app: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ServicesInfo {
    /// False if `-listallnetworkservices` failed (then `services` is empty).
    pub read_ok: bool,
    pub services: Vec<ServiceInfo>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectedState {
    NoneSelected,
    NotInList,
    Read,
    ReadFailed,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SelectedInfo {
    pub state: SelectedState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// "DHCP" or "Static" (same words as the main window).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
}

impl SelectedInfo {
    fn bare(state: SelectedState, name: Option<&str>) -> Self {
        SelectedInfo {
            state,
            name: name.map(str::to_string),
            mode: None,
            ip: None,
            subnet: None,
            gateway: None,
        }
    }
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SettingsInfo {
    pub config_folder: String,
    pub config_json_exists: bool,
    pub advanced_mode: bool,
    pub check_updates_on_launch: bool,
}

impl SettingsInfo {
    fn from_config(config: &DiagnosticsConfig) -> Self {
        let folder = config
            .config_path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| UNKNOWN.to_string());
        SettingsInfo {
            config_folder: folder,
            config_json_exists: config.config_path.is_file(),
            advanced_mode: config.advanced_mode,
            check_updates_on_launch: config.check_updates_on_launch,
        }
    }
}

/// Everything that doesn't come from the runner.
struct Context {
    generated_utc: String,
    app: AppInfo,
    helper: HelperStatus,
    journal: JournalInfo,
    settings: SettingsInfo,
}

// --- Public entry point ---

/// Collects the report. Never fails; blocking (run it off the UI thread).
pub fn get_diagnostics(selected_service: Option<String>, config: DiagnosticsConfig) -> Diagnostics {
    let override_path = super::networksetup_override();
    let program = super::networksetup_program(override_path.as_deref());
    let helper = super::get_helper_status();
    let journal = fetch_journal(&helper);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ctx = Context {
        generated_utc: build_info::utc::format_utc(now),
        app: app_info(&build_info::build_info(), cfg!(debug_assertions)),
        helper,
        journal,
        settings: SettingsInfo::from_config(&config),
    };
    collect_with(&SystemRunner, program, ctx, selected_service.as_deref())
}

fn app_info(build: &BuildInfo, is_debug: bool) -> AppInfo {
    AppInfo {
        version: build.version.clone(),
        git_sha: build.git_sha.clone(),
        dirty: build.dirty,
        built_utc: build.built_utc.clone(),
        build_type: if is_debug { "debug" } else { "release" }.to_string(),
    }
}

// --- Journal ---

/// Debug and test builds never contact the helper.
fn fetch_journal(status: &HelperStatus) -> JournalInfo {
    if cfg!(any(debug_assertions, test)) {
        return JournalInfo::new(JournalState::DevBuild, None);
    }
    journal_for_status(status, query_journal)
}

/// Release builds: one `journal_status` request, 2 s read/write timeout.
#[cfg(not(any(debug_assertions, test)))]
fn query_journal() -> Option<String> {
    super::helper::socket::query(&super::helper::v2_request("journal_status", &[])).1
}

/// Debug and test builds contain no socket code.
#[cfg(any(debug_assertions, test))]
fn query_journal() -> Option<String> {
    None
}

/// Only asks the helper when the status check says it answers v2.
fn journal_for_status(
    status: &HelperStatus,
    query: impl FnOnce() -> Option<String>,
) -> JournalInfo {
    match status.state {
        HelperState::Ok | HelperState::VersionMismatch => match query() {
            Some(line) => parse_journal_reply(&line),
            None => JournalInfo::unreadable("no answer from the helper"),
        },
        HelperState::DevBuild => JournalInfo::new(JournalState::DevBuild, None),
        other => JournalInfo::new(JournalState::NotAvailable, Some(label(&other))),
    }
}

/// `{"v":2,"ok":"journal","state":none|ok|unusable,["reason"],"services":[…]}`
fn parse_journal_reply(line: &str) -> JournalInfo {
    let reply: Value = match serde_json::from_str(line.trim()) {
        Ok(v @ Value::Object(_)) => v,
        _ => return JournalInfo::unreadable("invalid reply from the helper"),
    };
    if let Some(err) = reply.get("error").and_then(Value::as_str) {
        return JournalInfo::unreadable(format!("helper error: {}", err));
    }
    if reply.get("ok").and_then(Value::as_str) != Some("journal") {
        return JournalInfo::unreadable("unexpected reply from the helper");
    }
    match reply.get("state").and_then(Value::as_str) {
        Some("none") => JournalInfo::new(JournalState::None, None),
        Some("unusable") => {
            let reason = reply
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            JournalInfo::new(JournalState::Unusable, Some(reason.to_string()))
        }
        Some("ok") => {
            let services = reply
                .get("services")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|e| {
                            let service = e.get("service")?.as_str()?.to_string();
                            let recorded_at = e
                                .get("recorded_at")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .unwrap_or(UNKNOWN)
                                .to_string();
                            Some(JournalEntry {
                                service,
                                recorded_at,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            JournalInfo {
                state: JournalState::Ok,
                detail: None,
                services,
            }
        }
        _ => JournalInfo::unreadable("unknown journal state from the helper"),
    }
}

// --- Collection (all runner calls) ---

/// First line of stdout if the command exited 0 with non-empty output,
/// else "unknown".
fn first_line_or_unknown(runner: &dyn CommandRunner, program: &str, args: &[&str]) -> String {
    match runner.run(program, args) {
        Ok(out) if out.code == Some(0) => out
            .stdout
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| UNKNOWN.to_string()),
        _ => UNKNOWN.to_string(),
    }
}

fn read_system(runner: &dyn CommandRunner) -> SystemInfo {
    SystemInfo {
        macos_version: first_line_or_unknown(runner, SW_VERS, &["-productVersion"]),
        macos_build: first_line_or_unknown(runner, SW_VERS, &["-buildVersion"]),
        arch: first_line_or_unknown(runner, UNAME, &["-m"]),
    }
}

fn read_services(runner: &dyn CommandRunner, program: &str) -> ServicesInfo {
    match runner.run(program, &["-listallnetworkservices"]) {
        Ok(out) if !super::command_failed(&out) => ServicesInfo {
            read_ok: true,
            services: parse::parse_service_list(&out.stdout)
                .into_iter()
                .map(|s| ServiceInfo {
                    // Same rule as get_interfaces: shown only if enabled and
                    // not VPN/tunnel-like.
                    hidden_in_app: !s.enabled || super::is_hidden_service(&s.name),
                    enabled: s.enabled,
                    name: s.name,
                })
                .collect(),
        },
        _ => ServicesInfo {
            read_ok: false,
            services: vec![],
        },
    }
}

fn read_selected(
    runner: &dyn CommandRunner,
    program: &str,
    services: &ServicesInfo,
    selected: Option<&str>,
) -> SelectedInfo {
    let name = match selected {
        Some(n) if !n.is_empty() => n,
        _ => return SelectedInfo::bare(SelectedState::NoneSelected, None),
    };
    if !services.services.iter().any(|s| s.name == name) {
        return SelectedInfo::bare(SelectedState::NotInList, Some(name));
    }
    match runner.run(program, &["-getinfo", name]) {
        Ok(out) if !super::command_failed(&out) => {
            let info = parse::parse_ip_info(&out.stdout);
            SelectedInfo {
                state: SelectedState::Read,
                name: Some(name.to_string()),
                mode: Some(if info.is_dhcp { "DHCP" } else { "Static" }.to_string()),
                ip: Some(info.ip),
                subnet: Some(info.subnet),
                gateway: Some(info.gateway),
            }
        }
        _ => SelectedInfo::bare(SelectedState::ReadFailed, Some(name)),
    }
}

/// Runs the read-only commands in this order: sw_vers -productVersion,
/// sw_vers -buildVersion, uname -m, -listallnetworkservices, then -getinfo
/// for the selected service if it is in the list.
fn collect_with(
    runner: &dyn CommandRunner,
    program: &str,
    ctx: Context,
    selected: Option<&str>,
) -> Diagnostics {
    let system = read_system(runner);
    let network_services = read_services(runner, program);
    let selected_service = read_selected(runner, program, &network_services, selected);
    let mut d = Diagnostics {
        generated_utc: ctx.generated_utc,
        app: ctx.app,
        system,
        helper: ctx.helper,
        journal: ctx.journal,
        network_services,
        selected_service,
        settings: ctx.settings,
        text: String::new(),
    };
    d.text = render_text(&d);
    d
}

// --- Text ---

/// The serde (snake_case) name of a unit enum value, e.g. "not_running".
fn label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| UNKNOWN.to_string())
}

/// One value per line: control characters (newlines in a service name, …)
/// become spaces.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

fn or_none(s: &str) -> String {
    if s.trim().is_empty() {
        "(none)".to_string()
    } else {
        clean(s)
    }
}

fn render_text(d: &Diagnostics) -> String {
    let mut lines: Vec<String> = Vec::new();
    let kv = |lines: &mut Vec<String>, k: &str, v: &str| lines.push(format!("{}: {}", k, v));

    lines.push("It Hurts When IP diagnostics".to_string());
    kv(&mut lines, "Generated (UTC)", &clean(&d.generated_utc));

    lines.push(String::new());
    lines.push("App".to_string());
    kv(&mut lines, "Version", &clean(&d.app.version));
    kv(&mut lines, "Git SHA", &clean(&d.app.git_sha));
    kv(
        &mut lines,
        "Modified",
        if d.app.dirty { "yes" } else { "no" },
    );
    kv(&mut lines, "Built (UTC)", &clean(&d.app.built_utc));
    kv(&mut lines, "Build type", &d.app.build_type);

    lines.push(String::new());
    lines.push("System".to_string());
    kv(&mut lines, "macOS version", &clean(&d.system.macos_version));
    kv(&mut lines, "macOS build", &clean(&d.system.macos_build));
    kv(&mut lines, "Architecture", &clean(&d.system.arch));

    lines.push(String::new());
    lines.push("Helper".to_string());
    kv(&mut lines, "State", &label(&d.helper.state));
    kv(&mut lines, "Message", &or_none(&d.helper.message));
    if let Some(v) = &d.helper.helper_version {
        kv(&mut lines, "Helper version", &clean(v));
    }
    if let Some(v) = &d.helper.git_sha {
        kv(&mut lines, "Helper git SHA", &clean(v));
    }

    lines.push(String::new());
    lines.push("Journal".to_string());
    let detail = d.journal.detail.as_deref().map(clean).unwrap_or_default();
    match d.journal.state {
        JournalState::DevBuild => kv(&mut lines, "State", "dev build: helper not contacted"),
        JournalState::NotAvailable => {
            kv(&mut lines, "State", &format!("not available ({})", detail))
        }
        JournalState::None => kv(&mut lines, "State", "none (nothing recorded yet)"),
        JournalState::Unusable => {
            kv(&mut lines, "State", "unusable");
            kv(&mut lines, "Reason", &detail);
        }
        JournalState::Unreadable => {
            kv(&mut lines, "State", &format!("could not read ({})", detail))
        }
        JournalState::Ok => {
            kv(&mut lines, "State", "ok");
            kv(
                &mut lines,
                "Journaled services",
                &d.journal.services.len().to_string(),
            );
            for e in &d.journal.services {
                kv(
                    &mut lines,
                    "Journaled",
                    &format!("{} (recorded {})", clean(&e.service), clean(&e.recorded_at)),
                );
            }
        }
    }

    lines.push(String::new());
    lines.push("Network services".to_string());
    if d.network_services.read_ok {
        kv(
            &mut lines,
            "Count",
            &d.network_services.services.len().to_string(),
        );
        for s in &d.network_services.services {
            let mut marks = vec![if s.enabled { "enabled" } else { "disabled" }];
            if s.hidden_in_app {
                marks.push("hidden in app");
            }
            kv(
                &mut lines,
                "Service",
                &format!("{} [{}]", clean(&s.name), marks.join(", ")),
            );
        }
    } else {
        kv(
            &mut lines,
            "Count",
            "unknown (could not read the service list)",
        );
    }

    lines.push(String::new());
    lines.push("Selected service".to_string());
    let sel = &d.selected_service;
    let sel_name = sel.name.as_deref().map(clean).unwrap_or_default();
    match sel.state {
        SelectedState::NoneSelected => kv(&mut lines, "Name", "none selected"),
        SelectedState::NotInList => {
            kv(&mut lines, "Name", &sel_name);
            kv(&mut lines, "Status", "not in the service list");
        }
        SelectedState::ReadFailed => {
            kv(&mut lines, "Name", &sel_name);
            kv(&mut lines, "Status", "could not read its settings");
        }
        SelectedState::Read => {
            kv(&mut lines, "Name", &sel_name);
            kv(
                &mut lines,
                "Mode",
                &or_none(sel.mode.as_deref().unwrap_or("")),
            );
            kv(
                &mut lines,
                "IP address",
                &or_none(sel.ip.as_deref().unwrap_or("")),
            );
            kv(
                &mut lines,
                "Subnet mask",
                &or_none(sel.subnet.as_deref().unwrap_or("")),
            );
            kv(
                &mut lines,
                "Router",
                &or_none(sel.gateway.as_deref().unwrap_or("")),
            );
        }
    }

    lines.push(String::new());
    lines.push("Settings".to_string());
    kv(
        &mut lines,
        "Config folder",
        &clean(&d.settings.config_folder),
    );
    kv(
        &mut lines,
        "config.json",
        if d.settings.config_json_exists {
            "exists"
        } else {
            "missing"
        },
    );
    kv(
        &mut lines,
        "Advanced mode",
        on_off(d.settings.advanced_mode),
    );
    kv(
        &mut lines,
        "Check for updates on launch",
        on_off(d.settings.check_updates_on_launch),
    );

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    // FakeRunner only; test builds have no socket code and fetch_journal
    // never asks the helper.
    use super::*;
    use crate::platform::runner::FakeRunner;

    const FAKE: &str = "/abs/path/to/fake-networksetup";
    const FIX_DEFAULT_SERVICES: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/services.txt");
    const FIX_DEFAULT_WIFI: &str =
        include_str!("../../../../tools/fake-networksetup/fixtures/default/info/Wi-Fi.txt");

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn status(state: HelperState, message: &str) -> HelperStatus {
        HelperStatus {
            state,
            message: message.to_string(),
            helper_version: None,
            git_sha: None,
        }
    }

    fn ok_helper() -> HelperStatus {
        HelperStatus {
            state: HelperState::Ok,
            message: String::new(),
            helper_version: Some("1.13.1".to_string()),
            git_sha: Some("abc1234".to_string()),
        }
    }

    fn ctx(helper: HelperStatus, journal: JournalInfo) -> Context {
        Context {
            generated_utc: "2026-11-19T14:03:00Z".to_string(),
            app: app_info(
                &BuildInfo {
                    version: "1.13.1".to_string(),
                    git_sha: "abc1234".to_string(),
                    dirty: false,
                    built_utc: "2026-11-15T10:20:30Z".to_string(),
                },
                false,
            ),
            helper,
            journal,
            settings: SettingsInfo {
                config_folder: "/Users/test/Library/Application Support/com.ithurtswhenip.app"
                    .to_string(),
                config_json_exists: true,
                advanced_mode: false,
                check_updates_on_launch: true,
            },
        }
    }

    /// sw_vers ×2 and uname answered normally.
    fn push_system(fake: &FakeRunner) {
        fake.push_ok("26.7.1\n")
            .push_ok("25G241\n")
            .push_ok("arm64\n");
    }

    fn has_line(text: &str, line: &str) -> bool {
        text.lines().any(|l| l == line)
    }

    fn assert_lines(text: &str, want: &[&str]) {
        for line in want {
            assert!(has_line(text, line), "missing line {line:?} in:\n{text}");
        }
    }

    fn journal_ok_two() -> JournalInfo {
        parse_journal_reply(
            r#"{"v":2,"ok":"journal","state":"ok","services":[
                {"service":"Wi-Fi","recorded_at":"2026-11-01T10:00:00Z"},
                {"service":"Dante Primary","recorded_at":"2026-11-02T10:00:00Z"}]}"#,
        )
    }

    #[test]
    fn full_happy_render() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES).push_ok(FIX_DEFAULT_WIFI);

        let d = collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Wi-Fi"),
        );

        assert_eq!(
            fake.calls(),
            vec![
                (SW_VERS.to_string(), args(&["-productVersion"])),
                (SW_VERS.to_string(), args(&["-buildVersion"])),
                (UNAME.to_string(), args(&["-m"])),
                (FAKE.to_string(), args(&["-listallnetworkservices"])),
                (FAKE.to_string(), args(&["-getinfo", "Wi-Fi"])),
            ]
        );
        let want = "\
It Hurts When IP diagnostics
Generated (UTC): 2026-11-19T14:03:00Z

App
Version: 1.13.1
Git SHA: abc1234
Modified: no
Built (UTC): 2026-11-15T10:20:30Z
Build type: release

System
macOS version: 26.7.1
macOS build: 25G241
Architecture: arm64

Helper
State: ok
Message: (none)
Helper version: 1.13.1
Helper git SHA: abc1234

Journal
State: ok
Journaled services: 2
Journaled: Wi-Fi (recorded 2026-11-01T10:00:00Z)
Journaled: Dante Primary (recorded 2026-11-02T10:00:00Z)

Network services
Count: 5
Service: USB 10/100/1000 LAN [enabled]
Service: Dante Primary [enabled]
Service: Wi-Fi [enabled]
Service: Thunderbolt Bridge [enabled]
Service: Old Adapter [disabled, hidden in app]

Selected service
Name: Wi-Fi
Mode: DHCP
IP address: 10.20.30.40
Subnet mask: 255.255.252.0
Router: 10.20.28.1

Settings
Config folder: /Users/test/Library/Application Support/com.ithurtswhenip.app
config.json: exists
Advanced mode: off
Check for updates on launch: on
";
        assert_eq!(d.text, want);
        assert!(!d.text.contains('\u{1b}'), "no ANSI");

        // Struct fields carry the same facts for the 4.4 detail view.
        assert_eq!(d.system.macos_version, "26.7.1");
        assert_eq!(d.selected_service.state, SelectedState::Read);
        assert_eq!(d.network_services.services.len(), 5);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["journal"]["state"], "ok");
        assert_eq!(json["selected_service"]["mode"], "DHCP");
        assert_eq!(json["helper"]["state"], "ok");
        assert!(json["text"]
            .as_str()
            .unwrap()
            .starts_with("It Hurts When IP"));
    }

    #[test]
    fn sw_vers_and_uname_failures_are_unknown() {
        let fake = FakeRunner::new();
        fake.push(Some(1), "", "sw_vers: error\n") // -productVersion fails
            .push_ok("   \n") // -buildVersion prints nothing
            .push(None, "arm64\n", ""); // uname killed by a signal
        fake.push_ok(FIX_DEFAULT_SERVICES);

        let d = collect_with(&fake, FAKE, ctx(ok_helper(), journal_ok_two()), None);

        assert_eq!(d.system.macos_version, "unknown");
        assert_eq!(d.system.macos_build, "unknown");
        assert_eq!(d.system.arch, "unknown");
        assert_lines(
            &d.text,
            &[
                "macOS version: unknown",
                "macOS build: unknown",
                "Architecture: unknown",
                "Count: 5",
            ],
        );

        // The runner failing outright (no canned output) is also "unknown"
        // and the rest of the report still renders.
        let fake = FakeRunner::new();
        let d = collect_with(&fake, FAKE, ctx(ok_helper(), journal_ok_two()), None);
        assert_lines(
            &d.text,
            &[
                "macOS version: unknown",
                "Architecture: unknown",
                "Count: unknown (could not read the service list)",
                "Name: none selected",
            ],
        );
    }

    #[test]
    fn helper_missing_journal_not_available() {
        let missing = status(
            HelperState::Missing,
            "The background helper is missing. Reinstall It Hurts When IP from the latest .pkg.",
        );
        let mut asked = false;
        let journal = journal_for_status(&missing, || {
            asked = true;
            Some("{}".to_string())
        });
        assert!(!asked, "a missing helper must not be asked");
        assert_eq!(journal.state, JournalState::NotAvailable);

        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let d = collect_with(&fake, FAKE, ctx(missing, journal), None);
        assert_lines(
            &d.text,
            &[
                "State: missing",
                "Message: The background helper is missing. Reinstall It Hurts When IP from the latest .pkg.",
                "State: not available (missing)",
            ],
        );
        assert!(!d.text.contains("Helper version:"));

        for (state, want) in [
            (HelperState::NotRunning, "not_running"),
            (HelperState::NoPermission, "no_permission"),
            (HelperState::OldHelper, "old_helper"),
        ] {
            let j = journal_for_status(&status(state, "x"), || panic!("must not ask"));
            assert_eq!(j.state, JournalState::NotAvailable);
            assert_eq!(j.detail.as_deref(), Some(want));
        }
    }

    #[test]
    fn journal_unusable_with_reason() {
        let journal = journal_for_status(&ok_helper(), || {
            Some(
                r#"{"v":2,"ok":"journal","state":"unusable","reason":"unknown schema 2","services":[]}"#
                    .to_string() + "\n",
            )
        });
        assert_eq!(journal.state, JournalState::Unusable);

        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let d = collect_with(&fake, FAKE, ctx(ok_helper(), journal), None);
        assert_lines(&d.text, &["State: unusable", "Reason: unknown schema 2"]);
    }

    #[test]
    fn journal_ok_with_two_services() {
        let j = journal_ok_two();
        assert_eq!(j.state, JournalState::Ok);
        assert_eq!(
            j.services,
            vec![
                JournalEntry {
                    service: "Wi-Fi".to_string(),
                    recorded_at: "2026-11-01T10:00:00Z".to_string()
                },
                JournalEntry {
                    service: "Dante Primary".to_string(),
                    recorded_at: "2026-11-02T10:00:00Z".to_string()
                },
            ]
        );
    }

    #[test]
    fn journal_other_replies() {
        assert_eq!(
            parse_journal_reply(r#"{"v":2,"ok":"journal","state":"none","services":[]}"#).state,
            JournalState::None
        );
        let j =
            parse_journal_reply(r#"{"v":2,"error":"unknown command","code":"unknown_command"}"#);
        assert_eq!(j.state, JournalState::Unreadable);
        assert_eq!(j.detail.as_deref(), Some("helper error: unknown command"));
        for junk in [
            "",
            "not json",
            "[]",
            r#"{"ok":"pong"}"#,
            r#"{"ok":"journal","state":"odd"}"#,
        ] {
            assert_eq!(
                parse_journal_reply(junk).state,
                JournalState::Unreadable,
                "{junk:?}"
            );
        }
        // Version mismatch still asks; no answer is "could not read".
        let mismatch = status(HelperState::VersionMismatch, "x");
        let j = journal_for_status(&mismatch, || None);
        assert_eq!(j.state, JournalState::Unreadable);
        assert_eq!(j.detail.as_deref(), Some("no answer from the helper"));
    }

    #[test]
    fn debug_build_journal_line() {
        // Test builds behave like debug builds: the helper is never asked.
        let j = fetch_journal(&ok_helper());
        assert_eq!(j.state, JournalState::DevBuild);
        assert_eq!(query_journal(), None);

        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let mut c = ctx(super::super::helper::dev_build_status(true), j);
        c.app.build_type = "debug".to_string();
        let d = collect_with(&fake, FAKE, c, None);
        assert_lines(
            &d.text,
            &[
                "Build type: debug",
                "State: dev_build",
                "State: dev build: helper not contacted",
            ],
        );
    }

    #[test]
    fn empty_service_list() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok("An asterisk (*) denotes that a network service is disabled.\n");
        let d = collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Wi-Fi"),
        );
        assert!(d.network_services.read_ok);
        assert!(d.network_services.services.is_empty());
        assert_lines(
            &d.text,
            &["Count: 0", "Name: Wi-Fi", "Status: not in the service list"],
        );
        assert!(!d.text.contains("Service: "));
        assert_eq!(
            fake.calls().len(),
            4,
            "no -getinfo for a service not in the list"
        );
    }

    #[test]
    fn disabled_and_hidden_services_marked() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(
            "An asterisk (*) denotes that a network service is disabled.\n\
             Ethernet\n\
             Tailscale Tunnel\n\
             *Disabled LAN\n\
             *My VPN\n",
        );
        let d = collect_with(&fake, FAKE, ctx(ok_helper(), journal_ok_two()), None);
        assert_lines(
            &d.text,
            &[
                "Count: 4",
                "Service: Ethernet [enabled]",
                "Service: Tailscale Tunnel [enabled, hidden in app]",
                "Service: Disabled LAN [disabled, hidden in app]",
                "Service: My VPN [disabled, hidden in app]",
            ],
        );
    }

    #[test]
    fn selected_service_not_in_list_and_read_failure() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let d = collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Gone LAN"),
        );
        assert_eq!(d.selected_service.state, SelectedState::NotInList);
        assert_lines(
            &d.text,
            &["Name: Gone LAN", "Status: not in the service list"],
        );
        assert_eq!(fake.calls().len(), 4, "no -getinfo for an unknown service");

        // Listed, but -getinfo fails.
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES).push(
            Some(4),
            "** Error: The parameters were not valid.\n",
            "",
        );
        let d = collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Wi-Fi"),
        );
        assert_lines(
            &d.text,
            &["Name: Wi-Fi", "Status: could not read its settings"],
        );

        // Empty string from the UI = none selected.
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let d = collect_with(&fake, FAKE, ctx(ok_helper(), journal_ok_two()), Some(""));
        assert_lines(&d.text, &["Name: none selected"]);
    }

    #[test]
    fn control_characters_never_break_lines() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES);
        let d = collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Line One\nLine Two"),
        );
        assert_lines(&d.text, &["Name: Line One Line Two"]);
    }

    #[test]
    fn only_read_only_programs_are_run() {
        let fake = FakeRunner::new();
        push_system(&fake);
        fake.push_ok(FIX_DEFAULT_SERVICES).push_ok(FIX_DEFAULT_WIFI);
        collect_with(
            &fake,
            FAKE,
            ctx(ok_helper(), journal_ok_two()),
            Some("Wi-Fi"),
        );
        for (program, argv) in fake.calls() {
            let ok = match program.as_str() {
                SW_VERS => argv == args(&["-productVersion"]) || argv == args(&["-buildVersion"]),
                UNAME => argv == args(&["-m"]),
                FAKE => argv[0] == "-listallnetworkservices" || argv[0] == "-getinfo",
                _ => false,
            };
            assert!(ok, "unexpected command {program} {argv:?}");
        }
    }

    #[test]
    fn settings_from_config_path() {
        let s = SettingsInfo::from_config(&DiagnosticsConfig {
            config_path: PathBuf::from("/nonexistent-ihwip-test/config.json"),
            advanced_mode: true,
            check_updates_on_launch: false,
        });
        assert_eq!(s.config_folder, "/nonexistent-ihwip-test");
        assert!(!s.config_json_exists);
        assert!(s.advanced_mode);
        assert!(!s.check_updates_on_launch);
    }
}
