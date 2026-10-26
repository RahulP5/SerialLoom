//! Settings and topology persistence.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::driver::PortDriver;
use crate::driver::demo::DemoDriver;
#[cfg(windows)]
use crate::driver::native::NativeDriver;
#[cfg(windows)]
use crate::driver::usb::UsbDriver;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverKind {
    /// Software USB serial devices on the usbip-win2 virtual USB controller
    /// (Microsoft-signed drivers, works on any PC).
    #[default]
    Usb,
    /// SerialLoom's own UMDF driver (needs Microsoft signing to load).
    Native,
    Demo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub driver: DriverKind,
    /// Settings format version. Files written before versioning existed
    /// have no such field and count as version 1.
    #[serde(default = "version_one")]
    pub version: u32,
}

/// Current settings format.
pub const SETTINGS_VERSION: u32 = 2;

fn version_one() -> u32 {
    1
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            driver: if cfg!(windows) {
                DriverKind::Usb
            } else {
                DriverKind::Demo
            },
            version: SETTINGS_VERSION,
        }
    }
}

impl AppSettings {
    /// Upgrades settings saved by older versions. Returns true if changed.
    /// Version 1 defaulted to SerialLoom's own driver, which cannot load
    /// without a Microsoft signature; those users move to virtual USB ports.
    pub fn migrate(&mut self) -> bool {
        if self.version >= SETTINGS_VERSION {
            return false;
        }
        if self.driver == DriverKind::Native {
            self.driver = DriverKind::Usb;
        }
        self.version = SETTINGS_VERSION;
        true
    }

    /// `config_dir` is where drivers keep their own state.
    pub fn build_driver(&self, config_dir: &Path) -> Arc<dyn PortDriver> {
        match self.driver {
            #[cfg(windows)]
            DriverKind::Usb => UsbDriver::new(config_dir),
            #[cfg(windows)]
            DriverKind::Native => Arc::new(NativeDriver::new()),
            #[cfg(not(windows))]
            DriverKind::Usb | DriverKind::Native => {
                let _ = config_dir;
                Arc::new(DemoDriver::new())
            }
            DriverKind::Demo => Arc::new(DemoDriver::new()),
        }
    }
}

/// Reads JSON, falling back to the default when the file is missing or broken.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

/// Writes JSON through a temporary file so a crash never leaves half a file.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_old_native_default_to_usb() {
        // A file written before versioning: no "version" field.
        let mut old: AppSettings = serde_json::from_str(r#"{"driver":"native"}"#).unwrap();
        assert_eq!(old.version, 1);
        assert!(old.migrate());
        assert_eq!(old.driver, DriverKind::Usb);
        assert!(!old.migrate(), "migrating twice changes nothing");

        // Someone who picks the native driver deliberately keeps it.
        let mut chosen = AppSettings {
            driver: DriverKind::Native,
            version: SETTINGS_VERSION,
        };
        assert!(!chosen.migrate());
        assert_eq!(chosen.driver, DriverKind::Native);
    }

    #[test]
    fn fresh_settings_use_usb_on_windows() {
        if cfg!(windows) {
            assert_eq!(AppSettings::default().driver, DriverKind::Usb);
        }
    }
}
