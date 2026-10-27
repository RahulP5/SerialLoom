mod commands;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serialloom_core::Engine;
use serialloom_core::config::{AppSettings, load_json, save_json};
use tauri::{Emitter, Manager, RunEvent};

pub struct AppState {
    pub engine: Arc<Engine>,
    pub settings: Mutex<AppSettings>,
    pub settings_path: PathBuf,
}

/// How often live hub status is pushed to the UI.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

pub fn run() {
    serialloom_core::logging::init("app");

    let app = tauri::Builder::default()
        // Starting SerialLoom again (Start menu, shortcut) shows the running
        // copy instead of starting a second one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&dir)?;
            let settings_path = dir.join("settings.json");
            let mut settings: AppSettings = load_json(&settings_path);
            if settings.migrate() {
                let _ = save_json(&settings_path, &settings);
            }

            let engine = Arc::new(Engine::new(
                settings.build_driver(&dir),
                Some(dir.join("topology.json")),
            ));
            engine.autostart();

            let handle = app.handle().clone();
            let watcher = Arc::clone(&engine);
            std::thread::Builder::new()
                .name("status-emitter".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(STATUS_INTERVAL);
                        let _ = handle.emit("hub-status", watcher.statuses());
                    }
                })?;

            if let Some(window) = app.get_webview_window("main") {
                fit_to_work_area(&window);
            }

            app.manage(AppState {
                engine,
                settings: Mutex::new(settings),
                settings_path,
            });
            tray::create(app.handle())?;
            Ok(())
        })
        // Closing the window keeps SerialLoom running in the notification
        // area, so programs using the virtual ports are not cut off.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::set_settings,
            commands::driver_info,
            commands::list_ports,
            commands::install_driver,
            commands::diagnostics,
            commands::list_virtual_ports,
            commands::create_virtual_ports,
            commands::rename_virtual_port,
            commands::remove_virtual_port,
            commands::list_hubs,
            commands::create_hub,
            commands::start_hub,
            commands::stop_hub,
            commands::update_hub,
            commands::delete_hub,
            commands::add_member,
            commands::remove_member,
            commands::hub_monitor,
        ])
        .build(tauri::generate_context!())
        .expect("error while building SerialLoom");

    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            state.engine.shutdown();
        }
    });
}

/// Shrinks the window to fit the screen area not covered by the taskbar and
/// centres it there, so dialogs and buttons are never cut off.
fn fit_to_work_area(window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let area = monitor.work_area();
    let Ok(size) = window.outer_size() else { return };
    let margin = 0.96;
    let width = size.width.min((f64::from(area.size.width) * margin) as u32);
    let height = size.height.min((f64::from(area.size.height) * margin) as u32);
    let _ = window.set_size(tauri::PhysicalSize::new(width, height));
    let x = area.position.x + (area.size.width.saturating_sub(width) / 2) as i32;
    let y = area.position.y + (area.size.height.saturating_sub(height) / 2) as i32;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}
