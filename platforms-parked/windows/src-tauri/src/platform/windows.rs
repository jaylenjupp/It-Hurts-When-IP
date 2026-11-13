// Windows implementation: queries network state via PowerShell and applies
// changes through the privileged service (ItHurtsWhenIPService) over a
// named pipe at \\.\pipe\ithurtswhenip-service.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::Command;

const PIPE_NAME: &str = r"\\.\pipe\ithurtswhenip-service";
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn get_interfaces() -> Vec<String> {
    let out = run_ps(
        "Get-NetAdapter | Where-Object { $_.Status -eq 'Up' -and $_.Virtual -eq $false } | Select-Object -ExpandProperty Name"
    );
    out.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn get_ip_info(interface: String) -> serde_json::Value {
    let cmd = format!(
        r#"
$iface = '{}'
$ip = (Get-NetIPAddress -InterfaceAlias $iface -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -First 1)
$gw = (Get-NetRoute -InterfaceAlias $iface -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue | Select-Object -First 1)
$dhcp = (Get-NetIPInterface -InterfaceAlias $iface -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -First 1)
[PSCustomObject]@{{
    ip = if ($ip) {{ $ip.IPAddress }} else {{ '' }}
    prefix = if ($ip) {{ $ip.PrefixLength }} else {{ 0 }}
    gateway = if ($gw) {{ $gw.NextHop }} else {{ '' }}
    dhcp = if ($dhcp) {{ $dhcp.Dhcp -eq 'Enabled' }} else {{ $false }}
}} | ConvertTo-Json -Compress
"#,
        interface
    );

    let out = run_ps(&cmd);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap_or(serde_json::json!({}));

    let ip = parsed["ip"].as_str().unwrap_or("").to_string();
    let ip = if ip.is_empty() { String::from("—") } else { ip };

    let prefix = parsed["prefix"].as_u64().unwrap_or(0) as u8;
    let subnet = prefix_to_mask(prefix);

    let gateway = parsed["gateway"].as_str().unwrap_or("").to_string();
    let gateway = if !gateway.is_empty() {
        gateway
    } else if ip != "—" {
        // Interface has an IP but no default route configured. Show "0.0.0.0"
        // rather than "—" to match the macOS version, which echoes back the
        // literal value the user saved in the quick set (typically "0.0.0.0"
        // for configurations without a gateway).
        String::from("0.0.0.0")
    } else {
        String::from("—")
    };

    let is_dhcp = parsed["dhcp"].as_bool().unwrap_or(false);

    serde_json::json!({
        "ip": ip,
        "subnet": subnet,
        "gateway": gateway,
        "is_dhcp": is_dhcp
    })
}

pub fn set_dhcp(interface: String) -> Result<String, String> {
    pipe_command(serde_json::json!({
        "cmd": "set_dhcp",
        "interface": interface
    }))
}

pub fn set_static_ip(
    interface: String,
    ip: String,
    subnet: String,
    gateway: String,
) -> Result<String, String> {
    pipe_command(serde_json::json!({
        "cmd": "set_static_ip",
        "interface": interface,
        "ip": ip,
        "subnet": subnet,
        "gateway": gateway
    }))
}

// Send a command to the named pipe service and return the response.
//
// The wire format is one JSON object per line. The service reads a line,
// dispatches by the "cmd" field, and writes one JSON response line back.
fn pipe_command(payload: serde_json::Value) -> Result<String, String> {
    use std::fs::OpenOptions;

    let pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .map_err(|e| {
            format!("Could not connect to service: {}. Is ItHurtsWhenIPService running?", e)
        })?;

    let mut writer = &pipe;
    let reader = BufReader::new(&pipe);

    let msg = format!("{}\n", payload.to_string());
    writer
        .write_all(msg.as_bytes())
        .map_err(|e| format!("Failed to send command: {}", e))?;

    let line = reader
        .lines()
        .next()
        .ok_or_else(|| String::from("No response from service"))?
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let parsed: serde_json::Value = serde_json::from_str(&line)
        .map_err(|e| format!("Bad response from service: {}", e))?;

    let ok = parsed["ok"].as_bool().unwrap_or(false);
    let msg = parsed["msg"].as_str().unwrap_or("").to_string();
    if ok {
        Ok(msg)
    } else {
        Err(if msg.is_empty() { "Unknown service error".into() } else { msg })
    }
}

fn run_ps(cmd: &str) -> String {
    let ps_path = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let output = Command::new(ps_path)
        .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => String::new(),
    }
}

// Convert a CIDR prefix length (e.g. 24) to a dotted subnet mask string
// (e.g. "255.255.255.0"). PowerShell's Get-NetIPAddress returns prefix length,
// but our UI displays masks, so we convert here.
fn prefix_to_mask(prefix: u8) -> String {
    if prefix == 0 {
        return String::from("—");
    }
    let mask: u32 = if prefix >= 32 { 0xFFFFFFFF } else { !((1u32 << (32 - prefix)) - 1) };
    format!(
        "{}.{}.{}.{}",
        (mask >> 24) & 0xFF,
        (mask >> 16) & 0xFF,
        (mask >> 8) & 0xFF,
        mask & 0xFF
    )
}