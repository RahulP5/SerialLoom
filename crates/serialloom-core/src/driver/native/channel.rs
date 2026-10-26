//! Overlapped I/O on the control side of a virtual port.

use std::io;
use std::time::Duration;

use serialloom_proto as proto;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GENERIC_READ, GENERIC_WRITE, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

use crate::driver::{ControlChannel, ControlState};

const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct NativeChannel {
    handle: HANDLE,
}

// SAFETY: a file HANDLE may be used from any thread; overlapped calls on the
// same handle from different threads are allowed by Windows.
unsafe impl Send for NativeChannel {}
// SAFETY: see above; every call uses its own OVERLAPPED and event.
unsafe impl Sync for NativeChannel {}

impl NativeChannel {
    pub fn open(path: &str) -> io::Result<Self> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: path is NUL-terminated; the handle is closed on drop.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(5) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the port is already attached to another SerialLoom hub",
                ));
            }
            return Err(err);
        }
        Ok(Self { handle })
    }

    /// Runs one overlapped operation and waits for it, cancelling after `timeout`.
    fn run(
        &self,
        timeout: Option<Duration>,
        start: impl FnOnce(*mut OVERLAPPED) -> i32,
    ) -> io::Result<usize> {
        // SAFETY: manual-reset, initially unsignalled, unnamed event.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OVERLAPPED is a plain C struct; zero is its initial state.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = event;

        let result = (|| {
            if start(&mut ov) == 0 {
                // SAFETY: reads this thread's last error.
                let err = unsafe { GetLastError() };
                if err != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(err as i32));
                }
                let ms = timeout.map_or(INFINITE, |t| t.as_millis().min(u128::from(INFINITE - 1)) as u32);
                // SAFETY: event is valid until closed below.
                if unsafe { WaitForSingleObject(event, ms) } == WAIT_TIMEOUT {
                    // SAFETY: cancels only this operation.
                    unsafe { CancelIoEx(self.handle, &ov) };
                }
            }
            let mut n = 0u32;
            // SAFETY: waits until the operation (or its cancellation) finished,
            // so the caller's buffer is no longer in use afterwards.
            if unsafe { GetOverlappedResult(self.handle, &ov, &mut n, 1) } == 0 {
                // SAFETY: reads this thread's last error.
                let err = unsafe { GetLastError() };
                if err == ERROR_OPERATION_ABORTED {
                    return Ok(n as usize);
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            Ok(n as usize)
        })();

        // SAFETY: closes the event created above.
        unsafe { CloseHandle(event) };
        result
    }
}

impl ControlChannel for NativeChannel {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_mut_ptr();
        // SAFETY: buffer stays valid until `run` has waited for completion.
        self.run(Some(timeout), |ov| unsafe {
            ReadFile(self.handle, ptr, len, std::ptr::null_mut(), ov)
        })
    }

    fn write(&self, data: &[u8]) -> io::Result<usize> {
        let len = data.len().min(u32::MAX as usize) as u32;
        // SAFETY: data stays valid until `run` has waited for completion.
        self.run(Some(WRITE_TIMEOUT), |ov| unsafe {
            WriteFile(self.handle, data.as_ptr(), len, std::ptr::null_mut(), ov)
        })
    }

    fn state(&self) -> Option<ControlState> {
        let mut state = ControlState::default();
        let out = std::ptr::from_mut(&mut state).cast::<core::ffi::c_void>();
        // SAFETY: `state` is a repr(C) struct of PORT_STATE_SIZE bytes that
        // outlives the call.
        let n = self
            .run(Some(Duration::from_secs(1)), |ov| unsafe {
                DeviceIoControl(
                    self.handle,
                    proto::IOCTL_SERIALLOOM_GET_STATE,
                    std::ptr::null(),
                    0,
                    out,
                    proto::PORT_STATE_SIZE as u32,
                    std::ptr::null_mut(),
                    ov,
                )
            })
            .ok()?;
        (n == proto::PORT_STATE_SIZE).then_some(state)
    }
}

impl Drop for NativeChannel {
    fn drop(&mut self) {
        // SAFETY: closes the handle opened in `open`.
        unsafe { CloseHandle(self.handle) };
    }
}
