//! Farm & Lab app: a Tauri shell around the Leptos UI (app/ui).
//!
//! - Desktop: `daemon_api` passes UI requests to pc-daemon with its token
//!   (daemon.rs).
//! - Phone (Android / iOS, or `FARMLAB_PHONE=1` on a computer for testing):
//!   the UI talks to the hub through farmlab-mobile-core (phone.rs).
//! - Both: preferences, system info and the RAM / CPU limits (limits.rs).

mod daemon;
mod limits;
mod phone;

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use tauri::Manager;

/// The phone UI and backend: always on Android / iOS; on a computer when
/// FARMLAB_PHONE is set (to try the phone app against a real hub).
pub fn is_phone() -> bool {
    cfg!(mobile) || std::env::var_os("FARMLAB_PHONE").is_some()
}

#[tauri::command]
fn platform() -> &'static str {
    if is_phone() { "phone" } else { "desktop" }
}

#[tauri::command]
fn has_camera_scanner() -> bool {
    cfg!(mobile)
}

struct PrefsFile(PathBuf);

#[tauri::command]
fn prefs_get(f: tauri::State<'_, PrefsFile>) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(&f.0).ok()?).ok()
}

/// Saves the UI's preferences and applies their limits right away.
#[tauri::command]
fn prefs_set(f: tauri::State<'_, PrefsFile>, prefs: Value) -> Result<limits::Report, String> {
    if let Some(dir) = f.0.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &f.0,
        serde_json::to_vec_pretty(&prefs).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(limits::apply(limits::Limits::from_prefs(&prefs)))
}

#[derive(Serialize)]
struct SystemInfo {
    total_ram_mb: u64,
    cpus: usize,
    os: String,
}

#[tauri::command]
fn system_info() -> SystemInfo {
    SystemInfo {
        total_ram_mb: total_ram_mb(),
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
        os: std::env::consts::OS.into(),
    }
}

#[cfg(desktop)]
fn total_ram_mb() -> u64 {
    let mut s = sysinfo::System::new();
    s.refresh_memory();
    s.total_memory() / (1024 * 1024)
}

/// Phones: unknown (0); Settings then leaves the memory hint out.
#[cfg(mobile)]
fn total_ram_mb() -> u64 {
    0
}

/// The prefs file, also readable before Tauri starts (for the Linux limits
/// relaunch). Same folder as Tauri's app_config_dir for "org.farmlab.app".
fn prefs_path_early() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("org.farmlab.app").join("prefs.json"))
}

/// Before Tauri: on Linux, move into a systemd scope with the saved limits.
pub fn early_limits() {
    if is_phone() {
        return;
    }
    let prefs = prefs_path_early()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null);
    limits::relaunch_in_scope(limits::Limits::from_prefs(&prefs));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .setup(|app| {
            let config = app.path().app_config_dir()?;
            app.manage(PrefsFile(config.join("prefs.json")));
            // Only the phone app has a phone identity to keep.
            if is_phone() {
                app.manage(phone::Phone::load(app.path().app_data_dir()?)?);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            platform,
            has_camera_scanner,
            prefs_get,
            prefs_set,
            system_info,
            daemon::daemon_api,
            phone::phone_state,
            phone::phone_pair_qr,
            phone::phone_pair_manual,
            phone::phone_forget,
            phone::hub_request,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Farm & Lab app");
}
