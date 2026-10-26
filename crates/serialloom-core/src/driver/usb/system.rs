//! Windows plumbing for the USB/IP backend: the bundled usbip-win2 files,
//! installing/removing its signed drivers, and finding the COM port Windows
//! assigned to one of our virtual devices.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS;

use crate::driver::com_number;
use crate::driver::native::setup::{
    ComDb, DevInfo, contains_ascii_ci, device_state, from_wide, hardware_ids, instance_id, last_error,
    new_devinfo_data, read_port_name, remove_device, restart, set_string_property, wide, win_err,
    write_port_name,
};
use crate::error::{Error, Result};
use crate::model::VirtualPortState;

/// Hardware ID of the usbip-win2 virtual host controller (root device).
pub const VHCI_HWID: &str = r"ROOT\USBIP_WIN2\UDE";
const UDE_INF: &str = "usbip2_ude.inf";
const FILTER_INF: &str = "usbip2_filter.inf";
const DRIVER_FILES: [&str; 6] = [
    "usbip2_ude.inf",
    "usbip2_ude.sys",
    "usbip2_ude.cat",
    "usbip2_filter.inf",
    "usbip2_filter.sys",
    "usbip2_filter.cat",
];

/// Folder holding the bundled usbip-win2 files (`driver\…`, license).
pub fn bundled_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("usbip"), dir.join("resources").join("usbip")];
    if cfg!(debug_assertions) {
        // Development: target/debug/serialloom.exe -> repo root.
        if let Some(repo) = dir.parent().and_then(Path::parent) {
            candidates.push(repo.join("vendor").join("usbip-win2"));
        }
    }
    candidates
        .into_iter()
        .find(|d| d.join("driver").join(UDE_INF).is_file())
}

/// The bundled driver files are complete.
pub fn drivers_bundled() -> bool {
    bundled_dir().is_some_and(|d| DRIVER_FILES.iter().all(|f| d.join("driver").join(f).is_file()))
}

fn all_present_devices() -> Result<DevInfo> {
    // SAFETY: plain query; the result is checked below.
    let set = unsafe {
        SetupDiGetClassDevsW(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_ALLCLASSES | DIGCF_PRESENT,
        )
    };
    if set == -1 {
        return Err(win_err("listing devices", last_error()));
    }
    Ok(DevInfo(set))
}

/// Calls `f` for every present device with hardware ID `hwid`.
fn each_with_hwid(hwid: &str, mut f: impl FnMut(&DevInfo, &SP_DEVINFO_DATA)) -> Result<()> {
    let set = all_present_devices()?;
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        if hardware_ids(&set, &data)
            .iter()
            .any(|id| id.eq_ignore_ascii_case(hwid))
        {
            f(&set, &data);
        }
    }
    Ok(())
}

/// State of the virtual USB host controller, if it exists.
pub fn vhci_state() -> Option<(VirtualPortState, Option<String>)> {
    let mut found = None;
    let _ = each_with_hwid(VHCI_HWID, |_, data| {
        let st = device_state(data.DevInst);
        found = Some((st.state, st.text));
    });
    found
}

fn device_id(devinst: u32) -> Option<String> {
    let mut buf = vec![0u16; 512];
    // SAFETY: buffer length in characters.
    let cr = unsafe { CM_Get_Device_IDW(devinst, buf.as_mut_ptr(), buf.len() as u32, 0) };
    (cr == CR_SUCCESS).then(|| from_wide(&buf))
}

fn present_ports() -> Option<DevInfo> {
    // SAFETY: plain query; the result is checked below.
    let raw = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_PORTS,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT,
        )
    };
    (raw != -1).then_some(DevInfo(raw))
}

/// The Ports-class device (COM port) of the virtual device with USB serial
/// number `serial`. The COM port belongs to the CDC function
/// (`USB\VID_…&MI_00\…`); its parent composite device's instance ID ends
/// with our serial number.
fn find_port_device(serial: &str) -> Option<(DevInfo, SP_DEVINFO_DATA)> {
    let set = present_ports()?;
    let suffix = format!(r"\{}", serial.to_ascii_uppercase());
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        let mut parent = 0u32;
        // SAFETY: out-pointer to a devinst.
        let has_parent = unsafe { CM_Get_Parent(&mut parent, data.DevInst, 0) } == CR_SUCCESS;
        let matches = (has_parent
            && device_id(parent).is_some_and(|id| id.to_ascii_uppercase().ends_with(&suffix)))
            || device_id(data.DevInst).is_some_and(|id| id.to_ascii_uppercase().ends_with(&suffix));
        if matches {
            return Some((set, data));
        }
    }
    None
}

/// COM name Windows gave the virtual device with USB serial number `serial`.
pub fn com_port_for_serial(serial: &str) -> Option<String> {
    let (set, data) = find_port_device(serial)?;
    read_port_name(&set, &data)
}

/// Friendly name of the present COM port called `name`, if any.
pub fn port_owner(name: &str) -> Option<String> {
    let set = present_ports()?;
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        if read_port_name(&set, &data).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            let mut buf = vec![0u16; 256];
            // SAFETY: buffer size in bytes as passed.
            let ok = unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set.0,
                    &data,
                    SPDRP_FRIENDLYNAME,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    (buf.len() * 2) as u32,
                    std::ptr::null_mut(),
                )
            };
            return Some(if ok != 0 {
                from_wide(&buf)
            } else {
                name.to_string()
            });
        }
    }
    None
}

/// Gives the virtual device's COM port the number `new_name` (e.g. COM20),
/// like Device Manager's Port Settings > Advanced. Requires administrator
/// rights. Windows remembers the name for this device from then on.
pub fn rename_port(serial: &str, new_name: &str) -> Result<()> {
    let new_number =
        com_number(new_name).ok_or_else(|| Error::Invalid(format!("'{new_name}' is not a COM port name")))?;
    let (set, mut data) =
        find_port_device(serial).ok_or_else(|| Error::NotFound(format!("virtual device {serial}")))?;
    let old = read_port_name(&set, &data);
    if old.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(new_name)) {
        return Ok(());
    }
    if let Some(owner) = com_name_holder(new_name, Some(serial)) {
        return Err(Error::Invalid(format!(
            "{new_name} is already used by \"{owner}\"."
        )));
    }

    let db = ComDb::open()?;
    // No device, plugged in or not, holds this name (leftovers of deleted
    // SerialLoom ports aside), so a stale reservation may be taken over.
    db.claim_number(new_number, true)?;
    if let Err(e) = write_port_name(&set, &data, new_name) {
        db.release(new_number);
        return Err(e);
    }
    let _ = set_string_property(
        &set,
        &mut data,
        SPDRP_FRIENDLYNAME,
        &wide(&format!("SerialLoom Virtual COM Port ({new_name})")),
    );
    if let Some(n) = old.as_deref().and_then(com_number) {
        db.release(n);
    }
    // usbser.sys picks up the new name when the port restarts.
    restart(&set, &data);
    Ok(())
}

// ---------------------------------------------------------------------------
// Administrator operations (run from the elevated helper / installer)
// ---------------------------------------------------------------------------

/// Marker meaning "SerialLoom installed these drivers", so uninstalling
/// SerialLoom never removes a usbip-win2 the user installed separately.
fn marker() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("SerialLoom").join("usbip-installed-by-serialloom")
}

fn install_package(inf: &Path) -> Result<()> {
    let path = wide(&inf.display().to_string());
    let mut reboot = 0;
    // SAFETY: path is a NUL-terminated absolute INF path.
    if unsafe { DiInstallDriverW(std::ptr::null_mut(), path.as_ptr(), 0, &mut reboot) } == 0 {
        let err = last_error();
        // ERROR_NO_MORE_ITEMS: the package is in the driver store but no
        // device needed updating (e.g. it was already installed). That is
        // success, as `pnputil /add-driver /install` reports it.
        if err == ERROR_NO_MORE_ITEMS {
            tracing::info!("{} already up to date", inf.display());
            return Ok(());
        }
        return Err(win_err(&format!("installing {}", inf.display()), err));
    }
    Ok(())
}

/// Creates the root-enumerated host controller device (like `devcon install`).
fn create_root_device(inf: &Path, hwid: &str) -> Result<()> {
    let inf_w = wide(&inf.display().to_string());
    let mut class_guid: windows_sys::core::GUID = unsafe_zeroed();
    let mut class_name = vec![0u16; 64];
    // SAFETY: buffers sized as passed.
    if unsafe {
        SetupDiGetINFClassW(
            inf_w.as_ptr(),
            &mut class_guid,
            class_name.as_mut_ptr(),
            class_name.len() as u32,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(win_err("reading the driver class", last_error()));
    }
    // SAFETY: creates an empty set for that class.
    let raw = unsafe { SetupDiCreateDeviceInfoList(&class_guid, std::ptr::null_mut()) };
    if raw == -1 {
        return Err(win_err("preparing the device", last_error()));
    }
    let set = DevInfo(raw);
    let mut data = new_devinfo_data();
    // SAFETY: class name is NUL-terminated; data.cbSize is set.
    if unsafe {
        SetupDiCreateDeviceInfoW(
            set.0,
            class_name.as_ptr(),
            &class_guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DICD_GENERATE_ID,
            &mut data,
        )
    } == 0
    {
        return Err(win_err("creating the device", last_error()));
    }
    let mut ids: Vec<u16> = hwid.encode_utf16().collect();
    ids.extend([0, 0]);
    // SAFETY: multi-string with byte length.
    if unsafe {
        SetupDiSetDeviceRegistryPropertyW(
            set.0,
            &mut data,
            SPDRP_HARDWAREID,
            ids.as_ptr().cast(),
            (ids.len() * 2) as u32,
        )
    } == 0
    {
        return Err(win_err("setting the hardware ID", last_error()));
    }
    // SAFETY: registers the device node.
    if unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, &data) } == 0 {
        return Err(win_err("registering the device", last_error()));
    }
    Ok(())
}

fn unsafe_zeroed<T>() -> T {
    // SAFETY: only used for plain C structs where all-zero is valid.
    unsafe { std::mem::zeroed() }
}

/// Installs the bundled, Microsoft-signed usbip-win2 drivers.
pub fn install() -> Result<()> {
    if vhci_state().is_some() {
        tracing::info!("virtual USB host controller already present");
        return Ok(());
    }
    let dir = bundled_dir()
        .map(|d| d.join("driver"))
        .filter(|_| drivers_bundled())
        .ok_or_else(|| {
            Error::DriverUnavailable("the virtual USB controller files are missing next to SerialLoom".into())
        })?;

    install_package(&dir.join(FILTER_INF))?;
    let ude = dir.join(UDE_INF);
    create_root_device(&ude, VHCI_HWID)?;
    let hwid = wide(VHCI_HWID);
    let inf = wide(&ude.display().to_string());
    let mut reboot = 0;
    // SAFETY: strings are NUL-terminated.
    if unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            std::ptr::null_mut(),
            hwid.as_ptr(),
            inf.as_ptr(),
            INSTALLFLAG_FORCE,
            &mut reboot,
        )
    } == 0
    {
        return Err(win_err("installing the virtual USB controller", last_error()));
    }
    if let Some(parent) = marker().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker(), b"usbip-win2 drivers installed by SerialLoom\n");
    Ok(())
}

/// Removes the controller and its driver packages, but only if SerialLoom
/// installed them.
pub fn uninstall_if_ours() {
    if !marker().is_file() {
        return;
    }
    let mut removed = 0;
    let set = all_present_devices();
    if let Ok(set) = set {
        for index in 0.. {
            let mut data = new_devinfo_data();
            // SAFETY: data.cbSize is set; the set is valid.
            if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
                break;
            }
            if hardware_ids(&set, &data)
                .iter()
                .any(|id| id.eq_ignore_ascii_case(VHCI_HWID))
                && remove_device(&set, &data).is_ok()
            {
                removed += 1;
            }
        }
    }
    tracing::info!("removed {removed} virtual USB controller(s)");

    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    if let Ok(entries) = std::fs::read_dir(Path::new(&windir).join("INF")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            if !(lower.starts_with("oem") && lower.ends_with(".inf")) {
                continue;
            }
            let ours = std::fs::read(e.path()).is_ok_and(|b| {
                contains_ascii_ci(&b, b"usbip2_ude") || contains_ascii_ci(&b, b"usbip2_filter")
            });
            if ours {
                let n = wide(&name);
                // SAFETY: n is a NUL-terminated oem*.inf name.
                unsafe { SetupUninstallOEMInfW(n.as_ptr(), SUOI_FORCEDELETE, std::ptr::null()) };
            }
        }
    }
    let _ = std::fs::remove_file(marker());
}

// ---------------------------------------------------------------------------
// Device identity and clean removal
// ---------------------------------------------------------------------------

/// Instance-ID prefix of SerialLoom's virtual USB devices (VID/PID from cdc.rs).
fn our_usb_prefix() -> String {
    format!(
        r"USB\VID_{:04X}&PID_{:04X}",
        crate::usbip::cdc::VENDOR_ID,
        crate::usbip::cdc::PRODUCT_ID
    )
}

/// SerialLoom serial numbers: "SL" followed by 10 hex digits.
pub fn is_our_serial(serial: &str) -> bool {
    serial.len() == 12 && serial.starts_with("SL") && serial[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// For `USB\VID_1209&PID_0001\SL0123456789`, returns the serial number.
fn our_serial_of(instance_id: &str) -> Option<String> {
    let upper = instance_id.to_ascii_uppercase();
    let prefix = format!(r"{}\", our_usb_prefix());
    let rest = upper.strip_prefix(&prefix)?;
    is_our_serial(rest).then(|| rest.to_string())
}

/// Every device node, including ones that are not plugged in ("phantoms").
fn all_devices_including_absent() -> Result<DevInfo> {
    // SAFETY: plain query; the result is checked below.
    let set = unsafe {
        SetupDiGetClassDevsW(
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_ALLCLASSES,
        )
    };
    if set == -1 {
        return Err(win_err("listing devices", last_error()));
    }
    Ok(DevInfo(set))
}

fn parent_id(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Option<String> {
    let mut ty = 0u32;
    let mut buf = vec![0u16; 512];
    // SAFETY: buffer size in bytes as passed.
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            set.0,
            data,
            &windows_sys::Win32::Devices::Properties::DEVPKEY_Device_Parent,
            &mut ty,
            buf.as_mut_ptr().cast(),
            (buf.len() * 2) as u32,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok != 0).then(|| from_wide(&buf))
}

fn is_present(devinst: u32) -> bool {
    let mut status = 0u32;
    let mut problem = 0u32;
    // SAFETY: plain query with out-pointers.
    unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) == CR_SUCCESS }
}

fn friendly_name(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Option<String> {
    let mut buf = vec![0u16; 256];
    // SAFETY: buffer size in bytes as passed.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set.0,
            data,
            SPDRP_FRIENDLYNAME,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            (buf.len() * 2) as u32,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&buf)).filter(|s| !s.is_empty())
}

/// Who holds COM name `name`, plugged in or not, other than SerialLoom's
/// device `except_serial`. Real hardware that is unplugged still owns its
/// number, so a virtual port must never take it.
pub fn com_name_holder(name: &str, except_serial: Option<&str>) -> Option<String> {
    // SAFETY: plain query; the result is checked below.
    let raw =
        unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_PORTS, std::ptr::null(), std::ptr::null_mut(), 0) };
    if raw == -1 {
        return None;
    }
    let set = DevInfo(raw);
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        if !read_port_name(&set, &data).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        let ours = parent_id(&set, &data).and_then(|p| our_serial_of(&p));
        let present = is_present(data.DevInst);
        match ours {
            Some(s) if except_serial.is_some_and(|e| e.eq_ignore_ascii_case(&s)) => continue,
            // A leftover entry of a deleted SerialLoom port does not count.
            Some(_) if !present => continue,
            _ => {}
        }
        let label = friendly_name(&set, &data).unwrap_or_else(|| name.to_string());
        return Some(if present {
            label
        } else {
            format!("{label}, not connected right now")
        });
    }
    None
}

fn uninstall_node(set: &DevInfo, data: &SP_DEVINFO_DATA) {
    let mut reboot = 0;
    // SAFETY: the device belongs to `set`.
    if unsafe { DiUninstallDevice(std::ptr::null_mut(), set.0, data, 0, &mut reboot) } == 0 {
        // SAFETY: as above; fallback for devices without a driver.
        unsafe { SetupDiCallClassInstaller(DIF_REMOVE, set.0, data) };
    }
}

/// Removes every trace of SerialLoom virtual devices from Windows: the COM
/// port, its USB function and the USB device itself, plus the COM number
/// reservation. `serials = None` removes all of them (uninstaller).
/// Only devices with SerialLoom's VID/PID *and* serial pattern are touched.
/// Requires administrator rights.
pub fn remove_usb_devices(serials: Option<&[String]>) -> Result<usize> {
    let wanted = |s: &str| serials.is_none_or(|list| list.iter().any(|x| x.eq_ignore_ascii_case(s)));
    let set = all_devices_including_absent()?;
    let mut nodes: Vec<(SP_DEVINFO_DATA, String, Option<String>)> = Vec::new();
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        let id = instance_id(&set, &data);
        let parent = parent_id(&set, &data);
        nodes.push((data, id, parent));
    }

    // Our composite USB devices, then everything below them (deepest first).
    let roots: Vec<String> = nodes
        .iter()
        .filter(|(_, id, _)| our_serial_of(id).is_some_and(|s| wanted(&s)))
        .map(|(_, id, _)| id.clone())
        .collect();
    let mut order: Vec<usize> = Vec::new();
    let mut frontier = roots.clone();
    for _ in 0..4 {
        let next: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(i, (_, _, p))| {
                !order.contains(i)
                    && p.as_ref()
                        .is_some_and(|p| frontier.iter().any(|f| f.eq_ignore_ascii_case(p)))
            })
            .map(|(i, _)| i)
            .collect();
        if next.is_empty() {
            break;
        }
        frontier = next.iter().map(|&i| nodes[i].1.clone()).collect();
        order.extend(next);
    }
    order.reverse();
    order.extend(
        nodes
            .iter()
            .enumerate()
            .filter(|(_, (_, id, _))| roots.contains(id))
            .map(|(i, _)| i),
    );

    let db = ComDb::open().ok();
    for &i in &order {
        let (data, _, _) = &nodes[i];
        if let (Some(db), Some(n)) = (&db, read_port_name(&set, data).as_deref().and_then(com_number)) {
            db.release(n);
        }
        uninstall_node(&set, data);
    }
    Ok(roots.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_serialloom_devices() {
        assert!(is_our_serial("SL0123456789"));
        assert!(!is_our_serial("SL012345678"), "too short");
        assert!(!is_our_serial("XX0123456789"));
        assert!(!is_our_serial("SL01234567ZZ"));
        let id = format!(r"{}\SL00AA11BB22", our_usb_prefix());
        assert_eq!(our_serial_of(&id).as_deref(), Some("SL00AA11BB22"));
        // Same VID/PID but a serial SerialLoom never generates: not ours.
        assert_eq!(our_serial_of(&format!(r"{}\ABC", our_usb_prefix())), None);
        assert_eq!(our_serial_of(r"USB\VID_0403&PID_6001\SL00AA11BB22"), None);
    }
}
