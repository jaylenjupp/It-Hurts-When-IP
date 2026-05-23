// Main Backend - shared across platforms.
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;

use serde::{Deserialize, Serialize};
use tauri::Manager;

mod platform;
mod update;

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
    #[serde(default)]
    pub update_prompt_dismissed_at: Option<i64>,
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
            update_prompt_dismissed_at: None,
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

#[tauri::command]
async fn check_for_update() -> Result<update::UpdateInfo, String> {
    update::check_for_update().await
}

#[tauri::command]
fn dismiss_update_prompt(app: tauri::AppHandle) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock error: {}", e))?
        .as_secs() as i64;

    let mut config = load_config(&app);
    config.update_prompt_dismissed_at = Some(now);
    save_config(&app, &config);
    Ok(())
}

#[tauri::command]
fn get_update_prompt_dismissed_at(app: tauri::AppHandle) -> Option<i64> {
    load_config(&app).update_prompt_dismissed_at
}

// Whether the saved top-left (x, y) lands on a monitor connected *right now*.
//
// Works on both platforms because the saved coords and the monitor rects come
// from the same Tauri physical-pixel coordinate space — so the comparison is
// valid regardless of each OS's native origin convention. The margins keep the
// titlebar grabbable instead of flush against a far/bottom edge.
fn position_on_connected_monitor(window: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    let monitors = match window.available_monitors() {
        Ok(m) if !m.is_empty() => m,
        _ => return false, // no monitor info → don't risk an off-screen restore
    };
    for m in monitors {
        let p = m.position();
        let s = m.size();
        let left = p.x;
        let top = p.y;
        let right = p.x + s.width as i32;
        let bottom = p.y + s.height as i32;
        if x >= left && x <= right - 80 && y >= top && y <= bottom - 40 {
            return true;
        }
    }
    false
}

// --- Entrypoint ---

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_interfaces,
            get_ip_info,
            set_dhcp,
            set_static_ip,
            get_quick_sets,
            save_quick_set,
            save_last_interface,
            get_last_interface,
            check_for_update,
            dismiss_update_prompt,
            get_update_prompt_dismissed_at,
        ])

        .setup(|app| {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let window = app.get_webview_window("main").unwrap();
            let config_dir = app.path().app_data_dir().unwrap();
            let pos_path = config_dir.join("window_position.json");

            // Ignore window events until restore is done, so startup events can't
            // overwrite the saved position before we read it.
            let ready_to_save = Arc::new(AtomicBool::new(false));

            // Restore the saved position — but only if it still falls on a monitor
            // connected right now. If the last display is gone (undocked laptop,
            // swapped monitor) or it's the Windows minimize sentinel, restoring would
            // drop the window into empty space, so we centre instead.
            if pos_path.exists() {
                if let Ok(data) = fs::read_to_string(&pos_path) {
                    if let Ok(pos) = serde_json::from_str::<serde_json::Value>(&data) {
                        let x = pos["x"].as_f64().unwrap_or(100.0) as i32;
                        let y = pos["y"].as_f64().unwrap_or(100.0) as i32;
                        if position_on_connected_monitor(&window, x, y) {
                            window
                                .set_position(tauri::Position::Physical(
                                    tauri::PhysicalPosition { x, y },
                                ))
                                .ok();
                        } else {
                            window.center().ok();
                        }
                    }
                }
            }

            ready_to_save.store(true, Ordering::Relaxed);

            // Persist position on move and resize (a resize via the edges can shift the
            // top-left without firing Moved, depending on which edge is dragged).
            let window_clone = window.clone();
            let config_dir_clone = config_dir.clone();
            let ready_for_handler = ready_to_save.clone();
            window.on_window_event(move |event| {
                if !ready_for_handler.load(Ordering::Relaxed) {
                    return;
                }

                let write_pos = |x: i32, y: i32| {
                    // Never persist the off-screen/minimized sentinel (-32000 on
                    // Windows), or we'd try to restore off-screen next launch.
                    if x <= -10000 || y <= -10000 {
                        return;
                    }
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