//! Device management through SetupAPI and the COM port database.
//!
//! Listing works as a normal user. Creating, removing and installing need
//! administrator rights; the engine reaches those through `elevate`.

use std::path::Path;

use serialloom_proto as proto;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::SerialCommunication::{
    ComDBClaimNextFreePort, ComDBClaimPort, ComDBClose, ComDBOpen, ComDBReleasePort, HCOMDB,
};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GetLastError};
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, REG_SZ, RegCloseKey, RegQueryValueExW, RegSetValueExW,
};

use crate::driver::{com_number, validate_port_name};
use crate::error::{Error, Result};
use crate::model::{VirtualPortInfo, VirtualPortState};

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub(crate) fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last error code.
    unsafe { GetLastError() }
}

fn guid(g: &proto::Guid) -> windows_sys::core::GUID {
    windows_sys::core::GUID {
        data1: g.data1,
        data2: g.data2,
        data3: g.data3,
        data4: g.data4,
    }
}

pub(crate) fn win_err(what: &str, code: u32) -> Error {
    let hint = match code {
        0xE000_0203 | 0xE000_0228 => " The SerialLoom driver is not installed. Install it from Settings.",
        0x800B_0100 | 0x800B_0109 | 0xE000_0242 | 0xE000_022F => {
            " Windows does not trust the driver signature. Use a signed release, or enable test signing on a test machine."
        }
        5 => " Administrator rights are required.",
        _ => "",
    };
    Error::Driver(format!("{what} failed (error {code:#x}).{hint}"))
}

/// Owned SetupAPI device information set.
pub(crate) struct DevInfo(pub(crate) HDEVINFO);

impl Drop for DevInfo {
    fn drop(&mut self) {
        // SAFETY: the set was created by SetupAPI and is destroyed once.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

pub(crate) fn new_devinfo_data() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..unsafe_zeroed()
    }
}

fn unsafe_zeroed<T>() -> T {
    // SAFETY: only used for plain C structs where all-zero is valid.
    unsafe { std::mem::zeroed() }
}

fn ports_class_devices() -> Result<DevInfo> {
    // SAFETY: plain query; the result is checked below.
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_PORTS,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT,
        )
    };
    if set == -1 {
        return Err(win_err("listing serial devices", last_error()));
    }
    Ok(DevInfo(set))
}

pub(crate) fn hardware_ids(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Vec<String> {
    let mut buf = vec![0u16; 512];
    let mut required = 0u32;
    // SAFETY: buffer is sized in bytes as passed.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set.0,
            data,
            SPDRP_HARDWAREID,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            (buf.len() * 2) as u32,
            &mut required,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    buf.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

pub(crate) fn instance_id(set: &DevInfo, data: &SP_DEVINFO_DATA) -> String {
    let mut buf = vec![0u16; 512];
    // SAFETY: buffer length is in characters as documented.
    unsafe {
        SetupDiGetDeviceInstanceIdW(
            set.0,
            data,
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null_mut(),
        );
    }
    from_wide(&buf)
}

pub(crate) fn read_port_name(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Option<String> {
    // SAFETY: opens the device's hardware key for reading.
    let key: HKEY = unsafe { SetupDiOpenDevRegKey(set.0, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
    if key.is_null() || key as isize == -1 {
        return None;
    }
    let name = wide("PortName");
    let mut buf = vec![0u16; 64];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: key is open; buffer size passed in bytes.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    // SAFETY: key was opened above.
    unsafe { RegCloseKey(key) };
    (status == ERROR_SUCCESS)
        .then(|| from_wide(&buf))
        .filter(|s| !s.is_empty())
}

pub(crate) fn write_port_name(set: &DevInfo, data: &SP_DEVINFO_DATA, port: &str) -> Result<()> {
    // SAFETY: creates/opens the device's hardware key.
    let key: HKEY = unsafe {
        SetupDiCreateDevRegKeyW(
            set.0,
            data,
            DICS_FLAG_GLOBAL,
            0,
            DIREG_DEV,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if key.is_null() || key as isize == -1 {
        return Err(win_err("opening the device key", last_error()));
    }
    let name = wide("PortName");
    let value = wide(port);
    // SAFETY: key is open; value is NUL-terminated UTF-16 of the given byte length.
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    // SAFETY: key was opened above.
    unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(win_err("writing PortName", status))
    }
}

pub(crate) fn set_string_property(
    set: &DevInfo,
    data: &mut SP_DEVINFO_DATA,
    prop: u32,
    value: &[u16],
) -> Result<()> {
    // SAFETY: value is a NUL-terminated (multi-)string; size in bytes.
    let ok = unsafe {
        SetupDiSetDeviceRegistryPropertyW(set.0, data, prop, value.as_ptr().cast(), (value.len() * 2) as u32)
    };
    if ok == 0 {
        return Err(win_err("setting a device property", last_error()));
    }
    Ok(())
}

pub(crate) struct DeviceState {
    pub(crate) state: VirtualPortState,
    pub(crate) text: Option<String>,
    pub(crate) code: Option<u32>,
    pub(crate) status: Option<String>,
}

/// NTSTATUS Windows recorded for the device problem (`DEVPKEY_Device_ProblemStatus`).
fn problem_status(devinst: u32) -> Option<String> {
    let mut ty = 0u32;
    let mut value = 0i32;
    let mut size = size_of::<i32>() as u32;
    // SAFETY: the buffer is 4 bytes, matching DEVPROP_TYPE_NTSTATUS.
    let cr = unsafe {
        CM_Get_DevNode_PropertyW(
            devinst,
            &windows_sys::Win32::Devices::Properties::DEVPKEY_Device_ProblemStatus,
            &mut ty,
            std::ptr::from_mut(&mut value).cast(),
            &mut size,
            0,
        )
    };
    (cr == CR_SUCCESS && value != 0).then(|| format!("{:#010x}", value as u32))
}

pub(crate) fn device_state(devinst: u32) -> DeviceState {
    let mut status = 0u32;
    let mut problem = 0u32;
    // SAFETY: plain query with out-pointers.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    if cr != CR_SUCCESS {
        return DeviceState {
            state: VirtualPortState::Stopped,
            text: None,
            code: None,
            status: None,
        };
    }
    if status & DN_HAS_PROBLEM != 0 {
        let ntstatus = problem_status(devinst);
        let base = match problem {
            28 => "The driver is not installed.",
            52 => "Windows blocked the driver because its signature is not trusted.",
            10 => "The driver failed to start.",
            31 => "Windows could not load the driver.",
            22 => "The device is disabled.",
            _ => "Windows reports a problem with this device.",
        };
        let detail = match &ntstatus {
            Some(st) => format!(" (code {problem}, status {st})"),
            None => format!(" (code {problem})"),
        };
        return DeviceState {
            state: VirtualPortState::Problem,
            text: Some(format!("{base}{detail}")),
            code: Some(problem),
            status: ntstatus,
        };
    }
    DeviceState {
        state: if status & DN_STARTED != 0 {
            VirtualPortState::Ready
        } else {
            VirtualPortState::Stopped
        },
        text: None,
        code: None,
        status: None,
    }
}

/// Reads `SerialLoomStartup`, written by the driver, from the device's
/// hardware key or one of its subkeys (UMDF uses a subkey for driver writes).
fn driver_report(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegEnumKeyExW, RegGetValueW};

    // SAFETY: opens the device's hardware key for reading.
    let key: HKEY = unsafe { SetupDiOpenDevRegKey(set.0, data, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
    if key.is_null() || key as isize == -1 {
        return None;
    }
    let value_name = wide("SerialLoomStartup");
    let read = |subkey: &[u16]| -> Option<String> {
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let sub = if subkey.is_empty() {
            std::ptr::null()
        } else {
            subkey.as_ptr()
        };
        // SAFETY: key is open; buffer length is in bytes.
        let status = unsafe {
            RegGetValueW(
                key,
                sub,
                value_name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (status == ERROR_SUCCESS).then(|| from_wide(&buf))
    };

    let mut found = read(&[]);
    let mut index = 0;
    while found.is_none() && index < 32 {
        let mut name = vec![0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: key is open; name buffer length in characters.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        name.truncate(len as usize);
        name.push(0);
        found = read(&name);
        index += 1;
    }
    // SAFETY: key was opened above.
    unsafe { RegCloseKey(key) };
    found
}

/// Calls `f` for every SerialLoom device. Stops early when `f` returns false.
fn each_device(mut f: impl FnMut(&DevInfo, &SP_DEVINFO_DATA) -> bool) -> Result<()> {
    let set = ports_class_devices()?;
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        let ours = hardware_ids(&set, &data)
            .iter()
            .any(|id| id.eq_ignore_ascii_case(proto::HARDWARE_ID));
        if ours && !f(&set, &data) {
            break;
        }
    }
    Ok(())
}

fn info_of(set: &DevInfo, data: &SP_DEVINFO_DATA) -> VirtualPortInfo {
    let st = device_state(data.DevInst);
    VirtualPortInfo {
        id: instance_id(set, data),
        name: read_port_name(set, data).unwrap_or_else(|| "(unnamed)".into()),
        state: st.state,
        problem: st.text,
        problem_code: st.code,
        problem_status: st.status,
        driver_report: driver_report(set, data),
    }
}

pub fn list() -> Result<Vec<VirtualPortInfo>> {
    let mut out = Vec::new();
    each_device(|set, data| {
        out.push(info_of(set, data));
        true
    })?;
    out.sort_by(|a, b| crate::ports::natural_cmp(&a.name, &b.name));
    Ok(out)
}

fn instance_of(port: &str) -> Result<String> {
    let mut found = None;
    each_device(|set, data| {
        if read_port_name(set, data).is_some_and(|n| n.eq_ignore_ascii_case(port)) {
            found = Some(instance_id(set, data));
            return false;
        }
        true
    })?;
    found.ok_or_else(|| Error::NotFound(format!("virtual port {port}")))
}

/// Path of the control interface, e.g. `\\?\ROOT#PORTS#0003#{guid}\control`.
pub fn control_path(port: &str) -> Result<String> {
    let instance = wide(&instance_of(port)?);
    let g = guid(&proto::CONTROL_INTERFACE_GUID);
    let mut len = 0u32;
    // SAFETY: plain size query.
    let cr = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut len,
            &g,
            instance.as_ptr(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if cr != CR_SUCCESS || len <= 1 {
        return Err(Error::Driver(format!(
            "{port} exists but its driver is not running. Check the port's status in Virtual ports."
        )));
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: buffer has `len` characters as reported above.
    let cr = unsafe {
        CM_Get_Device_Interface_ListW(
            &g,
            instance.as_ptr(),
            buf.as_mut_ptr(),
            len,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if cr != CR_SUCCESS {
        return Err(Error::Driver(format!(
            "cannot find the control interface of {port} (CM error {cr})"
        )));
    }
    let mut path = from_wide(&buf);
    let suffix = format!("\\{}", proto::CONTROL_REFERENCE);
    if !path.to_ascii_lowercase().ends_with(&suffix) {
        path.push_str(&suffix);
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Administrator operations
// ---------------------------------------------------------------------------

pub(crate) struct ComDb(HCOMDB);

impl ComDb {
    pub(crate) fn open() -> Result<Self> {
        let mut h: HCOMDB = std::ptr::null_mut();
        // SAFETY: out-pointer to a handle.
        let status = unsafe { ComDBOpen(&mut h) };
        if status != 0 || h.is_null() {
            return Err(win_err("opening the COM port database", status as u32));
        }
        Ok(Self(h))
    }

    fn claim(&self, name: &str) -> Result<u32> {
        if name == "COM#" {
            let mut n = 0u32;
            // SAFETY: database is open.
            let status = unsafe { ComDBClaimNextFreePort(self.0, &mut n) };
            if status != 0 {
                return Err(win_err("reserving a COM number", status as u32));
            }
            return Ok(n);
        }
        let n = com_number(name).ok_or_else(|| Error::Invalid(format!("invalid port name {name}")))?;
        let mut forced = 0;
        // SAFETY: database is open.
        let status = unsafe { ComDBClaimPort(self.0, n, 0, &mut forced) };
        match status {
            0 => Ok(n),
            32 => Err(Error::Invalid(format!(
                "COM{n} is already used by another device."
            ))),
            e => Err(win_err("reserving the COM number", e as u32)),
        }
    }

    /// Reserves `COMn`, taking over a stale reservation when `force` is set.
    pub(crate) fn claim_number(&self, n: u32, force: bool) -> Result<()> {
        let mut forced = 0;
        // SAFETY: database is open.
        let status = unsafe { ComDBClaimPort(self.0, n, i32::from(force), &mut forced) };
        match status {
            0 => Ok(()),
            32 => Err(Error::Invalid(format!(
                "COM{n} is already used by another device."
            ))),
            e => Err(win_err("reserving the COM number", e as u32)),
        }
    }

    pub(crate) fn release(&self, n: u32) {
        // SAFETY: database is open.
        unsafe { ComDBReleasePort(self.0, n) };
    }
}

impl Drop for ComDb {
    fn drop(&mut self) {
        // SAFETY: closes the handle opened in `open`.
        unsafe { ComDBClose(self.0) };
    }
}

pub(crate) fn restart(set: &DevInfo, data: &SP_DEVINFO_DATA) {
    let params = SP_PROPCHANGE_PARAMS {
        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
            cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
            InstallFunction: DIF_PROPERTYCHANGE,
        },
        StateChange: DICS_PROPCHANGE,
        Scope: DICS_FLAG_GLOBAL,
        HwProfile: 0,
    };
    // SAFETY: params outlives both calls; sizes match the struct.
    unsafe {
        SetupDiSetClassInstallParamsW(
            set.0,
            data,
            &params.ClassInstallHeader,
            size_of::<SP_PROPCHANGE_PARAMS>() as u32,
        );
        SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, data);
    }
}

pub(crate) fn remove_device(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Result<()> {
    let mut reboot = 0;
    // SAFETY: device belongs to `set`.
    if unsafe { DiUninstallDevice(std::ptr::null_mut(), set.0, data, 0, &mut reboot) } != 0 {
        return Ok(());
    }
    // SAFETY: as above; fallback for devices without a driver.
    if unsafe { SetupDiCallClassInstaller(DIF_REMOVE, set.0, data) } != 0 {
        return Ok(());
    }
    Err(win_err("removing the device", last_error()))
}

/// Creates one virtual port. Requires administrator rights.
pub fn create(name: &str) -> Result<VirtualPortInfo> {
    validate_port_name(name)?;
    let db = ComDb::open()?;
    let number = db.claim(name)?;
    let port = format!("COM{number}");
    match create_device(&port) {
        Ok(info) => Ok(info),
        Err(e) => {
            db.release(number);
            Err(e)
        }
    }
}

fn create_device(port: &str) -> Result<VirtualPortInfo> {
    // SAFETY: creates an empty set for the Ports class.
    let raw = unsafe { SetupDiCreateDeviceInfoList(&GUID_DEVCLASS_PORTS, std::ptr::null_mut()) };
    if raw == -1 {
        return Err(win_err("preparing the device", last_error()));
    }
    let set = DevInfo(raw);
    let mut data = new_devinfo_data();
    let class_name = wide("Ports");
    let description = wide("SerialLoom Virtual Serial Port");
    // SAFETY: strings are NUL-terminated; data.cbSize is set.
    let ok = unsafe {
        SetupDiCreateDeviceInfoW(
            set.0,
            class_name.as_ptr(),
            &GUID_DEVCLASS_PORTS,
            description.as_ptr(),
            std::ptr::null_mut(),
            DICD_GENERATE_ID,
            &mut data,
        )
    };
    if ok == 0 {
        return Err(win_err("creating the device", last_error()));
    }

    let mut hwid: Vec<u16> = proto::HARDWARE_ID.encode_utf16().collect();
    hwid.extend([0, 0]);
    set_string_property(&set, &mut data, SPDRP_HARDWAREID, &hwid)?;

    // SAFETY: registers the device node so it survives this process.
    if unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, &data) } == 0 {
        return Err(win_err("registering the device", last_error()));
    }

    let result = (|| {
        write_port_name(&set, &data, port)?;
        set_string_property(
            &set,
            &mut data,
            SPDRP_FRIENDLYNAME,
            &wide(&format!("SerialLoom Virtual Port ({port})")),
        )?;
        let mut reboot = 0;
        // SAFETY: installs the best matching driver from the driver store.
        if unsafe {
            DiInstallDevice(
                std::ptr::null_mut(),
                set.0,
                &data,
                std::ptr::null(),
                0,
                &mut reboot,
            )
        } == 0
        {
            return Err(win_err("installing the driver on the device", last_error()));
        }
        // The Ports class installer may have picked its own number; ours wins.
        if read_port_name(&set, &data).is_none_or(|n| !n.eq_ignore_ascii_case(port)) {
            if let Some(other) = read_port_name(&set, &data).and_then(|n| com_number(&n))
                && let Ok(db) = ComDb::open()
            {
                db.release(other);
            }
            write_port_name(&set, &data, port)?;
            restart(&set, &data);
        }
        Ok(info_of(&set, &data))
    })();

    if result.is_err() {
        let _ = remove_device(&set, &data);
    }
    result
}

/// Removes one virtual port. Requires administrator rights.
pub fn remove(port: &str) -> Result<()> {
    let set = ports_class_devices()?;
    for index in 0.. {
        let mut data = new_devinfo_data();
        // SAFETY: data.cbSize is set; the set is valid.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } == 0 {
            break;
        }
        let ours = hardware_ids(&set, &data)
            .iter()
            .any(|id| id.eq_ignore_ascii_case(proto::HARDWARE_ID));
        if ours && read_port_name(&set, &data).is_some_and(|n| n.eq_ignore_ascii_case(port)) {
            remove_device(&set, &data)?;
            if let Some(n) = com_number(port) {
                ComDb::open()?.release(n);
            }
            return Ok(());
        }
    }
    Err(Error::NotFound(format!("virtual port {port}")))
}

/// Adds the driver package to the driver store. Requires administrator rights.
pub fn install_driver(inf: &Path) -> Result<()> {
    let path = wide(&inf.display().to_string());
    let mut reboot = 0;
    // SAFETY: path is a NUL-terminated absolute INF path.
    if unsafe { DiInstallDriverW(std::ptr::null_mut(), path.as_ptr(), 0, &mut reboot) } == 0 {
        let err = last_error();
        // ERROR_NO_MORE_ITEMS (259): staged, but no device needed updating.
        if err == 259 {
            return Ok(());
        }
        return Err(win_err("installing the driver package", err));
    }
    Ok(())
}

/// Removes every SerialLoom driver package from the driver store (best effort).
pub fn uninstall_driver_packages() {
    for name in installed_oem_infs() {
        let wide_name = wide(&name);
        // SAFETY: name is a NUL-terminated oem*.inf file name.
        unsafe { SetupUninstallOEMInfW(wide_name.as_ptr(), SUOI_FORCEDELETE, std::ptr::null()) };
    }
}

/// File names of installed driver packages (`%WINDIR%\INF\oem*.inf`) that are ours.
fn installed_oem_infs() -> Vec<String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let Ok(entries) = std::fs::read_dir(Path::new(&windir).join("INF")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let lower = name.to_ascii_lowercase();
            (lower.starts_with("oem")
                && lower.ends_with(".inf")
                && std::fs::read(e.path()).is_ok_and(|b| contains_ascii_ci(&b, br"root\serialloomport")))
            .then_some(name)
        })
        .collect()
}

/// True when a SerialLoom driver package is in the driver store.
pub fn driver_installed() -> bool {
    !installed_oem_infs().is_empty()
}

pub(crate) fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w.iter().zip(needle).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

// ---------------------------------------------------------------------------
// Diagnostics (read-only, no administrator rights needed)
// ---------------------------------------------------------------------------

pub(crate) fn windows_version() -> String {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    let path = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let text = |name: &str| -> String {
        let n = wide(name);
        let mut buf = vec![0u16; 256];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: buffer length in bytes; strings NUL-terminated.
        let st = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if st == ERROR_SUCCESS {
            from_wide(&buf)
        } else {
            "?".into()
        }
    };
    let n = wide("UBR");
    let mut ubr = 0u32;
    let mut len = 4u32;
    // SAFETY: 4-byte DWORD buffer.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut(&mut ubr).cast(),
            &mut len,
        )
    };
    format!(
        "{} {} (build {}.{ubr}, {})",
        text("ProductName"),
        text("DisplayVersion"),
        text("CurrentBuildNumber"),
        std::env::consts::ARCH
    )
}

pub(crate) fn run_tool(exe: &str, args: &[&str], max_chars: usize) -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() {
                text = String::from_utf8_lossy(&out.stderr).trim().to_string();
            }
            if text.is_empty() {
                text = "(no entries)".into();
            }
            if let Some((cut, _)) = text.char_indices().nth(max_chars) {
                text.truncate(cut);
                text.push_str("\n…(truncated)");
            }
            text
        }
        Err(e) => format!("(could not run {exe}: {e})"),
    }
}

/// Last SerialLoom-related section of `C:\Windows\INF\setupapi.dev.log`.
fn setupapi_log() -> String {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let path = Path::new(&windir).join("INF").join("setupapi.dev.log");
    let Ok(bytes) = std::fs::read(&path) else {
        return "(setupapi.dev.log is not readable)".into();
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lower = text.to_ascii_lowercase();
    let Some(hit) = lower.rfind("serialloom") else {
        return "(no SerialLoom entries)".into();
    };
    let start = lower[..hit].rfind("\n>>>  ").map_or(0, |i| i + 1);
    let end = lower[hit..].find("\n<<<  ").map_or(text.len(), |i| {
        let after = hit + i + 1;
        lower[after..].find('\n').map_or(text.len(), |j| after + j)
    });
    let section: Vec<&str> = text[start..end].lines().collect();
    let skip = section.len().saturating_sub(150);
    section[skip..].join("\n")
}

/// Plain-text troubleshooting report. Reads only; changes nothing.
pub fn diagnostics(info: &crate::model::DriverInfo) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    line(format!("Windows: {}", windows_version()));
    line(format!(
        "Driver: {} | available={} | installable={} | package={}",
        info.name,
        info.available,
        info.installable,
        info.location.as_deref().unwrap_or("-")
    ));
    line(format!("Driver store packages: {:?}", installed_oem_infs()));
    line(String::new());

    line("== Virtual ports ==".into());
    match list() {
        Ok(ports) if ports.is_empty() => line("(none)".into()),
        Ok(ports) => {
            for p in ports {
                line(format!(
                    "{} [{}] state={:?} code={} status={}",
                    p.name,
                    p.id,
                    p.state,
                    p.problem_code.map_or_else(|| "-".to_string(), |c| c.to_string()),
                    p.problem_status.as_deref().unwrap_or("-")
                ));
                line(format!(
                    "  driver report: {}",
                    p.driver_report.as_deref().unwrap_or("(none)")
                ));
            }
        }
        Err(e) => line(format!("(listing failed: {e})")),
    }
    line(String::new());

    line("== Kernel-PnP device start events ==".into());
    line(run_tool(
        "wevtutil.exe",
        &[
            "qe",
            "Microsoft-Windows-Kernel-PnP/Configuration",
            "/q:*[System[(EventID=411 or EventID=400)]]",
            "/c:8",
            "/rd:true",
            "/f:text",
        ],
        6000,
    ));
    line(String::new());

    line("== User-mode driver framework events (System log) ==".into());
    line(run_tool(
        "wevtutil.exe",
        &[
            "qe",
            "System",
            "/q:*[System[Provider[@Name='Microsoft-Windows-DriverFrameworks-UserMode']]]",
            "/c:8",
            "/rd:true",
            "/f:text",
        ],
        6000,
    ));
    line(String::new());

    line("== setupapi.dev.log (last SerialLoom section) ==".into());
    line(setupapi_log());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: collects the report on the build machine.
    #[test]
    fn diagnostics_report_builds() {
        let info = crate::model::DriverInfo {
            id: "native".into(),
            name: "test".into(),
            available: false,
            installable: false,
            location: None,
            message: None,
        };
        let report = diagnostics(&info);
        assert!(report.contains("Windows:"));
        assert!(report.contains("== Virtual ports =="));
        println!("{report}");
    }
}
