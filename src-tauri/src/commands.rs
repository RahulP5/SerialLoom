//! Tauri commands. Every command that may touch the driver or the hubs runs
//! on a blocking worker thread: a UAC prompt can take as long as the user
//! needs, and the UI must stay responsive meanwhile.

use std::sync::Arc;

use serde::Serialize;
use serialloom_core::config::{AppSettings, save_json};
use serialloom_core::model::*;
use serialloom_core::{Engine, Result as CoreResult};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;

use crate::AppState;
use crate::automation::AutomationInfo;

type CmdResult<T> = Result<T, String>;

async fn blocking<T, F>(state: &State<'_, AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Engine) -> CoreResult<T> + Send + 'static,
{
    let engine = Arc::clone(&state.engine);
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(|e| format!("internal error: {e}"))?
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    name: &'static str,
    version: &'static str,
    repository: &'static str,
    platform: &'static str,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "SerialLoom",
        version: env!("CARGO_PKG_VERSION"),
        repository: env!("CARGO_PKG_REPOSITORY"),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    let mut settings = state.settings.lock().clone();
    // The Windows startup list is the truth (Task Manager can change it).
    #[cfg(windows)]
    {
        settings.start_with_windows = serialloom_core::autostart::is_enabled();
    }
    settings
}

#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettings,
) -> CmdResult<DriverInfo> {
    // Choices made in the UI are current-format settings; never migrate them later.
    settings.version = serialloom_core::config::SETTINGS_VERSION;
    #[cfg(windows)]
    if settings.start_with_windows != serialloom_core::autostart::is_enabled() {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        serialloom_core::autostart::set(settings.start_with_windows, &exe).map_err(|e| e.to_string())?;
    }
    save_json(&state.settings_path, &settings).map_err(|e| e.to_string())?;
    let config_dir = state.settings_path.parent().unwrap_or(std::path::Path::new("."));
    let driver = settings.build_driver(config_dir);
    let automation = settings.automation;
    *state.settings.lock() = settings;
    state.engine.set_driver(driver);
    state.automation.apply(&app, &state.engine, automation);
    Ok(state.engine.driver_info())
}

#[tauri::command]
pub async fn driver_info(state: State<'_, AppState>) -> CmdResult<DriverInfo> {
    blocking(&state, |e| Ok(e.driver_info())).await
}

#[tauri::command]
pub async fn list_ports(state: State<'_, AppState>) -> CmdResult<Vec<PortInfo>> {
    blocking(&state, |e| e.ports()).await
}

#[tauri::command]
pub async fn install_driver(state: State<'_, AppState>) -> CmdResult<DriverInfo> {
    blocking(&state, |e| e.install_driver()).await
}

#[tauri::command]
pub async fn diagnostics(state: State<'_, AppState>) -> CmdResult<String> {
    let report = blocking(&state, |e| Ok(e.diagnostics())).await?;
    Ok(format!(
        "SerialLoom {} ({})
{report}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    ))
}

#[tauri::command]
pub async fn list_virtual_ports(state: State<'_, AppState>) -> CmdResult<Vec<VirtualPortInfo>> {
    blocking(&state, |e| e.virtual_ports()).await
}

#[tauri::command]
pub async fn create_virtual_ports(
    state: State<'_, AppState>,
    names: Vec<String>,
) -> CmdResult<Vec<VirtualPortInfo>> {
    blocking(&state, move |e| e.create_virtual_ports(names)).await
}

#[tauri::command]
pub async fn rename_virtual_port(
    state: State<'_, AppState>,
    name: String,
    new_name: String,
) -> CmdResult<VirtualPortInfo> {
    blocking(&state, move |e| e.rename_virtual_port(&name, &new_name)).await
}

#[tauri::command]
pub async fn remove_virtual_port(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    blocking(&state, move |e| e.remove_virtual_port(&name)).await
}

#[tauri::command]
pub async fn save_topology_backup(app: AppHandle, text: String) -> CmdResult<bool> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = app
            .dialog()
            .file()
            .add_filter("JSON", &["json"])
            .set_file_name("serialloom-topology.json")
            .blocking_save_file();
        let Some(path) = path else { return Ok(false) };
        let path = path.into_path().map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("internal error: {e}"))?
}

#[tauri::command]
pub async fn list_hubs(state: State<'_, AppState>) -> CmdResult<Vec<HubView>> {
    blocking(&state, |e| Ok(e.hubs())).await
}

#[tauri::command]
pub async fn create_hub(state: State<'_, AppState>, request: CreateHubRequest) -> CmdResult<HubView> {
    blocking(&state, move |e| e.create_hub(request)).await
}

#[tauri::command]
pub async fn start_hub(state: State<'_, AppState>, id: String) -> CmdResult<HubView> {
    blocking(&state, move |e| e.start_hub(&id)).await
}

#[tauri::command]
pub async fn stop_hub(state: State<'_, AppState>, id: String) -> CmdResult<HubView> {
    blocking(&state, move |e| e.stop_hub(&id)).await
}

#[tauri::command]
pub async fn update_hub(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    autostart: Option<bool>,
) -> CmdResult<HubView> {
    blocking(&state, move |e| e.update_hub(&id, name, autostart)).await
}

#[tauri::command]
pub async fn delete_hub(state: State<'_, AppState>, id: String, remove_virtual_ports: bool) -> CmdResult<()> {
    blocking(&state, move |e| e.delete_hub(&id, remove_virtual_ports)).await
}

#[tauri::command]
pub async fn add_member(
    state: State<'_, AppState>,
    id: String,
    request: EndpointRequest,
) -> CmdResult<HubView> {
    blocking(&state, move |e| e.add_member(&id, request)).await
}

#[tauri::command]
pub async fn remove_member(
    state: State<'_, AppState>,
    id: String,
    index: usize,
    remove_virtual_port: bool,
) -> CmdResult<HubView> {
    blocking(&state, move |e| e.remove_member(&id, index, remove_virtual_port)).await
}

// The window passes each argument by name, so they stay separate.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn open_trace_window(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    index: usize,
    source: String,
    target: String,
    forward_rx: bool,
    file_mode: bool,
    usb_probe: Option<bool>,
) -> CmdResult<()> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hash = DefaultHasher::new();
    let usb_probe = usb_probe.unwrap_or(false);
    (&id, index, usb_probe).hash(&mut hash);
    let label = format!("trace-{:016x}", hash.finish());
    if let Some(window) = app.get_webview_window(&label) {
        window.unminimize().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }

    let options = serde_json::json!({
        "id": id, "index": index, "source": source, "target": target,
        "forwardRx": forward_rx, "fileMode": file_mode, "usbProbe": usb_probe,
    });
    let parent = app
        .get_webview_window("main")
        .ok_or("main window is unavailable")?;
    let window = WebviewWindowBuilder::new(&app, label, WebviewUrl::App("index.html".into()))
        .parent(&parent)
        .map_err(|error| error.to_string())?
        .title("SerialLoom — Trace")
        .inner_size(1050.0, 700.0)
        .min_inner_size(600.0, 440.0)
        .initialization_script(format!("window.__SERIALLOOM_TRACE__ = {options};"))
        .build()
        .map_err(|error| error.to_string())?;

    let engine = Arc::clone(&state.engine);
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            let engine = Arc::clone(&engine);
            let id = id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if usb_probe {
                    #[cfg(windows)]
                    USB_PROBES.lock().remove(&id);
                } else {
                    let _ =
                        engine.capture_link(&id, index, serialloom_core::capture::CaptureAction::CloseWindow);
                }
            });
        }
    });
    Ok(())
}

#[cfg(windows)]
static USB_PROBES: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, serialloom_core::usb_probe::UsbProbe>>,
> = std::sync::LazyLock::new(Default::default);

#[tauri::command]
pub async fn capture_usb_probe(
    id: String,
    action: serialloom_core::capture::CaptureAction,
) -> CmdResult<serde_json::Value> {
    #[cfg(windows)]
    {
        use serialloom_core::capture::{CaptureAction, CaptureStatus};
        tauri::async_runtime::spawn_blocking(move || {
            let mut probes = USB_PROBES.lock();
            match action {
                CaptureAction::Start(request) => {
                    if probes.get(&id).is_some_and(|probe| probe.running()) {
                        return Err("USB capture is already running".into());
                    }
                    let probe = serialloom_core::usb_probe::UsbProbe::start(&id, request)
                        .map_err(|error| error.to_string())?;
                    probes.insert(id.clone(), probe);
                }
                CaptureAction::Stop | CaptureAction::CloseWindow => {
                    if let Some(probe) = probes.get(&id) {
                        probe.stop();
                    }
                }
                CaptureAction::Status | CaptureAction::Summary => {}
            }
            Ok(probes.get(&id).map_or_else(
                || serde_json::json!(CaptureStatus::idle()),
                |probe| probe.status(),
            ))
        })
        .await
        .map_err(|error| error.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (id, action);
        Err("USBPcap requires Windows".into())
    }
}

#[tauri::command]
pub async fn capture_link(
    state: State<'_, AppState>,
    id: String,
    index: usize,
    action: serialloom_core::capture::CaptureAction,
) -> CmdResult<serialloom_core::capture::CaptureStatus> {
    blocking(&state, move |e| e.capture_link(&id, index, action)).await
}

#[tauri::command]
pub async fn hub_monitor(
    state: State<'_, AppState>,
    id: String,
    after: u64,
) -> CmdResult<Vec<MonitorRecord>> {
    blocking(&state, move |e| e.monitor(&id, after)).await
}

#[tauri::command]
pub fn automation_info(state: State<'_, AppState>) -> AutomationInfo {
    state.automation.info()
}

/// SerialLoom Helper: whether it is installed and running.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperInfo {
    supported: bool,
    installed: bool,
    running: bool,
}

#[cfg(windows)]
fn helper_info() -> HelperInfo {
    let st = serialloom_core::driver::native::helper::status();
    HelperInfo {
        supported: true,
        installed: st.installed,
        running: st.running,
    }
}

#[cfg(not(windows))]
fn helper_info() -> HelperInfo {
    HelperInfo::default()
}

#[tauri::command]
pub async fn helper_status() -> CmdResult<HelperInfo> {
    tauri::async_runtime::spawn_blocking(helper_info)
        .await
        .map_err(|e| format!("internal error: {e}"))
}

/// Installs or removes SerialLoom Helper. This always asks for administrator
/// permission (UAC): the helper never installs or removes itself.
async fn change_helper(install: bool) -> CmdResult<HelperInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        {
            use serialloom_core::driver::native::elevate::{AdminOp, run};
            run(if install {
                AdminOp::InstallService
            } else {
                AdminOp::UninstallService
            })
            .map_err(|e| e.to_string())?;
            Ok(helper_info())
        }
        #[cfg(not(windows))]
        {
            let _ = install;
            Err("SerialLoom Helper is available on Windows only".to_string())
        }
    })
    .await
    .map_err(|e| format!("internal error: {e}"))?
}

#[tauri::command]
pub async fn install_helper() -> CmdResult<HelperInfo> {
    change_helper(true).await
}

#[tauri::command]
pub async fn remove_helper() -> CmdResult<HelperInfo> {
    change_helper(false).await
}
