//! Notification-area icon. Closing the window only hides it, so virtual
//! ports and connections keep running. "Quit SerialLoom" in the icon's menu
//! stops every connection, removes the virtual ports and exits.

use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::AppState;

const REFRESH: Duration = Duration::from_secs(2);

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open SerialLoom", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "Quit SerialLoom (closes all ports)",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("SerialLoom")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "open" => show_main_window(app),
            "quit" => {
                tracing::info!("quit from the notification area");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;

    // Keep the menu and tooltip in step with the running hubs.
    let app = app.clone();
    std::thread::Builder::new()
        .name("tray-status".into())
        .spawn(move || {
            let mut shown = String::new();
            loop {
                if let Some(state) = app.try_state::<AppState>() {
                    let text = summary(&state);
                    if text != shown {
                        let _ = status.set_text(&text);
                        let _ = tray.set_tooltip(Some(format!("SerialLoom · {text}")));
                        shown = text;
                    }
                }
                std::thread::sleep(REFRESH);
            }
        })?;
    Ok(())
}

fn summary(state: &AppState) -> String {
    let statuses = state.engine.statuses();
    let running = statuses.iter().filter(|s| s.running).count();
    match (running, statuses.len()) {
        (_, 0) => "No hubs yet".into(),
        (r, 1) => format!("{r} of 1 hub running"),
        (r, n) => format!("{r} of {n} hubs running"),
    }
}
