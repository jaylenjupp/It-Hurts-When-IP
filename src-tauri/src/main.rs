// Main Backend - shared across platforms.
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;

use serde::{Deserialize, Serialize};
use tauri::Manager;

mod platform;

// --- Persistent config ---

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
        let blank = |n: &str| QuickSetProfile {
            name: n.to_string(),
            ip: String::new(),
            subnet: String::new(),
            gateway: String::new(),
        };
        AppConfig {
            quick_set_1: blank("Quick Set 1"),
            quick_set_2: blank("Quick Set 2"),
            quick_set_3: blank("Quick Set 3"),
            quick_set_4: blank("Quick Set 4"),
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

// --- Tauri commands (delegate to platform module) ---

#[tauri::command]
fn get_interfaces() -> Vec<String> {
    platform::get_interfaces()
}

#[tauri::command]
fn get_ip_info(interface: String) -> serde_json::Value {
    platform::get_ip_info(interface)
}

#[tauri::command]
fn set_dhcp(interface: String) -> Result<String, String> {
    platform::set_dhcp(interface)
}

#[tauri::command]
fn set_static_ip(
    interface: String,
    ip: String,
    subnet: String,
    gateway: String,
) -> Result<String, String> {
    platform::set_static_ip(interface, ip, subnet, gateway)
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
fn save_quick_set(
    app: tauri::AppHandle,
    slot: u8,
    name: String,
    ip: String,
    subnet: String,
    gateway: String,
) -> Result<String, String> {
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

// --- Entrypoint ---

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
            get_last_interface,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}