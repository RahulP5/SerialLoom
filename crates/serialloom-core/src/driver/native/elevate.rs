//! Running device operations as administrator.
//!
//! When SerialLoom is not elevated it relaunches its own executable through
//! UAC with `--serialloom-admin <reply-file> <op> [ports…]`. Everything the
//! elevated process acts on comes from its command line, which cannot change
//! after the user approves the prompt, and is validated again here. The
//! driver INF is located by the elevated process itself, never passed in.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    IsUserAnAdmin, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::{localsign, setup};
use crate::driver::validate_port_name;
use crate::error::{Error, Result};
use crate::model::VirtualPortInfo;

/// First argument that switches the executable into admin-helper mode.
pub const ADMIN_FLAG: &str = "--serialloom-admin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminOp {
    InstallDriver,
    /// Installs the bundled usbip-win2 virtual USB controller drivers.
    InstallUsbip,
    CreatePorts(Vec<String>),
    RemovePorts(Vec<String>),
    /// Gives virtual USB serial ports specific COM numbers: (USB serial number, COM name).
    RenameUsbPorts(Vec<(String, String)>),
    /// Removes virtual USB devices (by serial number) from Windows completely.
    RemoveUsbDevices(Vec<String>),
    /// Used by the uninstaller: removes every port and the driver package.
    UninstallAll,
}

impl AdminOp {
    fn to_args(&self) -> Vec<String> {
        match self {
            AdminOp::InstallDriver => vec!["install-driver".into()],
            AdminOp::InstallUsbip => vec!["install-usb".into()],
            AdminOp::UninstallAll => vec!["uninstall-all".into()],
            AdminOp::CreatePorts(names) => std::iter::once("create".to_string())
                .chain(names.iter().cloned())
                .collect(),
            AdminOp::RemovePorts(names) => std::iter::once("remove".to_string())
                .chain(names.iter().cloned())
                .collect(),
            AdminOp::RemoveUsbDevices(serials) => std::iter::once("remove-usb".to_string())
                .chain(serials.iter().cloned())
                .collect(),
            AdminOp::RenameUsbPorts(pairs) => std::iter::once("rename-usb".to_string())
                .chain(pairs.iter().map(|(serial, name)| format!("{serial}={name}")))
                .collect(),
        }
    }

    fn parse(args: &[String]) -> Result<Self> {
        let names = || -> Result<Vec<String>> {
            let names: Vec<String> = args[1..].iter().map(|s| s.to_ascii_uppercase()).collect();
            if names.is_empty() || names.len() > 64 {
                return Err(Error::Invalid("expected 1 to 64 port names".into()));
            }
            for n in &names {
                validate_port_name(n)?;
            }
            Ok(names)
        };
        match args.first().map(String::as_str) {
            Some("install-driver") if args.len() == 1 => Ok(AdminOp::InstallDriver),
            Some("install-usb") if args.len() == 1 => Ok(AdminOp::InstallUsbip),
            Some("uninstall-all") if args.len() == 1 => Ok(AdminOp::UninstallAll),
            Some("create") => Ok(AdminOp::CreatePorts(names()?)),
            Some("remove") => Ok(AdminOp::RemovePorts(names()?)),
            Some("remove-usb") => {
                let serials: Vec<String> = args[1..].to_vec();
                if serials.is_empty() || serials.len() > 64 {
                    return Err(Error::Invalid("expected 1 to 64 devices".into()));
                }
                for s in &serials {
                    validate_usb_serial(s)?;
                }
                Ok(AdminOp::RemoveUsbDevices(serials))
            }
            Some("rename-usb") => {
                let pairs = args[1..]
                    .iter()
                    .map(|arg| {
                        let (serial, name) = arg
                            .split_once('=')
                            .ok_or_else(|| Error::Invalid("expected SERIAL=COMn".into()))?;
                        let name = name.to_ascii_uppercase();
                        validate_usb_serial(serial)?;
                        validate_port_name(&name)?;
                        if name == "COM#" {
                            return Err(Error::Invalid("a specific COM name is required".into()));
                        }
                        Ok((serial.to_string(), name))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if pairs.is_empty() || pairs.len() > 64 {
                    return Err(Error::Invalid("expected 1 to 64 ports".into()));
                }
                Ok(AdminOp::RenameUsbPorts(pairs))
            }
            _ => Err(Error::Invalid("unknown admin operation".into())),
        }
    }
}

/// USB serial numbers SerialLoom generates: 2 to 15 ASCII letters or digits.
fn validate_usb_serial(serial: &str) -> Result<()> {
    if (2..=15).contains(&serial.len()) && serial.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "'{serial}' is not a valid device serial number"
        )))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Reply {
    error: Option<String>,
    ports: Vec<VirtualPortInfo>,
}

pub fn is_elevated() -> bool {
    // SAFETY: plain query with no arguments.
    unsafe { IsUserAnAdmin() != 0 }
}

/// Finds the driver INF shipped with the app.
pub fn bundled_inf() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![
        dir.join("driver").join("serialloom_port.inf"),
        dir.join("resources").join("driver").join("serialloom_port.inf"),
    ];
    if cfg!(debug_assertions) {
        // Development: target/debug/serialloom.exe -> repo root.
        let repo = dir.parent().and_then(Path::parent)?;
        candidates.push(repo.join("drivers/serialloom-port/target/release/package/serialloom_port.inf"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn execute(op: &AdminOp) -> Result<Vec<VirtualPortInfo>> {
    match op {
        AdminOp::InstallDriver => {
            let inf = bundled_inf().ok_or_else(|| {
                Error::DriverUnavailable("the driver package is missing next to SerialLoom".into())
            })?;
            // Signs a verified copy on this PC (free; no paid certificate needed).
            let staged = localsign::prepare(&inf)?;
            setup::install_driver(&staged.inf())?;
            Ok(Vec::new())
        }
        AdminOp::InstallUsbip => {
            crate::driver::usb::system::install()?;
            Ok(Vec::new())
        }
        AdminOp::CreatePorts(names) => {
            let mut created = Vec::new();
            for name in names {
                match setup::create(name) {
                    Ok(p) => created.push(p),
                    Err(e) => {
                        // All or nothing: roll back what this batch created.
                        for p in &created {
                            let _ = setup::remove(&p.name);
                        }
                        return Err(e);
                    }
                }
            }
            Ok(created)
        }
        AdminOp::RemovePorts(names) => {
            for name in names {
                setup::remove(name)?;
            }
            Ok(Vec::new())
        }
        AdminOp::RemoveUsbDevices(serials) => {
            crate::driver::usb::system::remove_usb_devices(Some(serials))?;
            Ok(Vec::new())
        }
        AdminOp::RenameUsbPorts(pairs) => {
            for (serial, name) in pairs {
                crate::driver::usb::system::rename_port(serial, name)?;
            }
            Ok(Vec::new())
        }
        AdminOp::UninstallAll => {
            for p in setup::list()? {
                let _ = setup::remove(&p.name);
            }
            setup::uninstall_driver_packages();
            localsign::remove_local_certificates();
            let _ = crate::driver::usb::system::remove_usb_devices(None);
            crate::driver::usb::system::uninstall_if_ours();
            Ok(Vec::new())
        }
    }
}

/// Runs `op`, asking for elevation through UAC when needed.
pub fn run(op: AdminOp) -> Result<Vec<VirtualPortInfo>> {
    if is_elevated() {
        return execute(&op);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let reply_path =
        std::env::temp_dir().join(format!("serialloom-reply-{}-{stamp}.json", std::process::id()));
    let mut args = vec![ADMIN_FLAG.to_string(), reply_path.display().to_string()];
    args.extend(op.to_args());

    tracing::info!("requesting administrator rights for {:?}", op.to_args());
    let code = spawn_elevated(&args).inspect_err(|e| tracing::error!("elevation failed: {e}"))?;
    tracing::info!("administrator step exited with code {code}");
    let reply: Reply = std::fs::read_to_string(&reply_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&reply_path);

    match (code, reply.error) {
        (0, _) => Ok(reply.ports),
        (_, Some(e)) => Err(Error::Driver(e)),
        (c, None) => Err(Error::Driver(format!(
            "the administrator step failed (exit code {c})"
        ))),
    }
}

fn quote(arg: &str) -> String {
    // Our arguments never contain quotes; paths may contain spaces.
    format!("\"{arg}\"")
}

fn spawn_elevated(args: &[String]) -> Result<u32> {
    use std::os::windows::ffi::OsStrExt;

    let exe = std::env::current_exe()?;
    let wide = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(std::iter::once(0)).collect() };
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let params_text = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    let params = wide(params_text.as_ref());

    // SAFETY: SHELLEXECUTEINFOW is a plain C struct; zero is a valid initial state.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;

    // SAFETY: every pointer in `info` refers to a live, NUL-terminated buffer.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: reads this thread's last error.
        let err = unsafe { GetLastError() };
        return Err(if err == 1223 {
            Error::Driver("Administrator permission was declined, so nothing was changed.".into())
        } else {
            Error::Driver(format!("could not start the administrator step (error {err})"))
        });
    }
    // Copy out of the struct first: it is `packed` on 32-bit targets.
    let process = info.hProcess;
    if process.is_null() {
        return Err(Error::Driver("the administrator step did not start".into()));
    }
    let mut code = 1u32;
    // SAFETY: `process` is a valid handle we own until CloseHandle.
    unsafe {
        WaitForSingleObject(process, INFINITE);
        GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
    }
    Ok(code)
}

/// Entry point for `serialloom.exe --serialloom-admin <reply> <op> …`.
/// `args` excludes the program name and the flag. Returns the exit code.
pub fn admin_main(args: &[String]) -> i32 {
    let Some((reply_path, op_args)) = args.split_first() else {
        return 2;
    };
    tracing::info!("admin operation: {op_args:?}");
    let reply = match AdminOp::parse(op_args).and_then(|op| execute(&op)) {
        Ok(ports) => Reply { error: None, ports },
        Err(e) => Reply {
            error: Some(e.to_string()),
            ports: Vec::new(),
        },
    };
    match &reply.error {
        Some(e) => tracing::error!("admin operation failed: {e}"),
        None => tracing::info!("admin operation succeeded"),
    }
    let code = i32::from(reply.error.is_some());
    write_reply(Path::new(reply_path), &reply);
    code
}

/// Writes the reply only into a fresh file in the temp folder, so the
/// elevated process cannot be tricked into overwriting something else.
fn write_reply(path: &Path, reply: &Reply) {
    let name_ok = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("serialloom-reply-") && n.ends_with(".json"));
    let dir_ok = path.parent().is_some_and(|p| p == std::env::temp_dir().as_path());
    if !(name_ok && dir_ok) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        let _ = serde_json::to_writer(&mut f, reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_and_validates_ops() {
        assert_eq!(
            AdminOp::parse(&s(&["install-driver"])).unwrap(),
            AdminOp::InstallDriver
        );
        assert_eq!(
            AdminOp::parse(&s(&["create", "com20", "COM#"])).unwrap(),
            AdminOp::CreatePorts(s(&["COM20", "COM#"]))
        );
        assert_eq!(
            AdminOp::parse(&s(&["install-usb"])).unwrap(),
            AdminOp::InstallUsbip
        );
        assert!(AdminOp::parse(&s(&["install-usb", "extra"])).is_err());
        assert!(AdminOp::parse(&s(&["create"])).is_err());
        assert!(AdminOp::parse(&s(&["create", "COM1 & calc"])).is_err());
        assert!(AdminOp::parse(&s(&["install-driver", "C:\\evil.inf"])).is_err());
        assert!(AdminOp::parse(&s(&["format-c"])).is_err());
    }

    #[test]
    fn round_trips_args() {
        let rename = AdminOp::RenameUsbPorts(vec![("SL0123456789".into(), "COM20".into())]);
        assert_eq!(AdminOp::parse(&rename.to_args()).unwrap(), rename);
        assert!(AdminOp::parse(&s(&["rename-usb", "SL01=COM#"])).is_err());
        assert!(AdminOp::parse(&s(&["rename-usb", "SL 01=COM5"])).is_err());
        assert!(AdminOp::parse(&s(&["rename-usb", "SL01=COM5&x"])).is_err());
        let op = AdminOp::RemovePorts(s(&["COM5"]));
        assert_eq!(AdminOp::parse(&op.to_args()).unwrap(), op);
    }
}
