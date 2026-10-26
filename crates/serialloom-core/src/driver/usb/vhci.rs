//! Direct client for the usbip-win2 virtual host controller (VHCI) driver.
//!
//! Mirrors the public interface in usbip-win2's `include/usbip/vhci.h`
//! (BSD-2-Clause): open the driver's device interface and send
//! `PLUGIN_HARDWARE_ONCE` / `PLUGOUT_HARDWARE`. Talking to the driver directly
//! means SerialLoom does not ship `usbip.exe` or the Visual C++ runtime.

use std::io;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
    CR_SUCCESS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// `GUID_DEVINTERFACE_USBIP_VHCI` = {B4030C06-DC5F-4FCC-87EB-E5515A0935C0}
const GUID_VHCI: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0xB403_0C06,
    data2: 0xDC5F,
    data3: 0x4FCC,
    data4: [0x87, 0xEB, 0xE5, 0x51, 0x5A, 0x09, 0x35, 0xC0],
};

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA)`
const fn ioctl(function: u32) -> u32 {
    (0x22 << 16) | (0x3 << 14) | (function << 2)
}
const PLUGOUT_HARDWARE: u32 = ioctl(0x801);
const PLUGIN_HARDWARE_ONCE: u32 = ioctl(0x806);

const BUS_ID_SIZE: usize = 32;
const SERVICE_SIZE: usize = 32;
const HOST_SIZE: usize = 1025;
const SERIAL_BUFSZ: usize = 16;

/// `ioctl::plugin_hardware : base, imported_device_location { serial; wsk_events; }`.
///
/// C++ lays out the `imported_device_location` base as a complete object,
/// including its tail padding (1097 bytes rounded up to 1100), before the
/// derived members. `_location_pad` reproduces that padding.
#[repr(C)]
struct PluginHardware {
    size: u32,
    // imported_device_location
    port: i32,
    location_hash: u32,
    busid: [u8; BUS_ID_SIZE],
    service: [u8; SERVICE_SIZE],
    host: [u8; HOST_SIZE],
    _location_pad: [u8; 3],
    // plugin_hardware
    serial: [u8; SERIAL_BUFSZ],
    wsk_events: u8,
}

/// `ioctl::plugout_hardware`.
#[repr(C)]
struct PlugoutHardware {
    size: u32,
    port: i32,
}

fn copy_str(dst: &mut [u8], s: &str) -> io::Result<()> {
    let b = s.as_bytes();
    if b.len() >= dst.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{s}' is too long"),
        ));
    }
    dst[..b.len()].copy_from_slice(b);
    Ok(())
}

/// Open handle to the virtual host controller.
pub struct Vhci(HANDLE);

// SAFETY: a device HANDLE can be used from any thread.
unsafe impl Send for Vhci {}
// SAFETY: DeviceIoControl calls on one handle may run concurrently.
unsafe impl Sync for Vhci {}

impl Vhci {
    pub fn open() -> io::Result<Self> {
        let mut len = 0u32;
        // SAFETY: plain size query.
        let cr = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut len,
                &GUID_VHCI,
                std::ptr::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if cr != CR_SUCCESS || len <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the virtual USB controller is not running",
            ));
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: buffer holds `len` characters.
        let cr = unsafe {
            CM_Get_Device_Interface_ListW(
                &GUID_VHCI,
                std::ptr::null(),
                buf.as_mut_ptr(),
                len,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if cr != CR_SUCCESS || end == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the virtual USB controller is not running",
            ));
        }
        buf.truncate(end);
        buf.push(0);
        // SAFETY: path is NUL-terminated; the handle is closed on drop.
        let h = unsafe {
            CreateFileW(
                buf.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(h))
    }

    /// Plugs in the USB/IP device `busid` served at `host:port`.
    /// Returns the virtual hub port number (>= 1).
    pub fn attach(&self, host: &str, port: u16, busid: &str) -> io::Result<i32> {
        // SAFETY: plain C struct; all-zero is a valid initial state.
        let mut r: PluginHardware = unsafe { std::mem::zeroed() };
        r.size = size_of::<PluginHardware>() as u32;
        copy_str(&mut r.busid, busid)?;
        copy_str(&mut r.service, &port.to_string())?;
        copy_str(&mut r.host, host)?;
        // Serial data is small and frequent: use the low-latency receive mode.
        r.wsk_events = 1;

        let out_len = 2 * size_of::<u32>() as u32; // up to and including `port`
        let mut returned = 0u32;
        let p = std::ptr::from_mut(&mut r).cast::<core::ffi::c_void>();
        // SAFETY: `r` is a valid buffer for both input and output.
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                PLUGIN_HARDWARE_ONCE,
                p,
                size_of::<PluginHardware>() as u32,
                p,
                out_len,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // SAFETY: reads this thread's last error.
            let err = unsafe { GetLastError() };
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        if returned != out_len || r.port < 1 {
            return Err(io::Error::other(
                "the virtual USB controller gave an unexpected reply",
            ));
        }
        Ok(r.port)
    }

    /// Unplugs the device on hub port `port`.
    pub fn detach(&self, port: i32) -> io::Result<()> {
        let mut r = PlugoutHardware {
            size: size_of::<PlugoutHardware>() as u32,
            port,
        };
        let mut returned = 0u32;
        // SAFETY: `r` is a valid input buffer.
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                PLUGOUT_HARDWARE,
                std::ptr::from_mut(&mut r).cast(),
                size_of::<PlugoutHardware>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Vhci {
    fn drop(&mut self) {
        // SAFETY: closes the handle opened in `open`.
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_driver_header() {
        // base (4) + imported_device_location (4+4+32+32+1025 = 1097, padded to 1100)
        // + serial (16) + wsk_events (1) = 1121, padded to 1124 = MSVC sizeof.
        assert_eq!(size_of::<PluginHardware>(), 1124);
        assert_eq!(std::mem::offset_of!(PluginHardware, port), 4);
        assert_eq!(std::mem::offset_of!(PluginHardware, host), 76);
        assert_eq!(std::mem::offset_of!(PluginHardware, serial), 1104);
        assert_eq!(std::mem::offset_of!(PluginHardware, wsk_events), 1120);
        assert_eq!(size_of::<PlugoutHardware>(), 8);
    }

    #[test]
    fn ioctl_codes() {
        assert_eq!(PLUGIN_HARDWARE_ONCE, 0x0022_E018);
        assert_eq!(PLUGOUT_HARDWARE, 0x0022_E004);
    }
}
