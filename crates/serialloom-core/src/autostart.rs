//! "Start SerialLoom when I sign in": a value in the current user's Run key
//! that starts the app in the background (notification area only).
//!
//! The value is named after the product, which is also what the uninstaller
//! removes. Windows' own switch for it (Task Manager > Startup apps) lives in
//! `StartupApproved\Run`; a value disabled there counts as off, and turning
//! the setting on clears that switch so it really starts.

use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};

use crate::error::{Error, Result};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE: &str = "SerialLoom";

/// Command-line flag: start hidden in the notification area.
pub const BACKGROUND_FLAG: &str = "--background";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get(key: &str, flags: u32, buf: &mut [u8]) -> std::result::Result<usize, u32> {
    let (key, value) = (wide(key), wide(VALUE));
    let mut len = buf.len() as u32;
    // SAFETY: strings are NUL-terminated; len is the buffer size in bytes.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            flags,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(len as usize)
    } else {
        Err(rc)
    }
}

/// The command the Run value holds, if any.
pub fn command() -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let len = get(RUN_KEY, RRF_RT_REG_SZ, &mut buf).ok()?;
    let units: Vec<u16> = buf[..len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Windows' Startup apps switch: the first byte is odd (2 = on) or 3 = off.
fn disabled_by_windows() -> bool {
    let mut buf = [0u8; 12];
    get(APPROVED_KEY, RRF_RT_REG_BINARY, &mut buf).is_ok_and(|n| n > 0 && buf[0] == 3)
}

/// True when SerialLoom will start at sign-in.
pub fn is_enabled() -> bool {
    command().is_some() && !disabled_by_windows()
}

/// Run-key command for `exe`.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" {BACKGROUND_FLAG}", exe.display())
}

/// Turns starting at sign-in on (for `exe`) or off.
pub fn set(enabled: bool, exe: &Path) -> Result<()> {
    let value = wide(VALUE);
    if !enabled {
        let key = wide(RUN_KEY);
        // SAFETY: strings are NUL-terminated.
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        return match rc {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(Error::Driver(format!(
                "could not turn off starting with Windows (error {e})"
            ))),
        };
    }
    let key = wide(RUN_KEY);
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: out-pointer to a key handle; closed below.
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(Error::Driver(format!(
            "could not open the startup list (error {rc})"
        )));
    }
    let data = wide(&command_for(exe));
    // SAFETY: data is a NUL-terminated UTF-16 string of the byte length passed.
    let rc = unsafe {
        let rc = RegSetValueExW(
            hkey,
            value.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        );
        RegCloseKey(hkey);
        rc
    };
    if rc != ERROR_SUCCESS {
        return Err(Error::Driver(format!(
            "could not turn on starting with Windows (error {rc})"
        )));
    }
    // Clear a "disabled" switch from Task Manager so it really starts.
    let approved = wide(APPROVED_KEY);
    // SAFETY: strings are NUL-terminated; a missing value is fine.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, approved.as_ptr(), value.as_ptr()) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_quotes_the_path_and_starts_hidden() {
        assert_eq!(
            command_for(Path::new(r"C:\Program Files\SerialLoom\serialloom.exe")),
            r#""C:\Program Files\SerialLoom\serialloom.exe" --background"#
        );
    }
}
