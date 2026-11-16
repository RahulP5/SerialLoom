//! Turns the automation API on and off as the setting changes, and tells the
//! window when another program changed ports or connections.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use serialloom_core::Engine;
use tauri::AppHandle;

/// Event sent to the window after an automation command changed something.
pub const TOPOLOGY_CHANGED: &str = "topology-changed";

#[derive(Default)]
pub struct Automation {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    #[cfg(windows)]
    server: Option<serialloom_core::automation::AutomationServer>,
    error: Option<String>,
}

/// What the Settings page shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationInfo {
    pub running: bool,
    pub pipe: Option<String>,
    pub error: Option<String>,
    /// Full path of serialloom-cli.exe when it is installed next to the app.
    pub cli_path: Option<String>,
}

impl Automation {
    /// Starts or stops the API to match `enabled`.
    pub fn apply(&self, app: &AppHandle, engine: &Arc<Engine>, enabled: bool) {
        let mut inner = self.inner.lock();
        inner.error = None;
        if !enabled {
            #[cfg(windows)]
            {
                inner.server = None;
            }
            return;
        }
        #[cfg(windows)]
        {
            use tauri::Emitter;

            if inner.server.is_some() {
                return;
            }
            let handle = app.clone();
            let on_change: serialloom_core::automation::ChangeHook = Arc::new(move || {
                let _ = handle.emit(TOPOLOGY_CHANGED, ());
            });
            match serialloom_core::automation::AutomationServer::start(Arc::clone(engine), on_change) {
                Ok(server) => inner.server = Some(server),
                Err(e) => {
                    tracing::error!("{e}");
                    inner.error = Some(e.to_string());
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (app, engine);
            inner.error = Some("the automation API is available on Windows only".into());
        }
    }

    pub fn stop(&self) {
        #[cfg(windows)]
        {
            self.inner.lock().server = None;
        }
    }

    pub fn info(&self) -> AutomationInfo {
        let inner = self.inner.lock();
        #[cfg(windows)]
        let pipe = inner.server.as_ref().map(|s| s.pipe_name().to_string());
        #[cfg(not(windows))]
        let pipe: Option<String> = None;
        let cli_path = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join("serialloom-cli.exe")))
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string());
        AutomationInfo {
            running: pipe.is_some(),
            pipe,
            error: inner.error.clone(),
            cli_path,
        }
    }
}
