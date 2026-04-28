// Main Backend - Backend (system commands)
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;
use std::fs;
use serde::{Deserialize, Serialize};
use tauri::Manager;



#[derive(Serialize, Deserialize, Clone)]
struct QuickSetProfile {
    name: String,
    ip: String,
    subnet: String,
    gateway: String,
}

#[derive(Serialize, Deserialize)]
struct AppConfig {
    quick_set_1: QuickSetProfile,
    quick_set_2: QuickSetProfile,
    quick_set_3: QuickSetProfile,
    quick_set_4: QuickSetProfile,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            quick_set_1: QuickSetProfile {
                name: String::from("Quick Set 1"),
                ip: String::from(""),
                subnet: String::from(""),
                gateway: String::from(""),
            },
            quick_set_2: QuickSetProfile {
                name: String::from("Quick Set 2"),
                ip: String::from(""),
                subnet: String::from(""),
                gateway: String::from(""),
            },
            quick_set_3: QuickSetProfile {
                name: String::from("Quick Set 3"),
                ip: String::from(""),
                subnet: String::from(""),
                gateway: String::from(""),
            },
            quick_set_4: QuickSetProfile {
                name: String::from("Quick Set 4"),
                ip: String::from(""),
                subnet: String::from(""),
                gateway: String::from(""),
            },
        }
    }
}

fn get_config_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap().join("config.json")
}

fn load_config(app: &tauri::AppHandle) -> AppConfig {
    let path = get_config_path(app);
    if path.exists() {
        let data = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        AppConfig::default()
    }
}

fn save_config(app: &tauri::AppHandle, config: &AppConfig) {
    let path = get_config_path(app);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let data = serde_json::to_string_pretty(config).unwrap();
    fs::write(path, data).ok();
}

#[tauri::command]
fn get_interfaces() -> Vec<String> {
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

#[tauri::command]
fn get_ip_info(interface: String) -> serde_json::Value {
    let cmd = format!("networksetup -getinfo '{}'", interface);
    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output();

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

fn send_to_helper(request: serde_json::Value) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect("/var/run/com.ipswitcher.helper.sock")
        .map_err(|e| format!("Could not connect to helper: {}", e))?;

    let request_str = format!("{}\n", request.to_string());
    stream.write_all(request_str.as_bytes())
        .map_err(|e| format!("Failed to send request: {}", e))?;

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response)
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

#[tauri::command]
fn set_dhcp(interface: String) -> Result<String, String> {
    send_to_helper(serde_json::json!({
        "cmd": "set_dhcp",
        "interface": interface
    }))
}

#[tauri::command]
fn set_static_ip(interface: String, ip: String, subnet: String, gateway: String) -> Result<String, String> {
    send_to_helper(serde_json::json!({
        "cmd": "set_static_ip",
        "interface": interface,
        "ip": ip,
        "subnet": subnet,
        "gateway": gateway
    }))
}

#[tauri::command]
fn get_quick_sets(app: tauri::AppHandle) -> serde_json::Value {
    let config = load_config(&app);
    serde_json::json!({
        "quick_set_1": config.quick_set_1,
        "quick_set_2": config.quick_set_2,
        "quick_set_3": config.quick_set_3,
        "quick_set_4": config.quick_set_4,
    })
}

#[tauri::command]
fn save_quick_set(app: tauri::AppHandle, slot: u8, name: String, ip: String, subnet: String, gateway: String) -> Result<String, String> {
    let mut config = load_config(&app);

    let profile = QuickSetProfile { name, ip, subnet, gateway };

    match slot {
        1 => config.quick_set_1 = profile,
        2 => config.quick_set_2 = profile,
        3 => config.quick_set_3 = profile,
        _ => config.quick_set_4 = profile,
    }

    save_config(&app, &config);
    Ok(String::from("Saved"))
}

#[tauri::command]
fn save_last_interface(app: tauri::AppHandle, interface: String) {
    let base = get_config_path(&app);
    let path = base.parent().unwrap().join("last_interface.json");
    let data = serde_json::json!({ "interface": interface });
    fs::write(&path, data.to_string()).ok();
}

#[tauri::command]
fn get_last_interface(app: tauri::AppHandle) -> String {
    let path = get_config_path(&app).parent().unwrap().join("last_interface.json");
    if path.exists() {
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                return parsed["interface"].as_str().unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_interfaces,
            get_ip_info,
            set_dhcp,
            set_static_ip,
            get_quick_sets,
            save_quick_set,
            save_last_interface,
            get_last_interface
        ])
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();

            // Restore saved position
            let config_dir = app.path().app_data_dir().unwrap();
            let pos_path = config_dir.join("window_position.json");

            if pos_path.exists() {
                if let Ok(data) = fs::read_to_string(&pos_path) {
                    if let Ok(pos) = serde_json::from_str::<serde_json::Value>(&data) {
                        let x = pos["x"].as_f64().unwrap_or(100.0);
                        let y = pos["y"].as_f64().unwrap_or(100.0);
                        window.set_position(tauri::Position::Physical(
                            tauri::PhysicalPosition { x: x as i32, y: y as i32 }
                        )).ok();
                    }
                }
            }

            // Save position when window moves
            let window_clone = window.clone();
            let config_dir_clone = config_dir.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Moved(pos) = event {
                    let pos_data = serde_json::json!({
                        "x": pos.x,
                        "y": pos.y
                    });
                    let pos_path = config_dir_clone.join("window_position.json");
                    fs::create_dir_all(&config_dir_clone).ok();
                    fs::write(&pos_path, pos_data.to_string()).ok();
                }
                // Also save on resize
                if let tauri::WindowEvent::Resized(_) = event {
                    if let Ok(pos) = window_clone.outer_position() {
                        let pos_data = serde_json::json!({
                            "x": pos.x,
                            "y": pos.y
                        });
                        let pos_path = config_dir_clone.join("window_position.json");
                        fs::write(&pos_path, pos_data.to_string()).ok();
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}