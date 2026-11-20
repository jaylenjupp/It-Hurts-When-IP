// macOS implementation: queries network state via `networksetup` and applies
// changes through the privileged helper daemon (com.ithurtswhenip.helper)
// over a Unix socket. The in-app uninstall (`macos/rescue.rs`) never uses the
// helper: it runs the bundled rescue script.
//
// Reads go through a `CommandRunner` with argv only (no shell) and the pure
// parsers in `macos/parse.rs`.
//
// Helper protocol v2 (requests, replies, status banner) lives in
// `macos/helper.rs`. The helper socket code is compiled into release builds
// only. Debug and test
// builds contain no socket code at all: their writes go to the program named
// by IHWIP_NETWORKSETUP (the fake) or are refused. The build host runs a real
// root helper, so a dev build must never be able to reach it.

mod diagnostics;
mod helper;
mod installer;
mod parse;
mod repair;
mod rescue;

use super::runner::{CmdOutput, CommandRunner, SystemRunner};
use parse::IpInfo;

// In-app uninstall through the bundled rescue script (no helper involved).
pub use rescue::{rescue_plan, rescue_uninstall, RescuePlan, RescueRun};
// In-app network repair through the same script (P3-D4: never the helper).
pub use repair::{repair_plan, repair_run, RepairPlan, RepairRun};

pub use helper::HelperStatus;

// Settings → Maintenance → Copy diagnostics (read-only).
pub use diagnostics::{get_diagnostics, Diagnostics, DiagnosticsConfig};

// Settings → Updates: checksum of a downloaded pkg and opening it in
// Installer (release builds only).
pub use installer::{open_pkg, sha256_file};

const SYSTEM_NETWORKSETUP: &str = "/usr/sbin/networksetup";

/// Services whose name contains any of these (case-insensitive) are hidden
/// from the interface list (same set the old `grep -v -E -i` used).
const HIDDEN_NAME_PARTS: [&str; 5] = ["vpn", "ppp", "tun", "tap", "tailscale"];

const DEBUG_NO_FAKE: &str =
    "debug build: set IHWIP_NETWORKSETUP to the fake; real helper is never used in debug builds";

// --- Public API (called from main.rs; signatures must not change) ---

pub fn get_interfaces() -> Vec<String> {
    let override_path = networksetup_override();
    get_interfaces_with(
        &SystemRunner,
        networksetup_program(override_path.as_deref()),
    )
}

pub fn get_ip_info(interface: String) -> serde_json::Value {
    let override_path = networksetup_override();
    let info = get_ip_info_with(
        &SystemRunner,
        networksetup_program(override_path.as_deref()),
        &interface,
    );
    ip_info_json(&info)
}

pub fn set_dhcp(interface: String) -> Result<String, String> {
    let override_path = networksetup_override();
    set_dhcp_with(
        &SystemRunner,
        cfg!(debug_assertions),
        override_path.as_deref(),
        &interface,
    )
}

pub fn set_static_ip(
    interface: String,
    ip: String,
    subnet: String,
    gateway: String,
) -> Result<String, String> {
    let override_path = networksetup_override();
    set_static_ip_with(
        &SystemRunner,
        cfg!(debug_assertions),
        override_path.as_deref(),
        &interface,
        &ip,
        &subnet,
        &gateway,
    )
}

/// Banner status for the privileged helper. Debug and test builds never
/// contact the helper and always report `dev_build`; release builds check the
/// installed files (read-only), connect and ping with a 2 s timeout.
#[cfg(any(debug_assertions, test))]
pub fn get_helper_status() -> HelperStatus {
    helper::dev_build_status(networksetup_override().is_some())
}

#[cfg(not(any(debug_assertions, test)))]
pub fn get_helper_status() -> HelperStatus {
    use std::path::Path;
    let files_present =
        Path::new(helper::HELPER_BINARY).exists() && Path::new(helper::HELPER_PLIST).exists();
    if !files_present {
        return helper::classify(
            false,
            helper::ConnectOutcome::NoSocket,
            None,
            env!("CARGO_PKG_VERSION"),
        );
    }
    let (connect, reply) = helper::socket::probe();
    helper::classify(true, connect, reply.as_deref(), env!("CARGO_PKG_VERSION"))
}

// --- App menu ---

/// Id of the "Settings…" menu item (⌘,).
pub const MENU_SETTINGS_ID: &str = "ihwip-settings";
/// Id of the "Uninstall It Hurts When IP…" menu item.
pub const MENU_UNINSTALL_ID: &str = "ihwip-uninstall";
/// Id of the "Repair Network…" menu item.
pub const MENU_REPAIR_ID: &str = "ihwip-repair";

/// What goes into the app submenu, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMenuEntry {
    Settings,
    Uninstall,
    Repair,
    Separator,
}

/// Insertions into Tauri's default app submenu (About, separator, Services,
/// …), applied in this order. Result:
/// 0 About · 1 separator · 2 Settings… · 3 separator ·
/// 4 Uninstall It Hurts When IP… · 5 Repair Network… · 6 separator ·
/// 7 Services …
const APP_MENU_INSERTS: [(AppMenuEntry, usize); 5] = [
    (AppMenuEntry::Settings, 2),
    (AppMenuEntry::Separator, 3),
    (AppMenuEntry::Uninstall, 4),
    (AppMenuEntry::Repair, 5),
    (AppMenuEntry::Separator, 6),
];

/// Tauri's default macOS menu, plus "Settings…" (⌘,), "Uninstall It Hurts
/// When IP…" and "Repair Network…" in the app menu, after About (see
/// `APP_MENU_INSERTS`).
pub fn app_menu<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};

    let menu = Menu::default(handle)?;
    if let Some(MenuItemKind::Submenu(app_submenu)) = menu.items()?.first() {
        for (entry, index) in APP_MENU_INSERTS {
            match entry {
                AppMenuEntry::Settings => app_submenu.insert(
                    &MenuItem::with_id(
                        handle,
                        MENU_SETTINGS_ID,
                        "Settings…",
                        true,
                        Some("CmdOrCtrl+,"),
                    )?,
                    index,
                )?,
                AppMenuEntry::Uninstall => app_submenu.insert(
                    &MenuItem::with_id(
                        handle,
                        MENU_UNINSTALL_ID,
                        "Uninstall It Hurts When IP…",
                        true,
                        None::<&str>,
                    )?,
                    index,
                )?,
                AppMenuEntry::Repair => app_submenu.insert(
                    &MenuItem::with_id(
                        handle,
                        MENU_REPAIR_ID,
                        "Repair Network…",
                        true,
                        None::<&str>,
                    )?,
                    index,
                )?,
                AppMenuEntry::Separator => {
                    app_submenu.insert(&PredefinedMenuItem::separator(handle)?, index)?
                }
            }
        }
    }
    Ok(menu)
}

// --- Which networksetup to run ---

/// Debug builds only: IHWIP_NETWORKSETUP (if set and non-empty) replaces the
/// system networksetup. Should be an absolute path to the fake.
#[cfg(debug_assertions)]
fn networksetup_override() -> Option<String> {
    std::env::var("IHWIP_NETWORKSETUP")
        .ok()
        .filter(|v| !v.is_empty())
}

/// Release builds never read IHWIP_NETWORKSETUP.
#[cfg(not(debug_assertions))]
fn networksetup_override() -> Option<String> {
    None
}

fn networksetup_program(override_path: Option<&str>) -> &str {
    match override_path {
        Some(p) if !p.is_empty() => p,
        _ => SYSTEM_NETWORKSETUP,
    }
}

// --- Reads ---

/// A command failed if it didn't exit 0 or reported an error on stdout or
/// stderr. networksetup prints errors on stdout, so stderr alone is not
/// enough.
fn command_failed(out: &CmdOutput) -> bool {
    out.code != Some(0)
        || parse::is_error_output(&out.stdout)
        || parse::is_error_output(&out.stderr)
}

fn is_hidden_service(name: &str) -> bool {
    let lower = name.to_lowercase();
    HIDDEN_NAME_PARTS.iter().any(|part| lower.contains(part))
}

/// Enabled services only, minus VPN/tunnel-like names. A failed read gives
/// an empty list.
fn get_interfaces_with(runner: &dyn CommandRunner, program: &str) -> Vec<String> {
    match runner.run(program, &["-listallnetworkservices"]) {
        Ok(out) if !command_failed(&out) => parse::parse_service_list(&out.stdout)
            .into_iter()
            .filter(|s| s.enabled && !is_hidden_service(&s.name))
            .map(|s| s.name)
            .collect(),
        _ => vec![],
    }
}

/// The service name is passed as one argv element, whatever it contains.
/// A failed read gives the "—" fallback.
fn get_ip_info_with(runner: &dyn CommandRunner, program: &str, interface: &str) -> IpInfo {
    match runner.run(program, &["-getinfo", interface]) {
        Ok(out) if !command_failed(&out) => parse::parse_ip_info(&out.stdout),
        _ => IpInfo::fallback(),
    }
}

fn ip_info_json(info: &IpInfo) -> serde_json::Value {
    serde_json::json!({
        "ip": info.ip,
        "subnet": info.subnet,
        "gateway": info.gateway,
        "is_dhcp": info.is_dhcp
    })
}

// --- Writes ---

#[derive(Debug, PartialEq, Eq)]
enum WriteRoute<'a> {
    /// Release builds: the privileged helper over its socket.
    Helper,
    /// Debug builds with IHWIP_NETWORKSETUP set: run that program directly.
    Fake(&'a str),
    /// Debug builds without an override: refuse.
    Refuse,
}

/// Where a write goes. Debug builds can never get `Helper`.
fn choose_write_route(is_debug: bool, override_path: Option<&str>) -> WriteRoute<'_> {
    if !is_debug {
        return WriteRoute::Helper;
    }
    match override_path {
        Some(p) if !p.is_empty() => WriteRoute::Fake(p),
        _ => WriteRoute::Refuse,
    }
}

fn set_dhcp_with(
    runner: &dyn CommandRunner,
    is_debug: bool,
    override_path: Option<&str>,
    interface: &str,
) -> Result<String, String> {
    match choose_write_route(is_debug, override_path) {
        WriteRoute::Helper => {
            send_to_helper(helper::v2_request("set_dhcp", &[("interface", interface)]))
        }
        WriteRoute::Fake(program) => run_write(
            runner,
            program,
            &["-setdhcp", interface],
            format!("Set {} to DHCP", interface),
        ),
        WriteRoute::Refuse => Err(DEBUG_NO_FAKE.to_string()),
    }
}

fn set_static_ip_with(
    runner: &dyn CommandRunner,
    is_debug: bool,
    override_path: Option<&str>,
    interface: &str,
    ip: &str,
    subnet: &str,
    gateway: &str,
) -> Result<String, String> {
    match choose_write_route(is_debug, override_path) {
        WriteRoute::Helper => send_to_helper(helper::v2_request(
            "set_static_ip",
            &[
                ("interface", interface),
                ("ip", ip),
                ("subnet", subnet),
                ("gateway", gateway),
            ],
        )),
        WriteRoute::Fake(program) => run_write(
            runner,
            program,
            &setmanual_args(interface, ip, subnet, gateway),
            format!("Set {} to {}", interface, ip),
        ),
        WriteRoute::Refuse => Err(DEBUG_NO_FAKE.to_string()),
    }
}

/// `-setmanual <svc> <ip> <mask> <router>`: the gateway is always passed as
/// given, `""` when empty (same as the helper).
fn setmanual_args<'a>(
    interface: &'a str,
    ip: &'a str,
    subnet: &'a str,
    gateway: &'a str,
) -> Vec<&'a str> {
    vec!["-setmanual", interface, ip, subnet, gateway]
}

/// Debug-build write through the runner. Ok/Err strings match what the
/// helper returns, so the UI behaves the same.
fn run_write(
    runner: &dyn CommandRunner,
    program: &str,
    args: &[&str],
    ok_message: String,
) -> Result<String, String> {
    let out = runner
        .run(program, args)
        .map_err(|e| format!("Could not run {}: {}", program, e))?;
    if command_failed(&out) {
        Err(failure_message(&out))
    } else {
        Ok(ok_message)
    }
}

fn failure_message(out: &CmdOutput) -> String {
    let text = [out.stdout.trim(), out.stderr.trim()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        return text;
    }
    match out.code {
        Some(code) => format!("networksetup exited with code {}", code),
        None => "networksetup was stopped by a signal".to_string(),
    }
}

/// Debug and test builds: no socket code is compiled in. `choose_write_route`
/// never picks the helper in a debug build; this stub only lets the match
/// compile (and keeps `cargo test --release` away from the socket too).
#[cfg(any(debug_assertions, test))]
fn send_to_helper(_request: serde_json::Value) -> Result<String, String> {
    Err("the privileged helper is not used in debug or test builds".to_string())
}

/// Release builds: one v2 request over the helper socket (see
/// `helper::socket`). The helper's error text is returned in full.
#[cfg(not(any(debug_assertions, test)))]
fn send_to_helper(request: serde_json::Value) -> Result<String, String> {
    helper::socket::send(request)
}

#[cfg(test)]
mod tests {
    // These tests use FakeRunner only. None of them calls a *_with write
    // function with is_debug = false, and test builds have no socket code.
    use super::*;
    use crate::platform::runner::FakeRunner;

    const FAKE: &str = "/abs/path/to/fake-networksetup";

    const FIX_DEFAULT_SERVICES: &str =
        include_str!("../../../tools/fake-networksetup/fixtures/default/services.txt");
    const FIX_STUDIO_SERVICES: &str =
        include_str!("../../../tools/fake-networksetup/fixtures/mac-studio/services.txt");
    const FIX_DEFAULT_WIFI: &str =
        include_str!("../../../tools/fake-networksetup/fixtures/default/info/Wi-Fi.txt");
    const FIX_STUDIO_UNKNOWN: &str = include_str!(
        "../../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.stdout.txt"
    );
    const FIX_STUDIO_UNKNOWN_EXIT: &str = include_str!(
        "../../../tools/fake-networksetup/fixtures/mac-studio/errors/getinfo-nonexistent.exit"
    );

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn unknown_exit() -> Option<i32> {
        Some(FIX_STUDIO_UNKNOWN_EXIT.trim().parse().unwrap())
    }

    // --- program path ---

    #[test]
    fn program_defaults_to_system_networksetup() {
        assert_eq!(networksetup_program(None), "/usr/sbin/networksetup");
        assert_eq!(networksetup_program(Some("")), "/usr/sbin/networksetup");
        assert_eq!(networksetup_program(Some(FAKE)), FAKE);
    }

    // --- get_interfaces ---

    #[test]
    fn get_interfaces_runs_exact_argv_and_hides_disabled() {
        let fake = FakeRunner::new();
        fake.push_ok(FIX_DEFAULT_SERVICES);

        let names = get_interfaces_with(&fake, FAKE);

        assert_eq!(
            fake.calls(),
            vec![(FAKE.to_string(), args(&["-listallnetworkservices"]))]
        );
        assert_eq!(
            names,
            vec![
                "USB 10/100/1000 LAN",
                "Dante Primary",
                "Wi-Fi",
                "Thunderbolt Bridge"
            ]
        );
    }

    #[test]
    fn get_interfaces_hides_vpn_like_names_case_insensitively() {
        let fake = FakeRunner::new();
        fake.push_ok(FIX_STUDIO_SERVICES);
        let names = get_interfaces_with(&fake, FAKE);
        assert!(!names.iter().any(|n| n == "VPN Service"));
        assert_eq!(names.len(), 8);

        let fake = FakeRunner::new();
        fake.push_ok(
            "An asterisk (*) denotes that a network service is disabled.\n\
             Ethernet\n\
             Tailscale Tunnel\n\
             PPPoE Uplink\n\
             utun3\n\
             TAP Adapter\n\
             My vpn\n\
             *Disabled LAN\n\
             Wi-Fi\n",
        );
        assert_eq!(get_interfaces_with(&fake, FAKE), vec!["Ethernet", "Wi-Fi"]);
    }

    #[test]
    fn get_interfaces_failed_read_is_empty() {
        let fake = FakeRunner::new();
        fake.push(Some(1), FIX_DEFAULT_SERVICES, "");
        assert!(get_interfaces_with(&fake, FAKE).is_empty());

        let fake = FakeRunner::new();
        fake.push_ok("** Error: The parameters were not valid.\n");
        assert!(get_interfaces_with(&fake, FAKE).is_empty());

        // Queue empty -> runner returns an io error.
        let fake = FakeRunner::new();
        assert!(get_interfaces_with(&fake, FAKE).is_empty());
    }

    // --- get_ip_info ---

    #[test]
    fn get_ip_info_passes_hostile_name_as_one_argv_element() {
        let hostile = "x'; rm -rf /'";
        let fake = FakeRunner::new();
        fake.push(unknown_exit(), FIX_STUDIO_UNKNOWN, "");

        let info = get_ip_info_with(&fake, FAKE, hostile);

        assert_eq!(
            fake.calls(),
            vec![(FAKE.to_string(), args(&["-getinfo", hostile]))]
        );
        assert_eq!(info, IpInfo::fallback());
    }

    #[test]
    fn get_ip_info_passes_name_with_newline_as_one_argv_element() {
        let name = "Line One\nLine Two";
        let fake = FakeRunner::new();
        fake.push_ok(FIX_DEFAULT_WIFI);

        get_ip_info_with(&fake, FAKE, name);

        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, args(&["-getinfo", "Line One\nLine Two"]));
    }

    #[test]
    fn get_ip_info_unknown_service_is_fallback() {
        // Real macOS: exit 4, two lines on stdout, nothing on stderr.
        let fake = FakeRunner::new();
        fake.push(unknown_exit(), FIX_STUDIO_UNKNOWN, "");
        assert_eq!(get_ip_info_with(&fake, FAKE, "Nope"), IpInfo::fallback());

        // Same text with exit 0 is still a failure.
        let fake = FakeRunner::new();
        fake.push_ok(FIX_STUDIO_UNKNOWN);
        assert_eq!(get_ip_info_with(&fake, FAKE, "Nope"), IpInfo::fallback());
    }

    #[test]
    fn get_ip_info_never_trusts_exit_or_stderr_alone() {
        // Non-zero exit with normal-looking stdout -> fallback.
        let fake = FakeRunner::new();
        fake.push(Some(1), FIX_DEFAULT_WIFI, "");
        assert_eq!(get_ip_info_with(&fake, FAKE, "Wi-Fi"), IpInfo::fallback());

        // Exit 0 but an error on stderr -> fallback.
        let fake = FakeRunner::new();
        fake.push(Some(0), FIX_DEFAULT_WIFI, "** Error: something\n");
        assert_eq!(get_ip_info_with(&fake, FAKE, "Wi-Fi"), IpInfo::fallback());

        // Killed by a signal -> fallback.
        let fake = FakeRunner::new();
        fake.push(None, FIX_DEFAULT_WIFI, "");
        assert_eq!(get_ip_info_with(&fake, FAKE, "Wi-Fi"), IpInfo::fallback());
    }

    #[test]
    fn get_ip_info_json_shape_is_unchanged() {
        let fake = FakeRunner::new();
        fake.push_ok(FIX_DEFAULT_WIFI);
        let json = ip_info_json(&get_ip_info_with(&fake, FAKE, "Wi-Fi"));
        assert_eq!(
            json,
            serde_json::json!({
                "ip": "10.20.30.40",
                "subnet": "255.255.252.0",
                "gateway": "10.20.28.1",
                "is_dhcp": true
            })
        );

        assert_eq!(
            ip_info_json(&IpInfo::fallback()),
            serde_json::json!({
                "ip": "—",
                "subnet": "—",
                "gateway": "—",
                "is_dhcp": false
            })
        );
    }

    #[test]
    fn setmanual_args_always_has_four_operands() {
        assert_eq!(
            setmanual_args("LAN", "10.0.0.5", "255.0.0.0", ""),
            vec!["-setmanual", "LAN", "10.0.0.5", "255.0.0.0", ""]
        );
        assert_eq!(
            setmanual_args("LAN", "10.0.0.5", "255.0.0.0", "10.0.0.1"),
            vec!["-setmanual", "LAN", "10.0.0.5", "255.0.0.0", "10.0.0.1"]
        );
    }

    #[test]
    fn helper_status_in_test_builds_is_dev_build() {
        // Test (and debug) builds never contact the helper.
        assert_eq!(get_helper_status().state, helper::HelperState::DevBuild);
    }

    // --- write routing ---

    #[test]
    fn write_route_choice() {
        assert_eq!(choose_write_route(false, None), WriteRoute::Helper);
        assert_eq!(choose_write_route(false, Some(FAKE)), WriteRoute::Helper);
        assert_eq!(choose_write_route(true, Some(FAKE)), WriteRoute::Fake(FAKE));
        assert_eq!(choose_write_route(true, None), WriteRoute::Refuse);
        assert_eq!(choose_write_route(true, Some("")), WriteRoute::Refuse);
    }

    #[test]
    fn debug_static_ip_with_override_runs_setmanual() {
        let fake = FakeRunner::new();
        fake.push_ok("");
        let result = set_static_ip_with(
            &fake,
            true,
            Some(FAKE),
            "Dante Primary",
            "192.168.10.50",
            "255.255.255.0",
            "192.168.10.1",
        );
        assert_eq!(result, Ok("Set Dante Primary to 192.168.10.50".to_string()));
        assert_eq!(
            fake.calls(),
            vec![(
                FAKE.to_string(),
                args(&[
                    "-setmanual",
                    "Dante Primary",
                    "192.168.10.50",
                    "255.255.255.0",
                    "192.168.10.1"
                ])
            )]
        );
    }

    #[test]
    fn debug_static_ip_passes_empty_gateway_as_empty_arg() {
        let fake = FakeRunner::new();
        fake.push_ok("").push_ok("");

        set_static_ip_with(&fake, true, Some(FAKE), "LAN", "10.0.0.5", "255.0.0.0", "").unwrap();
        set_static_ip_with(
            &fake,
            true,
            Some(FAKE),
            "LAN",
            "10.0.0.5",
            "255.0.0.0",
            "0.0.0.0",
        )
        .unwrap();

        let calls = fake.calls();
        assert_eq!(
            calls[0].1,
            args(&["-setmanual", "LAN", "10.0.0.5", "255.0.0.0", ""])
        );
        assert_eq!(
            calls[1].1,
            args(&["-setmanual", "LAN", "10.0.0.5", "255.0.0.0", "0.0.0.0"])
        );
    }

    #[test]
    fn debug_dhcp_with_override_runs_setdhcp() {
        let fake = FakeRunner::new();
        fake.push_ok("");
        let result = set_dhcp_with(&fake, true, Some(FAKE), "USB 10/100/1000 LAN");
        assert_eq!(result, Ok("Set USB 10/100/1000 LAN to DHCP".to_string()));
        assert_eq!(
            fake.calls(),
            vec![(FAKE.to_string(), args(&["-setdhcp", "USB 10/100/1000 LAN"]))]
        );
    }

    #[test]
    fn debug_write_without_override_is_refused_without_running_anything() {
        let fake = FakeRunner::new();

        let dhcp = set_dhcp_with(&fake, true, None, "Wi-Fi");
        let manual = set_static_ip_with(
            &fake,
            true,
            None,
            "Wi-Fi",
            "10.0.0.5",
            "255.0.0.0",
            "10.0.0.1",
        );
        let empty = set_dhcp_with(&fake, true, Some(""), "Wi-Fi");

        assert_eq!(dhcp, Err(DEBUG_NO_FAKE.to_string()));
        assert_eq!(manual, Err(DEBUG_NO_FAKE.to_string()));
        assert_eq!(empty, Err(DEBUG_NO_FAKE.to_string()));
        assert!(fake.calls().is_empty(), "runner must not be called");
    }

    #[test]
    fn debug_write_failure_reports_stdout_error() {
        let fake = FakeRunner::new();
        fake.push(unknown_exit(), FIX_STUDIO_UNKNOWN, "");
        let err = set_dhcp_with(&fake, true, Some(FAKE), "No Such Service IHWIP").unwrap_err();
        assert!(err.contains("is not a recognized network service"), "{err}");
        assert!(err.contains("** Error"), "{err}");

        let fake = FakeRunner::new();
        fake.push(Some(3), "", "");
        let err = set_dhcp_with(&fake, true, Some(FAKE), "Wi-Fi").unwrap_err();
        assert_eq!(err, "networksetup exited with code 3");

        let fake = FakeRunner::new();
        let err = set_dhcp_with(&fake, true, Some(FAKE), "Wi-Fi").unwrap_err();
        assert!(
            err.starts_with("Could not run /abs/path/to/fake-networksetup"),
            "{err}"
        );
    }

    #[test]
    fn app_menu_inserts_give_the_agreed_order() {
        // Insert indices must be consecutive from 2 (right after About and
        // its separator), so the inserted block keeps exactly this order.
        let entries: Vec<AppMenuEntry> = APP_MENU_INSERTS.iter().map(|(e, _)| *e).collect();
        assert_eq!(
            entries,
            vec![
                AppMenuEntry::Settings,
                AppMenuEntry::Separator,
                AppMenuEntry::Uninstall,
                AppMenuEntry::Repair,
                AppMenuEntry::Separator,
            ]
        );
        let indices: Vec<usize> = APP_MENU_INSERTS.iter().map(|(_, i)| *i).collect();
        assert_eq!(indices, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn app_menu_ids_are_stable() {
        assert_eq!(MENU_SETTINGS_ID, "ihwip-settings");
        assert_eq!(MENU_UNINSTALL_ID, "ihwip-uninstall");
        assert_eq!(MENU_REPAIR_ID, "ihwip-repair");
    }
}
