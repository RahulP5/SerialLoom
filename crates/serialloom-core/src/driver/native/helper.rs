//! SerialLoom Helper: a small Windows service that performs SerialLoom's
//! port operations as administrator, so creating, renumbering and removing
//! virtual ports does not ask for permission every time.
//!
//! The installer (already elevated) registers the service. It runs only
//! while needed: SerialLoom starts it on demand and it stops itself after a
//! few idle minutes.
//!
//! What keeps it safe:
//! * It does only what the regular UAC helper already allows for *port*
//!   operations, parsed and validated by the same code ([`AdminOp::parse`]).
//!   Installing or removing drivers and the service itself stays
//!   installer/UAC-only. USB operations accept SerialLoom's own devices only
//!   (USB ID *and* serial pattern), and a port never takes a COM number held
//!   by real hardware (checked again on the administrator side).
//! * Its pipe rejects remote clients and network logons, grants local users
//!   read/write only (they cannot create rogue server instances), and is
//!   created first-instance-only.
//! * It serves only the SerialLoom executable it was installed from.
//! * SerialLoom checks that the pipe really belongs to the service process
//!   before sending anything, and connects at identification level.
//! * The service binary lives in Program Files (writable by administrators
//!   only); installing it from anywhere else is refused.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    ERROR_FAILED_SERVICE_CONTROLLER_CONNECT, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
    ERROR_SERVICE_NOT_ACTIVE, GetLastError,
};
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

use super::elevate::{AdminOp, Reply};
use crate::driver::usb::system::is_our_serial;
use crate::error::{Error, Result};
use crate::model::VirtualPortInfo;
use crate::winpipe::{
    CLIENT_RIGHTS_SDDL, PipeListener, PipeStream, SecurityDescriptor, process_image, same_file,
};

pub const SERVICE_NAME: &str = "SerialLoomHelper";
const DISPLAY_NAME: &str = "SerialLoom Helper";
const DESCRIPTION: &str = "Lets SerialLoom create, renumber and remove its own virtual COM ports \
without asking for administrator permission each time. Starts when SerialLoom needs it.";

/// First argument that runs the executable as the service.
pub const SERVICE_FLAG: &str = "--serialloom-service";

pub const PIPE_NAME: &str = r"\\.\pipe\SerialLoom.Helper";

/// Protocol version, sent with every request.
const PROTOCOL: u32 = 1;

/// The service stops after this long without requests.
const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// A client must send its request within this time.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST: usize = 16 * 1024;
/// Port operations wait for Windows to restart devices; allow for that.
const REPLY_TIMEOUT: Duration = Duration::from_secs(180);

/// Pipe security: SYSTEM and administrators full control; network logons
/// denied; authenticated local users may read and write data, but not
/// create instances of the pipe.
fn pipe_sddl() -> String {
    format!("D:P(D;;GA;;;NU)(A;;GA;;;SY)(A;;GA;;;BA)(A;;{CLIENT_RIGHTS_SDDL};;;AU)")
}

/// Service security: as Windows' default, plus "start" and "query status"
/// for authenticated users, so SerialLoom can start the helper on demand.
/// Only administrators can stop, reconfigure or delete it.
const SERVICE_SDDL: &str =
    "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWRPLORC;;;AU)";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u32,
    op: Vec<String>,
}

/// Whether the helper may run `op`, with the extra checks it applies.
pub(crate) fn check_allowed(op: &AdminOp) -> Result<()> {
    let ours = |serial: &String| {
        if is_our_serial(serial) {
            Ok(())
        } else {
            Err(Error::Invalid(format!("'{serial}' is not a SerialLoom device")))
        }
    };
    match op {
        AdminOp::CreatePorts(_) | AdminOp::RemovePorts(_) => Ok(()),
        AdminOp::RenameUsbPorts(pairs) => pairs.iter().try_for_each(|(serial, _)| ours(serial)),
        AdminOp::RemoveUsbDevices(serials) => serials.iter().try_for_each(ours),
        AdminOp::InstallDriver
        | AdminOp::InstallUsbip
        | AdminOp::InstallService
        | AdminOp::UninstallService
        | AdminOp::UninstallAll => Err(Error::Invalid(
            "this operation is not available through SerialLoom Helper".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Client side (the app)
// ---------------------------------------------------------------------------

/// Installed / running state, for the Settings page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperStatus {
    pub installed: bool,
    pub running: bool,
}

struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn last_error() -> u32 {
    // SAFETY: reads this thread's last error.
    unsafe { GetLastError() }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_manager(access: u32) -> Result<ScHandle> {
    // SAFETY: local machine, default database.
    let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if h.is_null() {
        return Err(Error::Driver(format!(
            "could not open the service manager (error {})",
            last_error()
        )));
    }
    Ok(ScHandle(h))
}

/// `Ok(None)` when the service is not installed.
fn open_service(access: u32, manager_access: u32) -> Result<Option<ScHandle>> {
    let manager = open_manager(manager_access)?;
    let name = wide(SERVICE_NAME);
    // SAFETY: name is NUL-terminated.
    let h = unsafe { OpenServiceW(manager.0, name.as_ptr(), access) };
    if h.is_null() {
        let err = last_error();
        if err == ERROR_SERVICE_DOES_NOT_EXIST {
            return Ok(None);
        }
        return Err(Error::Driver(format!(
            "could not open SerialLoom Helper (error {err})"
        )));
    }
    Ok(Some(ScHandle(h)))
}

fn query(service: &ScHandle) -> Result<SERVICE_STATUS_PROCESS> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    // SAFETY: buffer is a SERVICE_STATUS_PROCESS of the size passed.
    let ok = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(Error::Driver(format!(
            "could not query SerialLoom Helper (error {})",
            last_error()
        )));
    }
    Ok(status)
}

pub fn status() -> HelperStatus {
    match open_service(SERVICE_QUERY_STATUS, SC_MANAGER_CONNECT) {
        Ok(Some(s)) => HelperStatus {
            installed: true,
            running: query(&s).is_ok_and(|st| st.dwCurrentState == SERVICE_RUNNING),
        },
        _ => HelperStatus::default(),
    }
}

/// Starts the service if needed and returns its process ID.
fn ensure_running() -> Result<u32> {
    let service = open_service(SERVICE_QUERY_STATUS | SERVICE_START, SC_MANAGER_CONNECT)?
        .ok_or_else(|| Error::NotFound("SerialLoom Helper is not installed".into()))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut starts = 0;
    loop {
        let st = query(&service)?;
        match st.dwCurrentState {
            SERVICE_RUNNING if st.dwProcessId != 0 => return Ok(st.dwProcessId),
            // Stopped, or stopped again (e.g. it was idling out as we asked).
            SERVICE_STOPPED => {
                if starts == 2 {
                    return Err(Error::Driver(
                        "SerialLoom Helper stops right after starting".into(),
                    ));
                }
                starts += 1;
                // SAFETY: no arguments.
                if unsafe { StartServiceW(service.0, 0, std::ptr::null()) } == 0 {
                    let err = last_error();
                    if err != ERROR_SERVICE_ALREADY_RUNNING {
                        return Err(Error::Driver(format!(
                            "could not start SerialLoom Helper (error {err})"
                        )));
                    }
                }
            }
            _ => {}
        }
        if Instant::now() > deadline {
            return Err(Error::Driver("SerialLoom Helper did not start in time".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Starts the helper and connects to it, checking that the pipe belongs to
/// the service process and not to a program that took the name first.
fn connect() -> Result<PipeStream> {
    let mut last_err = Error::Driver("SerialLoom Helper is not reachable".into());
    // Twice: the service may stop for idleness just as we connect.
    for _ in 0..2 {
        let pid = ensure_running()?;
        let conn = match PipeStream::connect(PIPE_NAME, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                last_err = Error::Driver(format!("could not connect to SerialLoom Helper: {e}"));
                continue;
            }
        };
        return match conn.server_process_id() {
            Ok(server) if server == pid => Ok(conn),
            other => Err(Error::Driver(format!(
                "refusing to use {PIPE_NAME}: it is served by process {other:?}, \
                 but SerialLoom Helper is process {pid}"
            ))),
        };
    }
    Err(last_err)
}

/// Runs `op` through the helper service.
///
/// `None` means the helper cannot be used (not installed, disabled, not
/// trustworthy, or the op is not one it performs) and nothing was sent, so
/// the caller can fall back to a UAC prompt. Once a request is sent, the
/// result is always `Some`, so an operation never runs twice.
pub fn call(op: &AdminOp) -> Option<Result<Vec<VirtualPortInfo>>> {
    check_allowed(op).ok()?;
    let conn = match connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("SerialLoom Helper not used ({e}); asking through UAC instead");
            return None;
        }
    };
    send(conn, op)
}

/// Sends `op` on a connection to the helper and reads its reply, with the
/// same meaning of `None` as [`call`].
fn send(mut conn: PipeStream, op: &AdminOp) -> Option<Result<Vec<VirtualPortInfo>>> {
    let request = Request {
        v: PROTOCOL,
        op: op.to_args(),
    };
    let bytes = serde_json::to_vec(&request).expect("request serializes");
    if let Err(e) = conn.write_line(&bytes, Some(REQUEST_TIMEOUT)) {
        tracing::warn!("could not send to SerialLoom Helper: {e}");
        return None;
    }
    tracing::info!("SerialLoom Helper: {:?}", request.op);
    let reply = conn
        .read_line(1024 * 1024, Some(REPLY_TIMEOUT))
        .map_err(|e| Error::Driver(format!("SerialLoom Helper did not answer: {e}")))
        .and_then(|line| line.ok_or_else(|| Error::Driver("SerialLoom Helper closed the connection".into())))
        .and_then(|line| {
            serde_json::from_slice::<Reply>(&line)
                .map_err(|e| Error::Driver(format!("unexpected answer from SerialLoom Helper: {e}")))
        });
    match reply {
        // Turned down before anything ran (e.g. this is not the installed
        // SerialLoom): asking through UAC instead is safe.
        Ok(r) if r.refused => {
            tracing::warn!(
                "SerialLoom Helper declined ({}); asking through UAC instead",
                r.error.as_deref().unwrap_or("no reason given")
            );
            None
        }
        Ok(Reply {
            error: None, ports, ..
        }) => Some(Ok(ports)),
        Ok(Reply { error: Some(e), .. }) => Some(Err(Error::Driver(e))),
        Err(e) => Some(Err(e)),
    }
}

// ---------------------------------------------------------------------------
// Service side
// ---------------------------------------------------------------------------

/// Serves requests until `stop` is set or nobody called for `idle`.
/// `exec` performs a checked operation.
pub(crate) fn serve(
    pipe: &str,
    sddl: &str,
    stop: &AtomicBool,
    idle: Duration,
    allowed_client: &Path,
    exec: &dyn Fn(&AdminOp) -> Result<Vec<VirtualPortInfo>>,
    on_ready: &mut dyn FnMut(),
) -> std::io::Result<()> {
    let mut listener = PipeListener::bind(pipe, sddl, 4)?;
    on_ready();
    let mut last = Instant::now();
    let mut failures = 0;
    while !stop.load(Ordering::SeqCst) {
        match listener.accept(Some(Duration::from_millis(500))) {
            Ok(Some(conn)) => {
                failures = 0;
                handle(conn, allowed_client, exec);
                last = Instant::now();
            }
            Ok(None) if last.elapsed() > idle => {
                tracing::info!("no requests for {} s; stopping", idle.as_secs());
                break;
            }
            Ok(None) => {}
            Err(e) => {
                failures += 1;
                tracing::warn!("accepting a connection failed: {e}");
                if failures > 20 {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    Ok(())
}

fn handle(
    mut conn: PipeStream,
    allowed_client: &Path,
    exec: &dyn Fn(&AdminOp) -> Result<Vec<VirtualPortInfo>>,
) {
    let mut trusted = false;
    // Outer error: refused without running anything.
    let outcome: std::result::Result<Result<Vec<VirtualPortInfo>>, Error> =
        match conn.client_process_id().and_then(process_image) {
            Ok(image) if same_file(&image, allowed_client) => {
                trusted = true;
                match conn.read_line(MAX_REQUEST, Some(REQUEST_TIMEOUT)) {
                    Ok(Some(line)) => check_request(&line).map(|op| run_checked(&op, exec)),
                    Ok(None) => return,
                    Err(e) => {
                        tracing::warn!("bad request: {e}");
                        return;
                    }
                }
            }
            Ok(image) => {
                tracing::warn!("refused a request from {}", image.display());
                Err(Error::Invalid(
                    "SerialLoom Helper only serves the installed SerialLoom".into(),
                ))
            }
            Err(e) => {
                tracing::warn!("could not identify the caller: {e}");
                Err(Error::Invalid(
                    "SerialLoom Helper could not identify the caller".into(),
                ))
            }
        };
    let reply = match outcome {
        Ok(Ok(ports)) => Reply {
            error: None,
            ports,
            refused: false,
        },
        Ok(Err(e)) => Reply {
            error: Some(e.to_string()),
            ports: Vec::new(),
            refused: false,
        },
        Err(e) => Reply {
            error: Some(e.to_string()),
            ports: Vec::new(),
            refused: true,
        },
    };
    let bytes = serde_json::to_vec(&reply).expect("reply serializes");
    // Refused callers get a short timeout and no waiting, so they cannot
    // keep the helper busy.
    let timeout = if trusted {
        REQUEST_TIMEOUT
    } else {
        Duration::from_millis(500)
    };
    if conn.write_line(&bytes, Some(timeout)).is_ok() && trusted {
        // Wait for SerialLoom to read the reply and hang up, so closing our
        // end never discards it.
        let _ = conn.read_line(16, Some(Duration::from_secs(5)));
    }
}

/// Parses and checks a request; an error means it must not run.
fn check_request(line: &[u8]) -> Result<AdminOp> {
    let req: Request =
        serde_json::from_slice(line).map_err(|e| Error::Invalid(format!("malformed request: {e}")))?;
    if req.v != PROTOCOL {
        return Err(Error::Invalid(format!(
            "SerialLoom Helper speaks version {PROTOCOL}, not {}; reinstall SerialLoom",
            req.v
        )));
    }
    let op = AdminOp::parse(&req.op)?;
    check_allowed(&op)?;
    Ok(op)
}

fn run_checked(
    op: &AdminOp,
    exec: &dyn Fn(&AdminOp) -> Result<Vec<VirtualPortInfo>>,
) -> Result<Vec<VirtualPortInfo>> {
    tracing::info!("helper operation: {:?}", op.to_args());
    let result = exec(op);
    match &result {
        Ok(_) => tracing::info!("helper operation succeeded"),
        Err(e) => tracing::error!("helper operation failed: {e}"),
    }
    result
}

#[cfg(test)]
fn run_request(
    line: &[u8],
    exec: &dyn Fn(&AdminOp) -> Result<Vec<VirtualPortInfo>>,
) -> Result<Vec<VirtualPortInfo>> {
    run_checked(&check_request(line)?, exec)
}

static STOP: AtomicBool = AtomicBool::new(false);
static STATUS_HANDLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

fn report(state: u32, exit_code: u32, wait_hint_ms: u32) {
    let handle = STATUS_HANDLE.load(Ordering::SeqCst);
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint_ms,
    };
    // SAFETY: handle came from RegisterServiceCtrlHandlerExW.
    unsafe { SetServiceStatus(handle, &status) };
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::SeqCst);
            report(SERVICE_STOP_PENDING, 0, 5000);
            0
        }
        SERVICE_CONTROL_INTERROGATE => 0,
        _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let name = wide(SERVICE_NAME);
    // SAFETY: name is NUL-terminated; the handler is a plain function.
    let handle =
        unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null()) };
    if handle.is_null() {
        return;
    }
    STATUS_HANDLE.store(handle, Ordering::SeqCst);
    report(SERVICE_START_PENDING, 0, 5000);

    let exit = match std::env::current_exe() {
        Ok(exe) => serve(
            PIPE_NAME,
            &pipe_sddl(),
            &STOP,
            IDLE_EXIT,
            &exe,
            &super::elevate::execute,
            &mut || report(SERVICE_RUNNING, 0, 0),
        ),
        Err(e) => Err(e),
    };
    let code = match exit {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("SerialLoom Helper stopped: {e}");
            1066 // ERROR_SERVICE_SPECIFIC_ERROR
        }
    };
    report(SERVICE_STOPPED, code, 0);
}

/// Entry point for `serialloom.exe --serialloom-service`. Returns the exit code.
pub fn service_entry() -> i32 {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: the table is terminated by a zeroed entry; blocks until the service stops.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let err = last_error();
        if err == ERROR_FAILED_SERVICE_CONTROLLER_CONNECT {
            eprintln!(
                "{SERVICE_FLAG} is used by Windows to run SerialLoom Helper; it cannot be started by hand."
            );
        }
        return 2;
    }
    0
}

// ---------------------------------------------------------------------------
// Installing (administrator only)
// ---------------------------------------------------------------------------

fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: raw receives a CoTaskMemAlloc'd string, freed below.
    let hr = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut raw) };
    let path = (hr == 0 && !raw.is_null()).then(|| {
        // SAFETY: raw is NUL-terminated.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        // SAFETY: len characters are readable.
        PathBuf::from(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(raw, len)
        }))
    });
    // SAFETY: freeing what SHGetKnownFolderPath allocated (null is fine).
    unsafe { CoTaskMemFree(raw.cast()) };
    path
}

/// The service runs as SYSTEM, so its executable must sit where only
/// administrators can change it.
fn in_program_files(exe: &Path) -> bool {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86]
        .iter()
        .filter_map(known_folder)
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .any(|dir| {
            exe.to_string_lossy()
                .to_lowercase()
                .starts_with(&format!("{}\\", dir.to_string_lossy().to_lowercase()))
        })
}

/// Registers (or updates) the service for the running executable.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?;
    if !in_program_files(&exe) {
        return Err(Error::Invalid(format!(
            "SerialLoom Helper can only be installed from Program Files, not {}",
            exe.display()
        )));
    }
    let binary = wide(&format!("\"{}\" {SERVICE_FLAG}", exe.display()));
    let display = wide(DISPLAY_NAME);
    let name = wide(SERVICE_NAME);

    let manager = open_manager(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
    // SAFETY: strings are NUL-terminated.
    let existing = unsafe { OpenServiceW(manager.0, name.as_ptr(), SERVICE_ALL_ACCESS) };
    let service = if existing.is_null() {
        // SAFETY: strings are NUL-terminated; null account means LocalSystem.
        let h = unsafe {
            CreateServiceW(
                manager.0,
                name.as_ptr(),
                display.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_DEMAND_START,
                SERVICE_ERROR_NORMAL,
                binary.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if h.is_null() {
            return Err(Error::Driver(format!(
                "could not register SerialLoom Helper (error {})",
                last_error()
            )));
        }
        ScHandle(h)
    } else {
        let service = ScHandle(existing);
        // SAFETY: strings are NUL-terminated; null keeps the account.
        let ok = unsafe {
            ChangeServiceConfigW(
                service.0,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_DEMAND_START,
                SERVICE_ERROR_NORMAL,
                binary.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                display.as_ptr(),
            )
        };
        if ok == 0 {
            return Err(Error::Driver(format!(
                "could not update SerialLoom Helper (error {})",
                last_error()
            )));
        }
        service
    };

    let mut text = wide(DESCRIPTION);
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: text.as_mut_ptr(),
    };
    // SAFETY: description points to a NUL-terminated string.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            (&description as *const SERVICE_DESCRIPTIONW).cast(),
        )
    };

    let sd = SecurityDescriptor::from_sddl(SERVICE_SDDL)?;
    // SAFETY: sd is a valid self-relative descriptor.
    if unsafe { SetServiceObjectSecurity(service.0, DACL_SECURITY_INFORMATION, sd.raw()) } == 0 {
        return Err(Error::Driver(format!(
            "could not set SerialLoom Helper permissions (error {})",
            last_error()
        )));
    }
    tracing::info!("SerialLoom Helper installed for {}", exe.display());
    Ok(())
}

/// Stops and removes the service. Not installed counts as success.
pub fn uninstall() -> Result<()> {
    let Some(service) = open_service(SERVICE_ALL_ACCESS, SC_MANAGER_CONNECT)? else {
        return Ok(());
    };
    let mut st = SERVICE_STATUS::default();
    // SAFETY: st is a valid out-struct.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut st) } != 0 {
        let deadline = Instant::now() + Duration::from_secs(10);
        while query(&service).is_ok_and(|s| s.dwCurrentState != SERVICE_STOPPED) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    } else {
        let err = last_error();
        if err != ERROR_SERVICE_NOT_ACTIVE {
            tracing::warn!("stopping SerialLoom Helper failed (error {err})");
        }
    }
    // SAFETY: valid service handle opened with DELETE access.
    if unsafe { DeleteService(service.0) } == 0 {
        let err = last_error();
        // Already marked for deletion: it goes away once all handles close.
        if err != 1072 {
            return Err(Error::Driver(format!(
                "could not remove SerialLoom Helper (error {err})"
            )));
        }
    }
    tracing::info!("SerialLoom Helper removed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::winpipe::current_user_sid;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn allows_only_port_operations_on_serialloom_devices() {
        let ok = |args: &[&str]| check_allowed(&AdminOp::parse(&s(args)).unwrap()).is_ok();
        assert!(ok(&["rename-usb", "SL0123456789=COM20"]));
        assert!(ok(&["remove-usb", "SL0123456789", "SLABCDEF0123"]));
        assert!(ok(&["create", "COM20"]));
        assert!(ok(&["remove", "COM20"]));
        // Real hardware serials pass the generic parser, but not the helper.
        assert!(!ok(&["rename-usb", "A600XYZ=COM20"]));
        assert!(!ok(&["remove-usb", "FT232R01"]));
        for op in [
            "install-driver",
            "install-usb",
            "uninstall-all",
            "install-service",
            "uninstall-service",
        ] {
            assert!(!ok(&[op]), "{op} must stay UAC-only");
        }
    }

    #[test]
    fn rejects_malformed_and_foreign_requests() {
        let never = |_: &AdminOp| -> Result<Vec<VirtualPortInfo>> { panic!("must not execute") };
        let run = |line: &str| run_request(line.as_bytes(), &never).unwrap_err().to_string();
        assert!(run("not json").contains("malformed"));
        assert!(run(r#"{"v":1,"op":["create","COM5"],"extra":1}"#).contains("malformed"));
        assert!(run(r#"{"v":2,"op":["create","COM5"]}"#).contains("version"));
        assert!(run(r#"{"v":1,"op":["install-driver"]}"#).contains("not available"));
        assert!(run(r#"{"v":1,"op":["create","COM5 & calc"]}"#).contains("not a valid port name"));
        assert!(run(r#"{"v":1,"op":["rename-usb","FT232R01=COM9"]}"#).contains("not a SerialLoom device"));
    }

    /// Runs the real server loop on a private pipe name with a fake executor
    /// and talks to it like the app does.
    fn with_server(allowed_client: PathBuf, test: impl FnOnce(&str)) -> usize {
        let pipe = format!(
            r"\\.\pipe\SerialLoom.Test.Helper.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        // The test server runs as this user, not SYSTEM, so this user needs
        // full rights to create pipe instances.
        let sddl = format!(
            "D:P(D;;GA;;;NU)(A;;GA;;;SY)(A;;GA;;;{})",
            current_user_sid().unwrap()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let server = {
            let (pipe, stop, calls) = (pipe.clone(), Arc::clone(&stop), Arc::clone(&calls));
            std::thread::spawn(move || {
                let exec = move |op: &AdminOp| -> Result<Vec<VirtualPortInfo>> {
                    calls.fetch_add(1, Ordering::SeqCst);
                    match op {
                        AdminOp::RenameUsbPorts(_) => Ok(Vec::new()),
                        _ => Err(Error::Driver("simulated failure".into())),
                    }
                };
                serve(
                    &pipe,
                    &sddl,
                    &stop,
                    Duration::from_secs(60),
                    &allowed_client,
                    &exec,
                    &mut || ready_tx.send(()).unwrap(),
                )
                .unwrap();
            })
        };
        ready_rx.recv().unwrap();
        test(&pipe);
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        calls.load(Ordering::SeqCst)
    }

    fn ask(pipe: &str, op: &[&str]) -> Reply {
        let mut c =
            PipeStream::connect(pipe, Duration::from_secs(5)).unwrap_or_else(|e| panic!("{op:?}: {e}"));
        assert_eq!(c.server_process_id().unwrap(), std::process::id());
        let req = serde_json::to_vec(&Request {
            v: PROTOCOL,
            op: s(op),
        })
        .unwrap();
        c.write_line(&req, Some(Duration::from_secs(5))).unwrap();
        let line = c
            .read_line(1 << 20, Some(Duration::from_secs(10)))
            .unwrap()
            .unwrap();
        serde_json::from_slice(&line).unwrap()
    }

    #[test]
    fn serves_checked_requests_from_serialloom() {
        let calls = with_server(std::env::current_exe().unwrap(), |pipe| {
            let r = ask(pipe, &["rename-usb", "SL0123456789=COM20"]);
            assert_eq!(r.error, None);
            assert!(!r.refused);
            // It ran and failed: the app must not retry it through UAC.
            let r = ask(pipe, &["remove-usb", "SL0123456789"]);
            assert_eq!(r.error.as_deref(), Some("simulated failure"));
            assert!(!r.refused);
            // Turned down without running: UAC may be asked instead.
            let r = ask(pipe, &["install-driver"]);
            assert!(r.error.unwrap().contains("not available"));
            assert!(r.refused);
        });
        assert_eq!(calls, 2, "the refused request never reached the executor");
    }

    #[test]
    fn refuses_other_programs() {
        let calls = with_server(
            PathBuf::from(r"C:\Program Files\SerialLoom\serialloom.exe"),
            |pipe| {
                // The helper answers a refused caller without reading its
                // request and hangs up, so the request may or may not get
                // through before the pipe closes. Either way the app's client
                // must fall back to UAC. Repeat to cover both orders.
                let op = AdminOp::parse(&s(&["rename-usb", "SL0123456789=COM20"])).unwrap();
                for _ in 0..20 {
                    let conn = PipeStream::connect(pipe, Duration::from_secs(5)).unwrap();
                    assert!(
                        send(conn, &op).is_none(),
                        "another SerialLoom copy may fall back to UAC"
                    );
                }
            },
        );
        assert_eq!(calls, 0);
    }

    #[test]
    fn service_install_requires_program_files() {
        assert!(!in_program_files(Path::new(
            r"C:\Users\someone\Downloads\serialloom.exe"
        )));
        assert!(!in_program_files(Path::new(
            r"C:\Program Files Evil\serialloom.exe"
        )));
        if let Some(pf) = known_folder(&FOLDERID_ProgramFiles) {
            // Must exist for canonicalize; the folder itself is enough here.
            let inside = pf.join("Common Files");
            if inside.exists() {
                assert!(in_program_files(&inside));
            }
        }
    }
}
