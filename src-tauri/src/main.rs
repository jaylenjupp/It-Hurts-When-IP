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
        .plugin(tauri_plugin_os::init())
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
       .setup(|app| {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let window = app.get_webview_window("main").unwrap();
            let config_dir = app.path().app_data_dir().unwrap();
            let pos_path = config_dir.join("window_position.json");

            // Save handler ignores events until restore is done. Without this
            // gate, events firing during the initial window setup can
            // overwrite our saved position before we get a chance to restore.
            let ready_to_save = Arc::new(AtomicBool::new(false));

            // Restore saved position on launch.
            if pos_path.exists() {
                if let Ok(data) = fs::read_to_string(&pos_path) {
                    if let Ok(pos) = serde_json::from_str::<serde_json::Value>(&data) {
                        let x = pos["x"].as_f64().unwrap_or(100.0) as i32;
                        let y = pos["y"].as_f64().unwrap_or(100.0) as i32;
                        window
                            .set_position(tauri::Position::Physical(
                                tauri::PhysicalPosition { x, y },
                            ))
                            .ok();
                    }
                }
            }

            // Open the gate after restore so subsequent moves and resizes
            // (real user actions) get saved.
            ready_to_save.store(true, Ordering::Relaxed);

            // Save position on every move and resize. We write on both events
            // because a resize via the window edges can shift the top-left
            // corner without firing a Moved event, depending on which edge
            // the user drags.
            let window_clone = window.clone();
            let config_dir_clone = config_dir.clone();
            let ready_for_handler = ready_to_save.clone();
            window.on_window_event(move |event| {
                if !ready_for_handler.load(Ordering::Relaxed) {
                    return;
                }

                let write_pos = |x: i32, y: i32| {
                    let pos_path = config_dir_clone.join("window_position.json");
                    fs::create_dir_all(&config_dir_clone).ok();
                    let pos_data = serde_json::json!({ "x": x, "y": y });
                    fs::write(&pos_path, pos_data.to_string()).ok();
                };

                if let tauri::WindowEvent::Moved(pos) = event {
                    write_pos(pos.x, pos.y);
                } else if let tauri::WindowEvent::Resized(_) = event {
                    if let Ok(pos) = window_clone.outer_position() {
                        write_pos(pos.x, pos.y);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}