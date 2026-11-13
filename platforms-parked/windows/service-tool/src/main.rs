#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};
mod network;

const SERVICE_NAME: &str = "ItHurtsWhenIPService";
const PIPE_NAME: &str = r"\\.\pipe\ithurtswhenip-service";
const LOG_PATH: &str = r"C:\ProgramData\ItHurtsWhenIP\service.log";
const CREATE_NO_WINDOW: u32 = 0x08000000;

// Status handle made available to the event handler so it can report Stopped
// to the SCM before the process exits. Without this, Windows times out
// waiting for a state transition and reports the service as "could not stop".
static STATUS_HANDLE: OnceLock<ServiceStatusHandle> = OnceLock::new();

fn log(msg: &str) {
    use std::fs::{create_dir_all, OpenOptions};
    let _ = create_dir_all(r"C:\ProgramData\ItHurtsWhenIP");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        let _ = writeln!(f, "[{}] {}", chrono_now(), msg);
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}", secs)
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    log("service_main entered");

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                log("stop/shutdown received");
                if let Some(handle) = STATUS_HANDLE.get() {
                    let _ = handle.set_service_status(ServiceStatus {
                        service_type: ServiceType::OWN_PROCESS,
                        current_state: ServiceState::Stopped,
                        controls_accepted: ServiceControlAccept::empty(),
                        exit_code: ServiceExitCode::Win32(0),
                        checkpoint: 0,
                        wait_hint: Duration::default(),
                        process_id: None,
                    });
                }
                std::process::exit(0);
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = match service_control_handler::register(SERVICE_NAME, event_handler) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("register failed: {}", e));
            return;
        }
    };

    let _ = STATUS_HANDLE.set(status_handle);

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    log("status set to Running, starting pipe server");
    run_pipe_server();
}

fn run_pipe_server() {
    loop {
        log("creating pipe instance");
        let pipe = match create_named_pipe(PIPE_NAME) {
            Ok(p) => {
                log("pipe created successfully");
                p
            }
            Err(e) => {
                log(&format!("pipe creation FAILED: {}", e));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        log("waiting for client connection");
        if connect_named_pipe(&pipe) {
            log("client connected");
            let pipe_clone = match pipe.try_clone() {
                Ok(c) => c,
                Err(e) => {
                    log(&format!("clone failed: {}", e));
                    continue;
                }
            };
            std::thread::spawn(move || handle_client(pipe, pipe_clone));
        } else {
            log("connect_named_pipe returned false");
        }
    }
}

fn handle_client(reader_pipe: std::fs::File, mut writer_pipe: std::fs::File) {
    let reader = BufReader::new(reader_pipe);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log(&format!("read error: {}", e));
                break;
            }
        };
        log(&format!("received: {}", line));
        let response = handle_command(&line);
        log(&format!("responding: {}", response));
        let _ = writeln!(writer_pipe, "{}", response);
    }
    log("client disconnected");
}

fn handle_command(json: &str) -> String {
    let cmd: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => return error_response(&e.to_string()),
    };

    // IPC wire format (matches macOS):
    //   { "cmd": "set_static_ip" | "set_dhcp",
    //     "interface": "<alias>",
    //     "ip": "1.2.3.4",          // set_static_ip only
    //     "subnet": "255.255.255.0", // set_static_ip only — mask string, not prefix length
    //     "gateway": "1.2.3.1"      // set_static_ip only
    //   }
    match cmd["cmd"].as_str().unwrap_or("") {
        "set_static_ip" => {
            let iface = cmd["interface"].as_str().unwrap_or("");
            let ip = cmd["ip"].as_str().unwrap_or("");
            let subnet = cmd["subnet"].as_str().unwrap_or("");
            let gateway = cmd["gateway"].as_str().unwrap_or("");
            let prefix = mask_to_prefix(subnet);
            set_static_ip(iface, ip, prefix, gateway)
        }
        "set_dhcp" => {
            let iface = cmd["interface"].as_str().unwrap_or("");
            set_dhcp(iface)
        }
        _ => error_response("unknown cmd"),
    }
}

// Apply a static IPv4 configuration to an interface using netsh.
//
// One atomic netsh call does all of this:
//   * Removes any existing IPv4 addresses (DHCP-assigned or manual)
//   * Sets the new address and subnet mask
//   * Adds or removes the default gateway
//   * Flips the interface's DHCP flag to Disabled
//
// We used to do this in four separate Win32 IP Helper calls
// (CreateUnicastIpAddressEntry + CreateIpForwardEntry2 etc). That was faster
// (~10ms vs ~80ms) but had three bad side effects: the DHCP flag was never
// disabled, so Get-NetIPInterface reported the interface as still DHCP; the
// routes added via CreateIpForwardEntry2 didn't always show up in
// Get-NetRoute; and a subsequent set_dhcp via netsh would get confused by the
// half-configured state and fail with an unhelpful error.
fn set_static_ip(interface: &str, ip: &str, prefix: u8, gateway: &str) -> String {
    if ip.parse::<std::net::Ipv4Addr>().is_err() {
        return error_response(&format!("invalid IP: {}", ip));
    }

    let has_gateway = !gateway.is_empty() && gateway != "—" && gateway != "0.0.0.0";
    if has_gateway && gateway.parse::<std::net::Ipv4Addr>().is_err() {
        return error_response(&format!("invalid gateway: {}", gateway));
    }

    let mask = prefix_to_mask(prefix);

    let name_arg = format!("name={}", interface);
    let address_arg = format!("address={}", ip);
    let mask_arg = format!("mask={}", mask);
    let gateway_arg = if has_gateway {
        format!("gateway={}", gateway)
    } else {
        "gateway=none".to_string()
    };

    let mut args: Vec<&str> = vec![
        "interface", "ipv4", "set", "address",
        &name_arg,
        "source=static",
        &address_arg,
        &mask_arg,
        &gateway_arg,
    ];
    if has_gateway {
        args.push("gwmetric=1");
    }

    let output = match Command::new("netsh")
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(e) => return error_response(&format!("failed to execute netsh: {}", e)),
    };

    if !output.status.success() {
        return error_response(&format!("failed to set static IP: {}", capture_error(&output)));
    }

    success_response(&format!("Set {} to {}", interface, ip))
}

fn set_dhcp(interface: &str) -> String {
    // Defensive cleanup: an earlier version of the service used the Win32 IP
    // Helper API to add static addresses and routes directly. Those entries
    // don't always get cleaned up by `netsh source=dhcp`, and their presence
    // is exactly what caused the "failed to enable DHCP" error after a static
    // IP had been applied. Clear them here so the first DHCP call after the
    // upgrade can recover. On a clean install this is a no-op.
    if let Ok(luid) = network::alias_to_luid(interface) {
        let _ = network::remove_all_unicast_ipv4(luid);
        let _ = network::remove_all_default_routes_ipv4(luid);
    }

    if let Err(msg) = network::enable_dhcp_ipv4(interface) {
        return error_response(&format!("failed to enable DHCP: {}", msg));
    }

    // DNS reset (PowerShell — no clean direct API)
    let dns_cmd = format!(
        "Set-DnsClientServerAddress -InterfaceAlias '{}' -ResetServerAddresses",
        interface
    );
    run_ps(&dns_cmd);

    // Force renew via ipconfig
    let _ = Command::new("ipconfig")
        .args(["/release", interface])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = Command::new("ipconfig")
        .args(["/renew", interface])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    success_response(&format!("Set {} to DHCP", interface))
}

// netsh and other Windows command-line tools write error text to stdout,
// not stderr. Capturing only stderr leaves us with empty error strings,
// which is what was happening with "failed to enable DHCP" — netsh had a
// real reason but it was on the wrong stream.
fn capture_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit code {:?}", output.status.code())
    }
}

fn prefix_to_mask(prefix: u8) -> String {
    if prefix == 0 {
        return "0.0.0.0".to_string();
    }
    if prefix >= 32 {
        return "255.255.255.255".to_string();
    }
    let mask: u32 = !((1u32 << (32 - prefix)) - 1);
    format!(
        "{}.{}.{}.{}",
        (mask >> 24) & 0xFF,
        (mask >> 16) & 0xFF,
        (mask >> 8) & 0xFF,
        mask & 0xFF
    )
}

// Convert a dotted subnet mask string (e.g. "255.255.255.0") to its prefix
// length (e.g. 24). Used to consume the unified IPC wire format, which sends
// the mask the user typed into the UI rather than a precomputed prefix.
// Falls back to /24 on any parse error — same default the old wire format used.
fn mask_to_prefix(mask: &str) -> u8 {
    let parts: Vec<u8> = mask
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 4 {
        return 24;
    }
    let n: u32 = ((parts[0] as u32) << 24)
        | ((parts[1] as u32) << 16)
        | ((parts[2] as u32) << 8)
        | (parts[3] as u32);
    n.count_ones() as u8
}

fn run_ps(cmd: &str) -> String {
    let ps_path = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let output = Command::new(ps_path)
        .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                format!("ERROR: {}", capture_error(&o))
            }
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

fn success_response(msg: &str) -> String {
    serde_json::json!({ "ok": true, "msg": msg }).to_string()
}

fn error_response(msg: &str) -> String {
    serde_json::json!({ "ok": false, "msg": msg }).to_string()
}

// Pipe with security descriptor allowing all authenticated users to read/write.
// SDDL "D:(A;;GA;;;AU)" = DACL, allow Generic All to Authenticated Users.
fn create_named_pipe(name: &str) -> Result<std::fs::File, std::io::Error> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::{
        SECURITY_ATTRIBUTES,
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    use windows_sys::Win32::Security::Authorization::SDDL_REVISION_1;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Pipes::*;

    let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let sddl: Vec<u16> = "D:(A;;GA;;;AU)".encode_utf16().chain(Some(0)).collect();

    unsafe {
        let mut sd_ptr = std::ptr::null_mut();
        let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1 as u32,
            &mut sd_ptr,
            std::ptr::null_mut(),
        );
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd_ptr,
            bInheritHandle: 0,
        };

        let handle = CreateNamedPipeW(
            wide_name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            65536,
            65536,
            0,
            &mut sa,
        );

        LocalFree(sd_ptr);

        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::fs::File::from_raw_handle(handle as *mut _))
    }
}

fn connect_named_pipe(pipe: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    unsafe {
        ConnectNamedPipe(pipe.as_raw_handle() as isize, std::ptr::null_mut()) != 0
    }
}

fn main() {
    if std::env::args().any(|a| a == "--standalone") {
        log("running in standalone mode");
        run_pipe_server();
        return;
    }

    log("starting via service dispatcher");
    if let Err(e) = service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        log(&format!("dispatcher failed: {}", e));
    }
}