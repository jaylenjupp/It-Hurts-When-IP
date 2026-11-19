// Main Backend - shared across platforms.
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager};

mod build_info;
mod config;
mod platform;
mod update;

use config::{AppConfig, QuickSetProfile};

// --- Persistent config (model + load/save live in config.rs) ---

/// Serialises load → modify → save so two commands can't lose each other's
/// changes.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn get_config_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap().join("config.json")
}

/// Read-only load. An unreadable file is moved aside and defaults are used.
fn load_config(app: &tauri::AppHandle) -> AppConfig {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    config::load_from(&get_config_path(app)).config
}

/// Load → change → save, keeping every other field as it was.
fn update_config(
    app: &tauri::AppHandle,
    change: impl FnOnce(&mut AppConfig),
) -> Result<(), String> {
    let path = get_config_path(app);
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = config::load_for_update(&path)?;
    change(&mut cfg);
    config::save_to(&path, &cfg)
}

#[derive(Serialize)]
struct Settings {
    advanced_mode: bool,
    check_updates_on_launch: bool,
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

/// Whether the privileged helper is installed, running, reachable from this
/// account and the same version as the app. Read-only; release builds ping
/// the helper (2 s timeout), debug builds never contact it.
#[tauri::command]
async fn get_helper_status() -> Result<platform::HelperStatus, String> {
    tauri::async_runtime::spawn_blocking(platform::get_helper_status)
        .await
        .map_err(|e| format!("The helper status could not be checked: {}", e))
}

/// Read-only diagnostics text (and fields) for Settings → Maintenance →
/// Copy diagnostics. Release builds ask the helper for its journal status
/// (2 s timeout); debug builds never contact it.
#[tauri::command]
async fn get_diagnostics(
    app: tauri::AppHandle,
    selected_service: Option<String>,
) -> Result<platform::Diagnostics, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let config = load_config(&app);
        platform::get_diagnostics(
            selected_service,
            platform::DiagnosticsConfig {
                config_path: get_config_path(&app),
                advanced_mode: config.advanced_mode,
                check_updates_on_launch: config.check_updates_on_launch,
            },
        )
    })
    .await
    .map_err(|e| format!("Diagnostics could not be collected: {}", e))
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
    let profile = QuickSetProfile {
        name,
        ip,
        subnet,
        gateway,
    };
    update_config(&app, |config| match slot {
        1 => config.quick_set_1 = profile,
        2 => config.quick_set_2 = profile,
        3 => config.quick_set_3 = profile,
        _ => config.quick_set_4 = profile,
    })?;
    Ok(String::from("Saved"))
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Settings {
    let config = load_config(&app);
    Settings {
        advanced_mode: config.advanced_mode,
        check_updates_on_launch: config.check_updates_on_launch,
    }
}

#[tauri::command]
fn set_advanced_mode(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    update_config(&app, |config| config.advanced_mode = enabled)
}

#[tauri::command]
fn set_check_updates_on_launch(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    update_config(&app, |config| config.check_updates_on_launch = enabled)
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
    let path = get_config_path(&app)
        .parent()
        .unwrap()
        .join("last_interface.json");
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

    update_config(&app, |config| config.update_prompt_dismissed_at = Some(now))
}

#[tauri::command]
fn get_update_prompt_dismissed_at(app: tauri::AppHandle) -> Option<i64> {
    load_config(&app).update_prompt_dismissed_at
}

#[tauri::command]
fn get_build_info() -> build_info::BuildInfo {
    build_info::build_info()
}

// --- Uninstall (rescue script; never the helper) ---

/// Seconds after a run that started stage B before the app quits on its own,
/// even if the window never asks to (the UI counts down 60 s first). The
/// rescue script's stage B gives up after 300 s if the app is still open.
const UNINSTALL_QUIT_BACKSTOP_SECS: u64 = 150;

/// Read-only plan for the given network choices.
#[tauri::command]
async fn rescue_plan(
    dhcp: Vec<String>,
    restore_journal: bool,
) -> Result<platform::RescuePlan, String> {
    tauri::async_runtime::spawn_blocking(move || platform::rescue_plan(dhcp, restore_journal))
        .await
        .map_err(|e| format!("The plan could not be made: {}", e))?
}

/// Re-checks the choices against a fresh plan, then runs the uninstall with
/// one macOS administrator prompt (release builds; debug builds only return
/// the command). If stage B started, the app quits by itself after
/// UNINSTALL_QUIT_BACKSTOP_SECS at the latest.
#[tauri::command]
async fn rescue_uninstall(
    app: tauri::AppHandle,
    dhcp: Vec<String>,
    restore_journal: bool,
) -> Result<platform::RescueRun, String> {
    let run = tauri::async_runtime::spawn_blocking(move || {
        platform::rescue_uninstall(dhcp, restore_journal)
    })
    .await
    .map_err(|e| format!("The uninstall could not be started: {}", e))??;
    if run.quit_required() {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(UNINSTALL_QUIT_BACKSTOP_SECS));
            app.exit(0);
        });
    }
    Ok(run)
}

// --- Repair network (rescue script; never the helper) ---

/// Settings → Advanced gates the full network reset in the app (owner
/// decision P4-D1). The rescue script's own Terminal `--repair full` is not
/// gated: recovery never depends on the app.
fn full_reset_gate(tier: &str, advanced_mode: bool) -> Result<(), String> {
    if tier == "full" && !advanced_mode {
        return Err(String::from(
            "Full network reset is only available with Advanced mode on (Settings → Advanced).",
        ));
    }
    Ok(())
}

/// Read-only repair plan: tier "soft" | "service" (with `service`) | "full".
/// "full" needs Advanced mode on.
#[tauri::command]
async fn repair_plan(
    app: tauri::AppHandle,
    tier: String,
    service: Option<String>,
) -> Result<platform::RepairPlan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        full_reset_gate(&tier, load_config(&app).advanced_mode)?;
        platform::repair_plan(tier, service)
    })
    .await
    .map_err(|e| format!("The repair plan could not be made: {}", e))?
}

/// Re-checks the tier against a fresh plan (a full reset also needs
/// `confirm` == the typed word RESET), then runs the repair with one macOS
/// administrator prompt (release builds; debug builds only return the
/// command). Never restarts the Mac. "full" needs Advanced mode on; with it
/// off nothing runs.
#[tauri::command]
async fn repair_run(
    app: tauri::AppHandle,
    tier: String,
    service: Option<String>,
    confirm: String,
) -> Result<platform::RepairRun, String> {
    tauri::async_runtime::spawn_blocking(move || {
        full_reset_gate(&tier, load_config(&app).advanced_mode)?;
        platform::repair_run(tier, service, confirm)
    })
    .await
    .map_err(|e| format!("The repair could not be started: {}", e))?
}

/// "Finish and quit" after an uninstall, so the rescue script can remove the
/// app bundle.
#[tauri::command]
fn rescue_quit(app: tauri::AppHandle) {
    app.exit(0);
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
        .menu(platform::app_menu)
        .on_menu_event(|app, event| {
            // The menu items only open Settings or the Uninstall / Repair
            // window; nothing else happens until the user confirms there.
            if event.id() == platform::MENU_SETTINGS_ID {
                app.emit("settings-open", ()).ok();
            } else if event.id() == platform::MENU_UNINSTALL_ID {
                app.emit("rescue-open", ()).ok();
            } else if event.id() == platform::MENU_REPAIR_ID {
                app.emit("repair-open", ()).ok();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_interfaces,
            get_ip_info,
            set_dhcp,
            set_static_ip,
            get_helper_status,
            get_diagnostics,
            get_quick_sets,
            save_quick_set,
            get_settings,
            set_advanced_mode,
            set_check_updates_on_launch,
            save_last_interface,
            get_last_interface,
            check_for_update,
            dismiss_update_prompt,
            get_update_prompt_dismissed_at,
            get_build_info,
            rescue_plan,
            rescue_uninstall,
            rescue_quit,
            repair_plan,
            repair_run,
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
                                .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                                    x,
                                    y,
                                }))
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

#[cfg(test)]
mod tests {
    use super::full_reset_gate;

    #[test]
    fn full_reset_refused_with_advanced_off() {
        let err = full_reset_gate("full", false).unwrap_err();
        assert_eq!(
            err,
            "Full network reset is only available with Advanced mode on (Settings → Advanced)."
        );
    }

    #[test]
    fn full_reset_allowed_with_advanced_on() {
        assert_eq!(full_reset_gate("full", true), Ok(()));
    }

    #[test]
    fn soft_and_service_allowed_either_way() {
        for tier in ["soft", "service"] {
            assert_eq!(full_reset_gate(tier, false), Ok(()));
            assert_eq!(full_reset_gate(tier, true), Ok(()));
        }
    }
}
