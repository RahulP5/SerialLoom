//! # SerialLoom virtual serial port driver (UMDF 2)
//!
//! One device instance is one virtual COM port. Programs open the COM side
//! (`\\.\COM20`) and see an ordinary serial port. The SerialLoom app opens the
//! control side (device interface + `\control`) and moves the bytes. See the
//! `serialloom-proto` crate for the contract.
//!
//! UMDF drivers run in a user-mode host process, so this crate uses `std`.
//!
//! Locking rule: WDF requests are never completed while the port mutex is
//! held, because completing a request can make the framework call back into
//! the driver on the same thread.

mod wdf;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use serialloom_proto as proto;
use wdk::println;
use wdk_sys::{
    NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, PWDFDEVICE_INIT, STATUS_ACCESS_DENIED,
    STATUS_BUFFER_TOO_SMALL, STATUS_CANCELLED, STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER,
    STATUS_NOT_SUPPORTED, STATUS_SUCCESS, STATUS_TIMEOUT, WDFDEVICE, WDFDRIVER, WDFFILEOBJECT, WDFOBJECT,
    WDFQUEUE, WDFREQUEST, WDFTIMER,
};

use serialloom_port_logic::{self as logic, PortState, ReadDecision, serial};
use wdf::{Completion, Handle};

/// Which side of the port a file handle belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Com,
    Control,
}

struct Inner {
    state: PortState,
    files: HashMap<Handle, Role>,
    /// When each pending COM-side read arrived (keyed by request handle).
    read_started: HashMap<Handle, Instant>,
}

struct Port {
    com_reads: Handle,
    control_reads: Handle,
    waits: Handle,
    timer: Option<Handle>,
    /// `(PDO name, COM name)` written to HARDWARE\DEVICEMAP\SERIALCOMM.
    devicemap: Option<(String, String)>,
    inner: Mutex<Inner>,
}

impl Port {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves the state usable; keep serving.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn ports() -> &'static Mutex<HashMap<Handle, Arc<Port>>> {
    static PORTS: OnceLock<Mutex<HashMap<Handle, Arc<Port>>>> = OnceLock::new();
    PORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn port_of(device: WDFDEVICE) -> Option<Arc<Port>> {
    ports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&Handle::from(device))
        .cloned()
}

// ---------------------------------------------------------------------------
// Driver and device setup
// ---------------------------------------------------------------------------

/// # Safety
/// Called by WDF with valid driver object and registry path pointers.
#[cfg_attr(not(test), unsafe(export_name = "DriverEntry"))]
pub unsafe extern "system" fn driver_entry(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    // SAFETY: arguments come straight from the framework.
    let status = unsafe { wdf::create_driver(driver, registry_path, Some(evt_device_add)) };
    println!("SerialLoom port driver loaded: {status:#x}");
    status
}

extern "C" fn evt_device_add(_driver: WDFDRIVER, device_init: PWDFDEVICE_INIT) -> NTSTATUS {
    // SAFETY: device_init is valid during EvtDriverDeviceAdd.
    match unsafe { add_device(device_init) } {
        Ok(()) => STATUS_SUCCESS,
        Err(status) => {
            println!("SerialLoom: device add failed {status:#x}");
            status
        }
    }
}

/// Startup log written to the device's registry key (value
/// `SerialLoomStartup`) so the app can show exactly which step failed.
#[derive(Default)]
struct Report {
    steps: Vec<String>,
}

impl Report {
    fn step<T>(&mut self, name: &str, result: Result<T, NTSTATUS>) -> Result<T, NTSTATUS> {
        match &result {
            Ok(_) => self.steps.push(format!("{name}: ok")),
            Err(e) => self.steps.push(format!("{name}: FAILED {e:#010x}")),
        }
        result
    }

    fn note(&mut self, text: String) {
        self.steps.push(text);
    }

    fn text(&self) -> String {
        format!(
            "driver {} | {}",
            env!("CARGO_PKG_VERSION"),
            self.steps.join(" | ")
        )
    }
}

/// # Safety
/// `device_init` must be the pointer passed to EvtDriverDeviceAdd.
unsafe fn add_device(device_init: PWDFDEVICE_INIT) -> Result<(), NTSTATUS> {
    // SAFETY: forwarded from the caller's contract.
    let device = unsafe {
        wdf::set_file_callbacks(device_init, evt_file_create, evt_file_cleanup);
        wdf::create_device(device_init, evt_device_cleanup)?
    };

    let mut report = Report::default();
    let result = configure(device, &mut report);
    if let Err(e) = &result {
        report.note(format!("device start aborted with {e:#010x}"));
    } else {
        report.note("started".into());
    }
    let text = report.text();
    println!("SerialLoom: {text}");
    if let Err(e) = wdf::write_startup_report(device, &text) {
        println!("SerialLoom: could not store startup report {e:#x}");
    }
    result
}

/// Sets up interfaces, the COM name, queues and the timer. Only failures that
/// make the port unusable abort the start; the rest are recorded.
fn configure(device: WDFDEVICE, report: &mut Report) -> Result<(), NTSTATUS> {
    let _ = report.step(
        "comport interface",
        wdf::create_interface(device, &proto::COMPORT_INTERFACE_GUID, None),
    );
    report.step(
        "control interface",
        wdf::create_interface(
            device,
            &proto::CONTROL_INTERFACE_GUID,
            Some(proto::CONTROL_REFERENCE),
        ),
    )?;

    // The app stores the assigned name, e.g. COM20, in the hardware key.
    let mut devicemap = None;
    if let Ok(name) = report.step("read PortName", wdf::read_port_name(device)) {
        report.note(format!("PortName={name}"));
        let _ = report.step(
            "symbolic link",
            wdf::create_symbolic_link(device, &format!(r"\DosDevices\Global\{name}")),
        );
        if let Ok(pdo) = report.step("PDO name", wdf::pdo_name(device))
            && report
                .step("SERIALCOMM entry", wdf::devicemap_set(device, &pdo, &name))
                .is_ok()
        {
            devicemap = Some((pdo, name));
        }
    }

    report.step(
        "default queue",
        wdf::create_default_queue(device, evt_io_read, evt_io_write, evt_io_device_control),
    )?;
    let com_reads = report.step("read queue", wdf::create_manual_queue(device))?;
    let control_reads = report.step("control queue", wdf::create_manual_queue(device))?;
    let waits = report.step("wait queue", wdf::create_manual_queue(device))?;
    // Without the timer, read timeouts are only checked when data arrives.
    let timer = report
        .step("timer", wdf::create_periodic_timer(device, evt_timer))
        .ok();

    let port = Arc::new(Port {
        com_reads,
        control_reads,
        waits,
        timer,
        devicemap,
        inner: Mutex::new(Inner {
            state: PortState::default(),
            files: HashMap::new(),
            read_started: HashMap::new(),
        }),
    });
    ports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(Handle::from(device), port);
    Ok(())
}

extern "C" fn evt_device_cleanup(object: WDFOBJECT) {
    let device = object.cast();
    let port = ports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&Handle::from(device));
    if let Some((pdo, _)) = port.as_ref().and_then(|p| p.devicemap.as_ref()) {
        let _ = wdf::devicemap_remove(device, pdo);
    }
}

// ---------------------------------------------------------------------------
// File objects: COM side or control side
// ---------------------------------------------------------------------------

extern "C" fn evt_file_create(device: WDFDEVICE, request: WDFREQUEST, file: WDFFILEOBJECT) {
    let status = match port_of(device) {
        None => STATUS_INVALID_DEVICE_REQUEST,
        Some(port) => {
            let role = if wdf::file_name(file).is_some_and(|n| {
                n.trim_start_matches('\\')
                    .eq_ignore_ascii_case(proto::CONTROL_REFERENCE)
            }) {
                Role::Control
            } else {
                Role::Com
            };
            let mut inner = port.lock();
            if inner.files.values().any(|r| *r == role) {
                // Serial ports are exclusive, and so is the control channel.
                STATUS_ACCESS_DENIED
            } else {
                inner.files.insert(Handle::from(file), role);
                match role {
                    Role::Com => {
                        inner.state.open_com();
                        if let Some(t) = port.timer {
                            wdf::timer_start(t);
                        }
                    }
                    Role::Control => inner.state.open_control(),
                }
                STATUS_SUCCESS
            }
        }
    };
    wdf::complete(request, status, 0);
    if let Some(port) = port_of(device) {
        finish(&port, |inner, out| flush_waits(&port, inner, out));
    }
}

extern "C" fn evt_file_cleanup(file: WDFFILEOBJECT) {
    let device = wdf::file_device(file);
    let Some(port) = port_of(device) else { return };

    let mut out: Vec<Completion> = Vec::new();
    for queue in [port.com_reads, port.control_reads, port.waits] {
        while let Some(req) = wdf::retrieve_for_file(queue, file) {
            out.push(Completion::new(req, STATUS_CANCELLED, 0));
        }
    }
    {
        let mut inner = port.lock();
        match inner.files.remove(&Handle::from(file)) {
            Some(Role::Com) => {
                inner.state.close_com();
                inner.read_started.clear();
                if let Some(t) = port.timer {
                    wdf::timer_stop(t);
                }
            }
            Some(Role::Control) => inner.state.close_control(),
            None => {}
        }
        flush_waits(&port, &mut inner, &mut out);
    }
    wdf::complete_all(out);
}

fn role_of(port: &Port, request: WDFREQUEST) -> Option<Role> {
    let file = wdf::request_file(request);
    port.lock().files.get(&Handle::from(file)).copied()
}

/// Runs `f` under the port lock, then completes the requests it produced.
fn finish(port: &Port, f: impl FnOnce(&mut Inner, &mut Vec<Completion>)) {
    let mut out = Vec::new();
    {
        let mut inner = port.lock();
        f(&mut inner, &mut out);
    }
    wdf::complete_all(out);
}

// ---------------------------------------------------------------------------
// Reads and writes
// ---------------------------------------------------------------------------

extern "C" fn evt_io_read(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    let Some(port) = port_of(wdf::queue_device(queue)) else {
        return wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0);
    };
    if length == 0 {
        return wdf::complete(request, STATUS_SUCCESS, 0);
    }
    match role_of(&port, request) {
        Some(Role::Com) => {
            port.lock()
                .read_started
                .insert(Handle::from(request), Instant::now());
            if let Err(status) = wdf::forward(request, port.com_reads) {
                return wdf::complete(request, status, 0);
            }
            finish(&port, |inner, out| process_com_reads(&port, inner, out));
        }
        Some(Role::Control) => {
            if let Err(status) = wdf::forward(request, port.control_reads) {
                return wdf::complete(request, status, 0);
            }
            finish(&port, |inner, out| process_control_reads(&port, inner, out));
        }
        None => wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0),
    }
}

extern "C" fn evt_io_write(queue: WDFQUEUE, request: WDFREQUEST, length: usize) {
    let Some(port) = port_of(wdf::queue_device(queue)) else {
        return wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0);
    };
    if length == 0 {
        return wdf::complete(request, STATUS_SUCCESS, 0);
    }
    let Some(role) = role_of(&port, request) else {
        return wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0);
    };
    let data = match wdf::input_bytes(request, 1) {
        Ok(d) => d.to_vec(),
        Err(status) => return wdf::complete(request, status, 0),
    };

    let mut accepted = data.len();
    finish(&port, |inner, out| {
        match role {
            Role::Com => {
                inner.state.com_write(&data);
                process_control_reads(&port, inner, out);
            }
            Role::Control => {
                accepted = inner.state.control_write(&data, Instant::now());
                process_com_reads(&port, inner, out);
            }
        }
        flush_waits(&port, inner, out);
    });
    wdf::complete(request, STATUS_SUCCESS, accepted);
}

/// Completes COM-side reads, oldest first, following the timeout rules.
fn process_com_reads(port: &Port, inner: &mut Inner, out: &mut Vec<Completion>) {
    let now = Instant::now();
    while let Some(request) = wdf::retrieve_next(port.com_reads) {
        let wanted = wdf::output_len(request);
        let started = inner
            .read_started
            .get(&Handle::from(request))
            .copied()
            .unwrap_or(now);
        let decision = logic::decide_read(
            &inner.state.timeouts,
            wanted,
            inner.state.to_com.len(),
            started,
            inner.state.last_rx,
            now,
        );
        match decision {
            ReadDecision::Wait => {
                if wdf::requeue(request).is_err() {
                    out.push(Completion::new(request, STATUS_CANCELLED, 0));
                }
                break;
            }
            ReadDecision::Complete { bytes, timed_out } => {
                inner.read_started.remove(&Handle::from(request));
                let n = match wdf::output_bytes(request, 0) {
                    Ok(buf) => {
                        let take = bytes.min(buf.len());
                        inner.state.to_com.pop_into(&mut buf[..take])
                    }
                    Err(status) => {
                        out.push(Completion::new(request, status, 0));
                        continue;
                    }
                };
                inner.state.to_com_total += n as u64;
                let status = if timed_out { STATUS_TIMEOUT } else { STATUS_SUCCESS };
                out.push(Completion::new(request, status, n));
            }
        }
    }
}

/// Hands bytes written by the program to SerialLoom's pending reads.
fn process_control_reads(port: &Port, inner: &mut Inner, out: &mut Vec<Completion>) {
    while !inner.state.from_com.is_empty() {
        let Some(request) = wdf::retrieve_next(port.control_reads) else {
            break;
        };
        match wdf::output_bytes(request, 1) {
            Ok(buf) => {
                let n = inner.state.from_com.pop_into(buf);
                out.push(Completion::new(request, STATUS_SUCCESS, n));
            }
            Err(status) => out.push(Completion::new(request, status, 0)),
        }
    }
}

/// Completes a pending WAIT_ON_MASK when an awaited event has happened.
fn flush_waits(port: &Port, inner: &mut Inner, out: &mut Vec<Completion>) {
    if inner.state.events == 0 {
        return;
    }
    if let Some(request) = wdf::retrieve_next(port.waits) {
        let events = std::mem::take(&mut inner.state.events);
        out.push(wdf::complete_with_u32(request, events));
    }
}

extern "C" fn evt_timer(timer: WDFTIMER) {
    let Some(port) = port_of(wdf::timer_device(timer)) else {
        return;
    };
    finish(&port, |inner, out| process_com_reads(&port, inner, out));
}

// ---------------------------------------------------------------------------
// Device control
// ---------------------------------------------------------------------------

extern "C" fn evt_io_device_control(
    queue: WDFQUEUE,
    request: WDFREQUEST,
    _output_len: usize,
    _input_len: usize,
    code: u32,
) {
    let Some(port) = port_of(wdf::queue_device(queue)) else {
        return wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0);
    };
    match role_of(&port, request) {
        Some(Role::Com) => serial_ioctl(&port, request, code),
        Some(Role::Control) => control_ioctl(&port, request, code),
        None => wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0),
    }
}

fn control_ioctl(port: &Port, request: WDFREQUEST, code: u32) {
    match code {
        proto::IOCTL_SERIALLOOM_GET_STATE => {
            let snapshot = port.lock().state.snapshot();
            // SAFETY: PortState is repr(C) plain data.
            let bytes = unsafe {
                std::slice::from_raw_parts(std::ptr::from_ref(&snapshot).cast::<u8>(), proto::PORT_STATE_SIZE)
            };
            wdf::complete_with_output(request, bytes);
        }
        proto::IOCTL_SERIALLOOM_SET_MODEM_STATUS => match wdf::input_u32(request) {
            Ok(bits) => {
                finish(port, |inner, out| {
                    inner.state.set_modem(|s| {
                        s.modem_override = (bits != proto::MODEM_AUTOMATIC).then_some(bits);
                    });
                    flush_waits(port, inner, out);
                });
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(status) => wdf::complete(request, status, 0),
        },
        _ => wdf::complete(request, STATUS_INVALID_DEVICE_REQUEST, 0),
    }
}

fn serial_ioctl(port: &Port, request: WDFREQUEST, code: u32) {
    use serial::*;

    // Simple getters: copy a value out.
    let get = |bytes: &[u8]| wdf::complete_with_output(request, bytes);

    match code {
        IOCTL_SERIAL_SET_BAUD_RATE => match wdf::input_u32(request) {
            Ok(0) => wdf::complete(request, STATUS_INVALID_PARAMETER, 0),
            Ok(baud) => {
                port.lock().state.baud_rate = baud;
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_BAUD_RATE => {
            let v = port.lock().state.baud_rate;
            get(&v.to_le_bytes());
        }

        IOCTL_SERIAL_SET_LINE_CONTROL => match wdf::input_array::<3>(request) {
            Ok(lc) => {
                port.lock().state.line_control = lc;
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_LINE_CONTROL => {
            let v = port.lock().state.line_control;
            get(&v);
        }

        IOCTL_SERIAL_SET_CHARS => match wdf::input_array::<6>(request) {
            Ok(c) => {
                port.lock().state.chars = c;
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_CHARS => {
            let v = port.lock().state.chars;
            get(&v);
        }

        IOCTL_SERIAL_SET_HANDFLOW => match wdf::input_array::<16>(request) {
            Ok(h) => {
                port.lock().state.handflow = h;
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_HANDFLOW => {
            let v = port.lock().state.handflow;
            get(&v);
        }

        IOCTL_SERIAL_SET_TIMEOUTS => match wdf::input_array::<20>(request) {
            Ok(raw) => {
                let t = logic::Timeouts::from_bytes(&raw);
                if t.is_valid() {
                    port.lock().state.timeouts = t;
                    wdf::complete(request, STATUS_SUCCESS, 0);
                } else {
                    wdf::complete(request, STATUS_INVALID_PARAMETER, 0);
                }
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_TIMEOUTS => {
            let v = port.lock().state.timeouts;
            get(&v.to_bytes());
        }

        IOCTL_SERIAL_SET_DTR | IOCTL_SERIAL_CLR_DTR | IOCTL_SERIAL_SET_RTS | IOCTL_SERIAL_CLR_RTS => {
            {
                let mut inner = port.lock();
                match code {
                    IOCTL_SERIAL_SET_DTR => inner.state.dtr = true,
                    IOCTL_SERIAL_CLR_DTR => inner.state.dtr = false,
                    IOCTL_SERIAL_SET_RTS => inner.state.rts = true,
                    _ => inner.state.rts = false,
                }
            }
            wdf::complete(request, STATUS_SUCCESS, 0);
        }
        IOCTL_SERIAL_GET_DTRRTS | IOCTL_SERIAL_GET_MODEM_CONTROL => {
            let v = {
                let s = &port.lock().state;
                u32::from(s.dtr) | (u32::from(s.rts) << 1)
            };
            get(&v.to_le_bytes());
        }
        IOCTL_SERIAL_SET_MODEM_CONTROL => match wdf::input_u32(request) {
            Ok(mcr) => {
                let mut inner = port.lock();
                inner.state.dtr = mcr & 1 != 0;
                inner.state.rts = mcr & 2 != 0;
                drop(inner);
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_MODEMSTATUS => {
            let v = port.lock().state.modem_status();
            get(&v.to_le_bytes());
        }

        IOCTL_SERIAL_GET_COMMSTATUS => {
            let queued = port.lock().state.to_com.len() as u32;
            get(&comm_status(queued));
        }
        IOCTL_SERIAL_GET_PROPERTIES => get(&comm_properties(logic::BUFFER_SIZE as u32)),
        IOCTL_SERIAL_GET_STATS => {
            let v = {
                let s = &port.lock().state;
                perf_stats(s.to_com_total, s.from_com_total)
            };
            get(&v);
        }
        IOCTL_SERIAL_CONFIG_SIZE => get(&0u32.to_le_bytes()),

        IOCTL_SERIAL_SET_WAIT_MASK => match wdf::input_u32(request) {
            Ok(mask) => {
                let mut out = Vec::new();
                {
                    let mut inner = port.lock();
                    inner.state.wait_mask = mask;
                    inner.state.events = 0;
                    // A new mask ends any pending wait with "no events".
                    if let Some(w) = wdf::retrieve_next(port.waits) {
                        out.push(wdf::complete_with_u32(w, 0));
                    }
                }
                wdf::complete_all(out);
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },
        IOCTL_SERIAL_GET_WAIT_MASK => {
            let v = port.lock().state.wait_mask;
            get(&v.to_le_bytes());
        }
        IOCTL_SERIAL_WAIT_ON_MASK => {
            if wdf::output_len(request) < 4 {
                return wdf::complete(request, STATUS_BUFFER_TOO_SMALL, 0);
            }
            let mut inner = port.lock();
            if inner.state.wait_mask == 0 || wdf::has_pending(port.waits) {
                drop(inner);
                return wdf::complete(request, STATUS_INVALID_PARAMETER, 0);
            }
            if inner.state.events != 0 {
                let events = std::mem::take(&mut inner.state.events);
                drop(inner);
                wdf::complete_all(vec![wdf::complete_with_u32(request, events)]);
            } else if let Err(s) = wdf::forward(request, port.waits) {
                drop(inner);
                wdf::complete(request, s, 0);
            }
        }

        IOCTL_SERIAL_PURGE => match wdf::input_u32(request) {
            Ok(flags) => {
                let mut out = Vec::new();
                {
                    let mut inner = port.lock();
                    inner.state.purge(flags);
                    if flags & logic::PURGE_RXABORT != 0 {
                        while let Some(r) = wdf::retrieve_next(port.com_reads) {
                            inner.read_started.remove(&Handle::from(r));
                            out.push(Completion::new(r, STATUS_CANCELLED, 0));
                        }
                    }
                }
                wdf::complete_all(out);
                wdf::complete(request, STATUS_SUCCESS, 0);
            }
            Err(s) => wdf::complete(request, s, 0),
        },

        // Accepted and ignored: there is no hardware behind the port.
        IOCTL_SERIAL_SET_QUEUE_SIZE
        | IOCTL_SERIAL_SET_BREAK_ON
        | IOCTL_SERIAL_SET_BREAK_OFF
        | IOCTL_SERIAL_IMMEDIATE_CHAR
        | IOCTL_SERIAL_SET_XOFF
        | IOCTL_SERIAL_SET_XON
        | IOCTL_SERIAL_RESET_DEVICE
        | IOCTL_SERIAL_XOFF_COUNTER
        | IOCTL_SERIAL_LSRMST_INSERT
        | IOCTL_SERIAL_CLEAR_STATS
        | IOCTL_SERIAL_SET_FIFO_CONTROL
        | IOCTL_SERIAL_APPLY_DEFAULT_CONFIGURATION => wdf::complete(request, STATUS_SUCCESS, 0),

        _ => wdf::complete(request, STATUS_NOT_SUPPORTED, 0),
    }
}
