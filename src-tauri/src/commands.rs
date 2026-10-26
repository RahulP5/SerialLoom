//! Tauri commands. Every command that may touch the driver or the hubs runs
//! on a blocking worker thread: a UAC prompt can take as long as the user
//! needs, and the UI must stay responsive meanwhile.

use std::sync::Arc;

use serde::Serialize;
use serialloom_core::config::{AppSettings, save_json};
use serialloom_core::model::*;
use serialloom_core::{Engine, Result as CoreResult};
use tauri::State;

use crate::AppState;

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
    state.settings.lock().clone()
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, mut settings: AppSettings) -> CmdResult<DriverInfo> {
    // Choices made in the UI are current-format settings; never migrate them later.
    settings.version = serialloom_core::config::SETTINGS_VERSION;
    save_json(&state.settings_path, &settings).map_err(|e| e.to_string())?;
    let config_dir = state.settings_path.parent().unwrap_or(std::path::Path::new("."));
    let driver = settings.build_driver(config_dir);
    *state.settings.lock() = settings;
    state.engine.set_driver(driver);
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

#[tauri::command]
pub async fn hub_monitor(
    state: State<'_, AppState>,
    id: String,
    after: u64,
) -> CmdResult<Vec<MonitorRecord>> {
    blocking(&state, move |e| e.monitor(&id, after)).await
}
