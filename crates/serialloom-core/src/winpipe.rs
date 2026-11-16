//! Local named pipes that only the intended parties can use.
//!
//! Used by the SerialLoom Helper service (administrator side) and by the
//! automation API of the running app. Both carry newline-delimited JSON.
//!
//! Protections, in order:
//! * Every pipe has an explicit security descriptor (SDDL) set by the caller,
//!   and rejects connections from other computers.
//! * The server creates its pipe with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so it
//!   fails loudly instead of sharing a name another process took first.
//! * Clients connect at identification level only, so a server can never act
//!   with the client's rights, and check who runs the server before sending
//!   anything (see [`PipeStream::server_process_id`] and [`process_user_sid`]).
//! * All I/O is overlapped with timeouts and size limits, so a client that
//!   stalls or floods cannot hang a server thread.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, SYNCHRONIZE, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, GetNamedPipeServerSessionId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ResetEvent, WaitForSingleObject,
};

/// Access a client asks for: read and write data, nothing else. In
/// particular not `FILE_APPEND_DATA`, which on a pipe means "create another
/// server instance", so a pipe can grant clients exactly this much.
pub const CLIENT_ACCESS: u32 = FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

/// SDDL access mask matching [`CLIENT_ACCESS`] (plus reading attributes and
/// the security descriptor). Grant this, never `GW`/`GA`, to clients that
/// must not be able to create server instances of the pipe.
pub const CLIENT_RIGHTS_SDDL: &str = "0x120183";

const BUFFER_SIZE: u32 = 16 * 1024;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: reads this thread's last error.
    unsafe { GetLastError() }
}

fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

/// A kernel handle closed on drop.
#[derive(Debug)]
pub struct Handle(HANDLE);

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Handle {}

impl Handle {
    fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn new_event() -> io::Result<Handle> {
    // SAFETY: unnamed manual-reset event, initially not signalled.
    Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
}

fn millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(u32::MAX, |t| t.as_millis().min(u128::from(u32::MAX - 1)) as u32)
}

/// A security descriptor built from SDDL.
pub(crate) struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable once built.
unsafe impl Send for SecurityDescriptor {}

impl SecurityDescriptor {
    pub(crate) fn raw(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }

    pub(crate) fn from_sddl(sddl: &str) -> io::Result<Self> {
        let text = wide(sddl);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: text is NUL-terminated; sd receives a LocalAlloc'd buffer.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(sd))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0) };
    }
}

/// Runs one overlapped operation on `handle`, waiting up to `timeout`.
/// `start` issues the call with the given OVERLAPPED and returns its BOOL.
fn overlapped_io(
    handle: HANDLE,
    event: &Handle,
    timeout: Option<Duration>,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> io::Result<u32> {
    let mut ov = OVERLAPPED {
        hEvent: event.raw(),
        ..Default::default()
    };
    if start(&mut ov) == 0 {
        let err = last_error();
        if err != ERROR_IO_PENDING {
            return Err(os_error(err));
        }
        // SAFETY: event is a valid handle.
        let wait = unsafe { WaitForSingleObject(event.raw(), millis(timeout)) };
        if wait != WAIT_OBJECT_0 {
            // Cancel, then wait for the cancellation so `ov` is no longer in use.
            let mut n = 0u32;
            // SAFETY: ov stays alive until GetOverlappedResult(wait = TRUE) returns.
            let done = unsafe {
                CancelIoEx(handle, &ov);
                GetOverlappedResult(handle, &ov, &mut n, 1)
            };
            if done != 0 {
                return Ok(n);
            }
            return Err(if wait == WAIT_TIMEOUT {
                io::Error::new(io::ErrorKind::TimedOut, "timed out")
            } else {
                io::Error::last_os_error()
            });
        }
    }
    let mut n = 0u32;
    // SAFETY: the operation has completed; ov is still alive.
    if unsafe { GetOverlappedResult(handle, &ov, &mut n, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------

/// A pipe instance waiting for a client. The OVERLAPPED lives in a box so
/// its address stays fixed while the connect is outstanding.
struct PendingConnect {
    pipe: Option<Handle>,
    event: Handle,
    ov: Box<OVERLAPPED>,
    /// A client is connected (or nothing is outstanding): no wait needed.
    connected: bool,
}

// SAFETY: the OVERLAPPED only holds the event handle; its boxed address
// stays fixed when the listener moves to another thread.
unsafe impl Send for PendingConnect {}

impl PendingConnect {
    fn raw_pipe(&self) -> HANDLE {
        self.pipe.as_ref().expect("present until handed over").raw()
    }

    /// Starts waiting for a client on this instance. A client that came
    /// and left before we got here is disconnected and the same instance
    /// waits again, so the pipe name never lapses.
    fn arm(&mut self) -> io::Result<()> {
        let pipe = self.raw_pipe();
        for _ in 0..64 {
            self.connected = true; // nothing outstanding until IO_PENDING
            *self.ov = OVERLAPPED {
                hEvent: self.event.raw(),
                ..Default::default()
            };
            // SAFETY: ov is boxed and outlives the operation (see Drop).
            unsafe { ResetEvent(self.event.raw()) };
            // SAFETY: as above.
            if unsafe { ConnectNamedPipe(pipe, &mut *self.ov) } != 0 {
                return Ok(());
            }
            match last_error() {
                ERROR_PIPE_CONNECTED => return Ok(()),
                ERROR_IO_PENDING => {
                    self.connected = false;
                    return Ok(());
                }
                ERROR_NO_DATA => {
                    // SAFETY: valid server end of the pipe.
                    unsafe { DisconnectNamedPipe(pipe) };
                }
                err => return Err(os_error(err)),
            }
        }
        Err(io::Error::other("clients keep disconnecting immediately"))
    }
}

impl Drop for PendingConnect {
    fn drop(&mut self) {
        if let Some(pipe) = &self.pipe
            && !self.connected
        {
            let mut n = 0u32;
            // SAFETY: cancel the outstanding connect and wait until the
            // system no longer uses `ov`, before the box is freed.
            unsafe {
                CancelIoEx(pipe.raw(), &*self.ov);
                GetOverlappedResult(pipe.raw(), &*self.ov, &mut n, 1);
            }
        }
    }
}

/// Accepts clients on one pipe name.
pub struct PipeListener {
    name: Vec<u16>,
    security: SecurityDescriptor,
    max_instances: u32,
    pending: Option<PendingConnect>,
}

impl PipeListener {
    /// Creates the pipe `name` (e.g. `\\.\pipe\SerialLoom.Helper`) protected
    /// by `sddl`. Fails if any process already created a pipe of that name.
    pub fn bind(name: &str, sddl: &str, max_instances: u32) -> io::Result<Self> {
        let mut listener = Self {
            name: wide(name),
            security: SecurityDescriptor::from_sddl(sddl)?,
            max_instances: max_instances.clamp(1, 254),
            pending: None,
        };
        // Claim the name right away so squatting is detected here.
        listener.pending = Some(listener.listen(true).map_err(|e| {
            // ACCESS_DENIED: the name exists (first-instance flag);
            // PIPE_BUSY: it exists and has no free instance.
            if matches!(e.raw_os_error(), Some(5) | Some(231)) {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another program already created the pipe {name}"),
                )
            } else {
                e
            }
        })?);
        Ok(listener)
    }

    fn listen(&self, first: bool) -> io::Result<PendingConnect> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.0,
            bInheritHandle: 0,
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: name is NUL-terminated; sa points to a valid descriptor.
        let pipe = Handle::new(unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                self.max_instances,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                &sa,
            )
        })?;
        let event = new_event()?;
        let mut pending = PendingConnect {
            pipe: Some(pipe),
            ov: Box::new(OVERLAPPED {
                hEvent: event.raw(),
                ..Default::default()
            }),
            event,
            connected: true,
        };
        pending.arm()?;
        Ok(pending)
    }

    /// Waits up to `timeout` for a client. `Ok(None)` means no client came;
    /// the pipe keeps listening, so call again.
    pub fn accept(&mut self, timeout: Option<Duration>) -> io::Result<Option<PipeStream>> {
        if self.pending.is_none() {
            self.pending = Some(self.listen(false)?);
        }
        let pending = self.pending.as_mut().expect("set above");
        if !pending.connected {
            // SAFETY: event is a valid handle.
            let wait = unsafe { WaitForSingleObject(pending.event.raw(), millis(timeout)) };
            if wait == WAIT_TIMEOUT {
                return Ok(None);
            }
            let mut n = 0u32;
            // SAFETY: the connect completed (or failed); ov is alive.
            let ok = unsafe { GetOverlappedResult(pending.raw_pipe(), &*pending.ov, &mut n, 0) };
            pending.connected = true; // no longer outstanding, either way
            if ok == 0 {
                let err = io::Error::last_os_error();
                let gone = matches!(
                    err.raw_os_error().map(|c| c as u32),
                    Some(ERROR_NO_DATA | ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED)
                );
                if gone {
                    // The client came and left: wait again on the same
                    // instance, so the name never lapses.
                    // SAFETY: valid server end of the pipe.
                    unsafe { DisconnectNamedPipe(pending.raw_pipe()) };
                    if pending.arm().is_ok() {
                        return Ok(None);
                    }
                }
                // Listen on a fresh instance before dropping this one.
                self.pending = self.listen(false).ok();
                return if gone { Ok(None) } else { Err(err) };
            }
        }
        let mut pending = self.pending.take().expect("set above");
        let pipe = pending.pipe.take().expect("present until handed over");
        // Start listening again before handing this one out, so the name
        // never lapses: clients always find the pipe, and no other process
        // can create one of the same name in between.
        match self.listen(false) {
            Ok(next) => self.pending = Some(next),
            Err(e) => tracing::warn!("could not create the next pipe instance: {e}"),
        }
        Ok(Some(PipeStream {
            pipe,
            event: new_event()?,
            buf: Vec::new(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Both sides
// ---------------------------------------------------------------------------

/// One connection, server or client side.
pub struct PipeStream {
    pipe: Handle,
    event: Handle,
    buf: Vec<u8>,
}

impl PipeStream {
    /// Connects to the pipe `name`, waiting up to `timeout` while all its
    /// instances are busy. A missing pipe is `ErrorKind::NotFound`.
    pub fn connect(name: &str, timeout: Duration) -> io::Result<Self> {
        let name_w = wide(name);
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: name is NUL-terminated. Identification level only: the
            // server can learn who we are but never act as us.
            let raw = unsafe {
                CreateFileW(
                    name_w.as_ptr(),
                    CLIENT_ACCESS,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    std::ptr::null_mut(),
                )
            };
            if raw != INVALID_HANDLE_VALUE {
                return Ok(Self {
                    pipe: Handle(raw),
                    event: new_event()?,
                    buf: Vec::new(),
                });
            }
            let err = last_error();
            let left = deadline.saturating_duration_since(Instant::now());
            match err {
                ERROR_PIPE_BUSY if !left.is_zero() => {
                    // SAFETY: name is NUL-terminated.
                    unsafe { WaitNamedPipeW(name_w.as_ptr(), millis(Some(left)).max(1)) };
                }
                ERROR_PIPE_BUSY => {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the pipe stayed busy"));
                }
                ERROR_FILE_NOT_FOUND => {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "nobody is listening"));
                }
                _ => return Err(os_error(err)),
            }
        }
    }

    /// Process at the other end, for a server checking its client.
    pub fn client_process_id(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        // SAFETY: out-pointer to a u32.
        if unsafe { GetNamedPipeClientProcessId(self.pipe.raw(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pid)
    }

    /// Process serving the pipe, for a client checking its server.
    pub fn server_process_id(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        // SAFETY: out-pointer to a u32.
        if unsafe { GetNamedPipeServerProcessId(self.pipe.raw(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pid)
    }

    /// Windows session of the process serving the pipe.
    pub fn server_session_id(&self) -> io::Result<u32> {
        let mut id = 0u32;
        // SAFETY: out-pointer to a u32.
        if unsafe { GetNamedPipeServerSessionId(self.pipe.raw(), &mut id) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(id)
    }

    /// Reads one `\n`-terminated line (without the `\n`). `Ok(None)` means
    /// the other side closed the connection. Lines longer than `max` bytes
    /// are an error, as is waiting longer than `timeout` in total.
    pub fn read_line(&mut self, max: usize, timeout: Option<Duration>) -> io::Result<Option<Vec<u8>>> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                if pos > max {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "message too long"));
                }
                let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if self.buf.len() > max {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "message too long"));
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left.is_some_and(|l| l.is_zero()) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            let mut chunk = [0u8; 4096];
            let read = overlapped_io(self.pipe.raw(), &self.event, left, |ov| {
                // SAFETY: chunk outlives the call (overlapped_io waits for completion).
                unsafe {
                    ReadFile(
                        self.pipe.raw(),
                        chunk.as_mut_ptr(),
                        chunk.len() as u32,
                        std::ptr::null_mut(),
                        ov,
                    )
                }
            });
            match read {
                Ok(0) => continue,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n as usize]),
                Err(e)
                    if matches!(
                        e.raw_os_error().map(|c| c as u32),
                        Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA)
                    ) =>
                {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed mid-message",
                    ));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Writes `data` followed by `\n`.
    pub fn write_line(&mut self, data: &[u8], timeout: Option<Duration>) -> io::Result<()> {
        let mut msg = Vec::with_capacity(data.len() + 1);
        msg.extend_from_slice(data);
        msg.push(b'\n');
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut sent = 0usize;
        while sent < msg.len() {
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let rest = &msg[sent..];
            let n = overlapped_io(self.pipe.raw(), &self.event, left, |ov| {
                // SAFETY: msg outlives the call.
                unsafe {
                    WriteFile(
                        self.pipe.raw(),
                        rest.as_ptr(),
                        rest.len() as u32,
                        std::ptr::null_mut(),
                        ov,
                    )
                }
            })?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the pipe accepted no data",
                ));
            }
            sent += n as usize;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Process identity
// ---------------------------------------------------------------------------

fn open_process(pid: u32) -> io::Result<Handle> {
    // SAFETY: plain open with a query-only right.
    Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// Full path of the executable running as process `pid`.
pub fn process_image(pid: u32) -> io::Result<PathBuf> {
    let process = open_process(pid)?;
    let mut buf = vec![0u16; 32 * 1024];
    let mut len = buf.len() as u32;
    // SAFETY: len is the buffer size in characters.
    if unsafe { QueryFullProcessImageNameW(process.raw(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// SID (e.g. `S-1-5-21-…`) of the user running process `pid`.
pub fn process_user_sid(pid: u32) -> io::Result<String> {
    let process = open_process(pid)?;
    let mut token_raw: HANDLE = std::ptr::null_mut();
    // SAFETY: out-pointer to a handle.
    if unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token_raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle::new(token_raw)?;
    // TOKEN_USER plus the SID it points into; 256 bytes is plenty.
    let mut buf = vec![0u64; 64];
    let mut needed = 0u32;
    // SAFETY: buffer size in bytes as passed; u64 storage keeps it aligned.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation filled a TOKEN_USER at the start of buf.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: sid points into buf; text receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: text is NUL-terminated.
    let s = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    };
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text.cast()) };
    Ok(s)
}

/// SID of the user running this process.
pub fn current_user_sid() -> io::Result<String> {
    // SAFETY: plain query.
    process_user_sid(unsafe { GetCurrentProcessId() })
}

/// Windows session this process runs in.
pub fn current_session_id() -> io::Result<u32> {
    let mut id = 0u32;
    // SAFETY: out-pointer to a u32.
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(id)
}

/// True when both paths name the same file (case-insensitive, resolved).
pub fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(tag: &str) -> String {
        format!(
            r"\\.\pipe\SerialLoom.Test.{tag}.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        )
    }

    fn own_only() -> String {
        format!("D:P(A;;GA;;;{})", current_user_sid().unwrap())
    }

    #[test]
    fn round_trip_and_identities() {
        let name = unique("rt");
        let mut listener = PipeListener::bind(&name, &own_only(), 4).unwrap();
        let client = {
            let name = name.clone();
            std::thread::spawn(move || {
                let mut c = PipeStream::connect(&name, Duration::from_secs(5)).unwrap();
                assert_eq!(c.server_process_id().unwrap(), std::process::id());
                c.write_line(b"hello", Some(Duration::from_secs(5))).unwrap();
                let reply = c.read_line(1024, Some(Duration::from_secs(5))).unwrap().unwrap();
                assert_eq!(reply, b"world");
            })
        };
        let mut s = loop {
            if let Some(s) = listener.accept(Some(Duration::from_secs(5))).unwrap() {
                break s;
            }
        };
        assert_eq!(s.client_process_id().unwrap(), std::process::id());
        let line = s.read_line(1024, Some(Duration::from_secs(5))).unwrap().unwrap();
        assert_eq!(line, b"hello");
        s.write_line(b"world", Some(Duration::from_secs(5))).unwrap();
        client.join().unwrap();
        // The client closed: a clean end of stream.
        assert!(s.read_line(1024, Some(Duration::from_secs(5))).unwrap().is_none());
    }

    #[test]
    fn refuses_a_name_already_taken() {
        let name = unique("squat");
        let _first = PipeListener::bind(&name, &own_only(), 1).unwrap();
        let err = PipeListener::bind(&name, &own_only(), 1).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn accept_times_out_and_keeps_listening() {
        let name = unique("idle");
        let mut listener = PipeListener::bind(&name, &own_only(), 1).unwrap();
        assert!(
            listener
                .accept(Some(Duration::from_millis(50)))
                .unwrap()
                .is_none()
        );
        let t = std::thread::spawn(move || PipeStream::connect(&name, Duration::from_secs(5)).map(|_| ()));
        let s = listener.accept(Some(Duration::from_secs(5))).unwrap();
        assert!(s.is_some());
        t.join().unwrap().unwrap();
    }

    #[test]
    fn limits_message_size_and_time() {
        let name = unique("limits");
        let mut listener = PipeListener::bind(&name, &own_only(), 2).unwrap();
        let n2 = name.clone();
        let t = std::thread::spawn(move || {
            let mut c = PipeStream::connect(&n2, Duration::from_secs(5)).unwrap();
            c.write_line(&vec![b'x'; 10_000], None).unwrap();
            // Keep the connection open until the server gave up.
            let _ = c.read_line(16, Some(Duration::from_secs(5)));
        });
        let mut s = listener.accept(Some(Duration::from_secs(5))).unwrap().unwrap();
        let err = s.read_line(1000, Some(Duration::from_secs(5))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(s);
        t.join().unwrap();

        // A client that sends nothing hits the read timeout.
        let n3 = name.clone();
        let t = std::thread::spawn(move || {
            let c = PipeStream::connect(&n3, Duration::from_secs(5)).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drop(c);
        });
        let mut s = listener.accept(Some(Duration::from_secs(5))).unwrap().unwrap();
        let err = s.read_line(1000, Some(Duration::from_millis(100))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        t.join().unwrap();
    }

    #[test]
    fn client_rights_cannot_create_server_instances() {
        // The helper's pipe grants local users exactly CLIENT_RIGHTS_SDDL.
        // Such a user can connect, but cannot add a server instance to
        // intercept other clients.
        let name = unique("rights");
        let sddl = format!("D:P(A;;{CLIENT_RIGHTS_SDDL};;;{})", current_user_sid().unwrap());
        let mut listener = PipeListener::bind(&name, &sddl, 4).unwrap();
        let rogue = listener
            .listen(false)
            .err()
            .expect("creating an instance must be denied");
        assert_eq!(rogue.raw_os_error(), Some(5), "{rogue}");
        let n2 = name.clone();
        let client = std::thread::spawn(move || PipeStream::connect(&n2, Duration::from_secs(5)).map(|_| ()));
        assert!(listener.accept(Some(Duration::from_secs(5))).unwrap().is_some());
        client.join().unwrap().unwrap();
    }

    #[test]
    fn missing_pipe_is_not_found() {
        let err = PipeStream::connect(&unique("none"), Duration::from_millis(100))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn identifies_this_process() {
        let me = std::process::id();
        assert!(same_file(
            &process_image(me).unwrap(),
            &std::env::current_exe().unwrap()
        ));
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
        assert_eq!(process_user_sid(me).unwrap(), current_user_sid().unwrap());
    }
}
