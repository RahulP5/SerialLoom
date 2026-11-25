//! Notification-area icon. Closing the window only hides it, so virtual
//! ports and connections keep running. "Quit SerialLoom" in the icon's menu
//! stops every connection, removes the virtual ports and exits.
//!
//! The window sends the menu's wording in the user's language
//! ([`set_tray_text`]); English is used until it does.

use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::AppState;

const REFRESH: Duration = Duration::from_secs(2);

/// Menu wording. `one` and `other` are templates with `{running}` and
/// `{count}`, chosen by the number of hubs.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayText {
    open: String,
    quit: String,
    starting: String,
    no_hubs: String,
    one: String,
    other: String,
}

impl Default for TrayText {
    fn default() -> Self {
        Self {
            open: "Open SerialLoom".into(),
            quit: "Quit SerialLoom (closes all ports)".into(),
            starting: "Starting…".into(),
            no_hubs: "No hubs yet".into(),
            one: "{running} of {count} hub running".into(),
            other: "{running} of {count} hubs running".into(),
        }
    }
}

struct Tray {
    open: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    text: Mutex<TrayText>,
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let text = TrayText::default();
    let open = MenuItem::with_id(app, "open", &text.open, true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", &text.starting, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", &text.quit, true, None::<&str>)?;
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
    app.manage(Tray {
        open,
        quit,
        text: Mutex::new(text),
    });

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
                if let (Some(state), Some(labels)) = (app.try_state::<AppState>(), app.try_state::<Tray>()) {
                    let text = summary(&state, &labels.text.lock());
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

fn summary(state: &AppState, text: &TrayText) -> String {
    let statuses = state.engine.statuses();
    let running = statuses.iter().filter(|s| s.running).count();
    let template = match statuses.len() {
        0 => return text.no_hubs.clone(),
        1 => &text.one,
        _ => &text.other,
    };
    template
        .replace("{running}", &running.to_string())
        .replace("{count}", &statuses.len().to_string())
}

/// Shows the menu in the language the window uses. The status line follows
/// within one refresh.
#[tauri::command]
pub fn set_tray_text(app: AppHandle, text: TrayText) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let _ = tray.open.set_text(&text.open);
    let _ = tray.quit.set_text(&text.quit);
    *tray.text.lock() = text;
}
