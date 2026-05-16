// macOS implementation: queries network state via `networksetup` and applies
// changes through the privileged helper daemon (com.ithurtswhenip.helper)
// over a Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Command;

const HELPER_SOCKET: &str = "/var/run/com.ithurtswhenip.helper.sock";

pub fn get_interfaces() -> Vec<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("networksetup -listallnetworkservices | tail -n +2 | grep -v -E -i 'vpn|ppp|tun|tap|tailscale'")
        .output();

    match output {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(&result.stdout).to_string();
            stdout
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .collect()
        }
        Err(_) => vec![],
    }
}

pub fn get_ip_info(interface: String) -> serde_json::Value {
    let cmd = format!("networksetup -getinfo '{}'", interface);
    let output = Command::new("sh").arg("-c").arg(&cmd).output();

    match output {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(&result.stdout).to_string();

            let mut ip = String::from("—");
            let mut subnet = String::from("—");
            let mut gateway = String::from("—");
            let mut is_dhcp = false;

            for line in stdout.lines() {
                if line.starts_with("IP address:") {
                    ip = line.replace("IP address:", "").trim().to_string();
                } else if line.starts_with("Subnet mask:") {
                    subnet = line.replace("Subnet mask:", "").trim().to_string();
                } else if line.starts_with("Router:") {
                    gateway = line.replace("Router:", "").trim().to_string();
                } else if line.contains("DHCP") {
                    is_dhcp = true;
                }
            }

            serde_json::json!({
                "ip": ip,
                "subnet": subnet,
                "gateway": gateway,
                "is_dhcp": is_dhcp
            })
        }
        Err(_) => serde_json::json!({
            "ip": "—",
            "subnet": "—",
            "gateway": "—",
            "is_dhcp": false
        }),
    }
}

pub fn set_dhcp(interface: String) -> Result<String, String> {
    send_to_helper(serde_json::json!({
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
    send_to_helper(serde_json::json!({
        "cmd": "set_static_ip",
        "interface": interface,
        "ip": ip,
        "subnet": subnet,
        "gateway": gateway
    }))
}

fn send_to_helper(request: serde_json::Value) -> Result<String, String> {
    let mut stream = UnixStream::connect(HELPER_SOCKET)
        .map_err(|e| format!("Could not connect to helper: {}", e))?;

    let request_str = format!("{}\n", request.to_string());
    stream
        .write_all(request_str.as_bytes())
        .map_err(|e| format!("Failed to send request: {}", e))?;

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let parsed: serde_json::Value = serde_json::from_str(response.trim())
        .map_err(|e| format!("Invalid response: {}", e))?;

    if let Some(ok) = parsed["ok"].as_str() {
        Ok(ok.to_string())
    } else if let Some(err) = parsed["error"].as_str() {
        Err(err.to_string())
    } else {
        Err("Unknown response from helper".to_string())
    }
}