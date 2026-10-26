//! Small safe-ish wrappers over the WDF calls the driver needs.
//!
//! Every `unsafe` block in the driver lives here. Each wrapper takes handles
//! that WDF handed to the driver and that are valid for the duration of the
//! callback they are used in.

use wdk_sys::{
    _POOL_TYPE, _WDF_EXECUTION_LEVEL, _WDF_FILEOBJECT_CLASS, _WDF_IO_QUEUE_DISPATCH_TYPE,
    _WDF_SYNCHRONIZATION_SCOPE, _WDF_TRI_STATE, DEVICE_REGISTRY_PROPERTY, GUID, NTSTATUS, PCUNICODE_STRING,
    PDRIVER_OBJECT, PFN_WDF_DEVICE_FILE_CREATE, PFN_WDF_DRIVER_DEVICE_ADD, PFN_WDF_FILE_CLEANUP,
    PFN_WDF_IO_QUEUE_IO_DEVICE_CONTROL, PFN_WDF_IO_QUEUE_IO_READ, PFN_WDF_IO_QUEUE_IO_WRITE,
    PFN_WDF_OBJECT_CONTEXT_CLEANUP, PFN_WDF_TIMER, PWDFDEVICE_INIT, STATUS_BUFFER_TOO_SMALL, STATUS_SUCCESS,
    UNICODE_STRING, WDF_DRIVER_CONFIG, WDF_FILEOBJECT_CONFIG, WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE,
    WDF_NO_OBJECT_ATTRIBUTES, WDF_OBJECT_ATTRIBUTES, WDF_TIMER_CONFIG, WDFDEVICE, WDFFILEOBJECT, WDFKEY,
    WDFMEMORY, WDFOBJECT, WDFQUEUE, WDFREQUEST, WDFTIMER, call_unsafe_wdf_function_binding,
};

/// A WDF handle stored as an integer so it can live in `Send` collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(usize);

impl<T> From<*mut T> for Handle {
    fn from(p: *mut T) -> Self {
        Self(p as usize)
    }
}

impl Handle {
    fn as_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }
}

const PLUGPLAY_REGKEY_DEVICE: u32 = 1;
const KEY_QUERY_VALUE: u32 = 0x0001;
const KEY_SET_VALUE: u32 = 0x0002;
/// `WDF_REGKEY_DEVICE_SUBKEY` in UMDF: a writable subkey of the hardware key.
const WDF_REGKEY_DEVICE_SUBKEY: u32 = 256;
const STATUS_NO_MORE_ENTRIES_CODE: NTSTATUS = 0x8000_001Au32 as NTSTATUS;

fn nt_success(s: NTSTATUS) -> bool {
    s >= 0
}

fn check(s: NTSTATUS) -> Result<(), NTSTATUS> {
    if nt_success(s) { Ok(()) } else { Err(s) }
}

fn size_of_u32<T>() -> u32 {
    u32::try_from(core::mem::size_of::<T>()).expect("WDF structs are small")
}

/// `WDF_OBJECT_ATTRIBUTES_INIT`.
fn attributes() -> WDF_OBJECT_ATTRIBUTES {
    WDF_OBJECT_ATTRIBUTES {
        Size: size_of_u32::<WDF_OBJECT_ATTRIBUTES>(),
        ExecutionLevel: _WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope: _WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..WDF_OBJECT_ATTRIBUTES::default()
    }
}

/// UTF-16 string with a `UNICODE_STRING` view onto it.
struct WideString {
    buf: Vec<u16>,
}

impl WideString {
    fn new(s: &str) -> Self {
        Self {
            buf: s.encode_utf16().collect(),
        }
    }

    fn with_capacity(chars: usize) -> Self {
        Self { buf: vec![0; chars] }
    }

    fn unicode(&mut self) -> UNICODE_STRING {
        let bytes = u16::try_from(self.buf.len() * 2).unwrap_or(u16::MAX);
        UNICODE_STRING {
            Length: bytes,
            MaximumLength: bytes,
            Buffer: self.buf.as_mut_ptr(),
        }
    }
}

fn from_unicode(u: &UNICODE_STRING) -> String {
    if u.Buffer.is_null() || u.Length == 0 {
        return String::new();
    }
    // SAFETY: Buffer points to Length bytes of UTF-16 owned by WDF or by us.
    let chars = unsafe { core::slice::from_raw_parts(u.Buffer, usize::from(u.Length) / 2) };
    String::from_utf16_lossy(chars)
}

fn to_guid(g: &serialloom_proto::Guid) -> GUID {
    GUID {
        Data1: g.data1,
        Data2: g.data2,
        Data3: g.data3,
        Data4: g.data4,
    }
}

// ---------------------------------------------------------------------------
// Driver and device
// ---------------------------------------------------------------------------

/// # Safety
/// Arguments must be the ones passed to `DriverEntry`.
pub unsafe fn create_driver(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
    device_add: PFN_WDF_DRIVER_DEVICE_ADD,
) -> NTSTATUS {
    let mut config = WDF_DRIVER_CONFIG {
        Size: size_of_u32::<WDF_DRIVER_CONFIG>(),
        EvtDriverDeviceAdd: device_add,
        ..WDF_DRIVER_CONFIG::default()
    };
    // SAFETY: valid pointers from DriverEntry; attributes and output may be null.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut config,
            WDF_NO_HANDLE.cast(),
        )
    }
}

/// # Safety
/// `init` must be the device init pointer from EvtDriverDeviceAdd.
pub unsafe fn set_file_callbacks(
    init: PWDFDEVICE_INIT,
    create: unsafe extern "C" fn(WDFDEVICE, WDFREQUEST, WDFFILEOBJECT),
    cleanup: unsafe extern "C" fn(WDFFILEOBJECT),
) {
    let create: PFN_WDF_DEVICE_FILE_CREATE = Some(create);
    let cleanup: PFN_WDF_FILE_CLEANUP = Some(cleanup);
    let mut config = WDF_FILEOBJECT_CONFIG {
        Size: size_of_u32::<WDF_FILEOBJECT_CONFIG>(),
        EvtDeviceFileCreate: create,
        EvtFileCleanup: cleanup,
        AutoForwardCleanupClose: _WDF_TRI_STATE::WdfUseDefault,
        FileObjectClass: _WDF_FILEOBJECT_CLASS::WdfFileObjectWdfCannotUseFsContexts,
        ..WDF_FILEOBJECT_CONFIG::default()
    };
    // SAFETY: init is valid; config lives across the call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetFileObjectConfig,
            init,
            &raw mut config,
            WDF_NO_OBJECT_ATTRIBUTES,
        );
    }
}

/// # Safety
/// `init` must be the device init pointer from EvtDriverDeviceAdd.
pub unsafe fn create_device(
    init: PWDFDEVICE_INIT,
    cleanup: unsafe extern "C" fn(WDFOBJECT),
) -> Result<WDFDEVICE, NTSTATUS> {
    let cleanup: PFN_WDF_OBJECT_CONTEXT_CLEANUP = Some(cleanup);
    let mut attrs = WDF_OBJECT_ATTRIBUTES {
        EvtCleanupCallback: cleanup,
        ..attributes()
    };
    let mut init = init;
    let mut device: WDFDEVICE = core::ptr::null_mut();
    // SAFETY: init is valid; WdfDeviceCreate consumes it.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfDeviceCreate, &raw mut init, &raw mut attrs, &raw mut device)
    };
    check(status).map(|()| device)
}

pub fn create_interface(
    device: WDFDEVICE,
    guid: &serialloom_proto::Guid,
    reference: Option<&str>,
) -> Result<(), NTSTATUS> {
    let guid = to_guid(guid);
    let mut wide = reference.map(WideString::new);
    let reference_string = wide.as_mut().map(WideString::unicode);
    let reference_ptr: PCUNICODE_STRING = reference_string
        .as_ref()
        .map_or(core::ptr::null(), core::ptr::from_ref);
    // SAFETY: device is live; guid and string outlive the call.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreateDeviceInterface,
            device,
            &raw const guid,
            reference_ptr
        )
    })
}

pub fn create_symbolic_link(device: WDFDEVICE, link: &str) -> Result<(), NTSTATUS> {
    let mut wide = WideString::new(link);
    let u = wide.unicode();
    // SAFETY: device is live; the string outlives the call.
    check(unsafe { call_unsafe_wdf_function_binding!(WdfDeviceCreateSymbolicLink, device, &raw const u) })
}

/// Reads `PortName` from the device's hardware key.
pub fn read_port_name(device: WDFDEVICE) -> Result<String, NTSTATUS> {
    let mut key: WDFKEY = core::ptr::null_mut();
    // SAFETY: device is live; key receives a handle we close below.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceOpenRegistryKey,
            device,
            PLUGPLAY_REGKEY_DEVICE,
            KEY_QUERY_VALUE,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut key,
        )
    })?;
    let mut name = WideString::new("PortName");
    let value_name = name.unicode();
    let mut value_buf = WideString::with_capacity(64);
    let mut value = value_buf.unicode();
    // SAFETY: key is open; value points to our 64-char buffer.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRegistryQueryUnicodeString,
            key,
            &raw const value_name,
            core::ptr::null_mut(),
            &raw mut value,
        )
    };
    // SAFETY: key was opened above.
    unsafe { call_unsafe_wdf_function_binding!(WdfRegistryClose, key) };
    check(status)?;
    let name = from_unicode(&value).trim_end_matches('\0').trim().to_string();
    if name.is_empty() {
        Err(STATUS_BUFFER_TOO_SMALL)
    } else {
        Ok(name)
    }
}

/// The PDO name, e.g. `\Device\00000071`, used as the SERIALCOMM value name.
pub fn pdo_name(device: WDFDEVICE) -> Result<String, NTSTATUS> {
    let mut attrs = WDF_OBJECT_ATTRIBUTES {
        ParentObject: device.cast(),
        ..attributes()
    };
    let mut memory: WDFMEMORY = core::ptr::null_mut();
    // SAFETY: device is live; memory is parented to the device.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceAllocAndQueryProperty,
            device,
            DEVICE_REGISTRY_PROPERTY::DevicePropertyPhysicalDeviceObjectName,
            _POOL_TYPE::NonPagedPoolNx,
            &raw mut attrs,
            &raw mut memory,
        )
    })?;
    let mut size: usize = 0;
    // SAFETY: memory was just allocated by WDF.
    let ptr = unsafe { call_unsafe_wdf_function_binding!(WdfMemoryGetBuffer, memory, &raw mut size) };
    // SAFETY: the buffer holds `size` bytes of NUL-terminated UTF-16.
    let chars = unsafe { core::slice::from_raw_parts(ptr.cast::<u16>(), size / 2) };
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    Ok(String::from_utf16_lossy(&chars[..end]))
}

fn with_devicemap(device: WDFDEVICE, f: impl FnOnce(WDFKEY) -> NTSTATUS) -> Result<(), NTSTATUS> {
    let mut sub = WideString::new("SERIALCOMM");
    let subkey = sub.unicode();
    let mut key: WDFKEY = core::ptr::null_mut();
    // SAFETY: device is live; key is closed below.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceOpenDevicemapKey,
            device,
            &raw const subkey,
            KEY_SET_VALUE,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut key,
        )
    })?;
    let status = f(key);
    // SAFETY: key was opened above.
    unsafe { call_unsafe_wdf_function_binding!(WdfRegistryClose, key) };
    check(status)
}

/// Adds `pdo = COMx` under HARDWARE\DEVICEMAP\SERIALCOMM so programs list the port.
pub fn devicemap_set(device: WDFDEVICE, pdo: &str, com: &str) -> Result<(), NTSTATUS> {
    with_devicemap(device, |key| {
        let mut n = WideString::new(pdo);
        let mut v = WideString::new(com);
        let (name, value) = (n.unicode(), v.unicode());
        // SAFETY: key is open; strings outlive the call.
        unsafe {
            call_unsafe_wdf_function_binding!(
                WdfRegistryAssignUnicodeString,
                key,
                &raw const name,
                &raw const value
            )
        }
    })
}

pub fn devicemap_remove(device: WDFDEVICE, pdo: &str) -> Result<(), NTSTATUS> {
    with_devicemap(device, |key| {
        let mut n = WideString::new(pdo);
        let name = n.unicode();
        // SAFETY: key is open; string outlives the call.
        unsafe { call_unsafe_wdf_function_binding!(WdfRegistryRemoveValue, key, &raw const name) }
    })
}

/// Stores `SerialLoomStartup = text` in the device's writable registry subkey.
pub fn write_startup_report(device: WDFDEVICE, text: &str) -> Result<(), NTSTATUS> {
    let mut key: WDFKEY = core::ptr::null_mut();
    // SAFETY: device is live; key is closed below.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceOpenRegistryKey,
            device,
            PLUGPLAY_REGKEY_DEVICE | WDF_REGKEY_DEVICE_SUBKEY,
            KEY_SET_VALUE,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut key,
        )
    })?;
    let mut n = WideString::new("SerialLoomStartup");
    let mut v = WideString::new(text);
    let (name, value) = (n.unicode(), v.unicode());
    // SAFETY: key is open; strings outlive the call.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRegistryAssignUnicodeString,
            key,
            &raw const name,
            &raw const value
        )
    };
    // SAFETY: key was opened above.
    unsafe { call_unsafe_wdf_function_binding!(WdfRegistryClose, key) };
    check(status)
}

// ---------------------------------------------------------------------------
// Queues and timer
// ---------------------------------------------------------------------------

pub fn create_default_queue(
    device: WDFDEVICE,
    read: unsafe extern "C" fn(WDFQUEUE, WDFREQUEST, usize),
    write: unsafe extern "C" fn(WDFQUEUE, WDFREQUEST, usize),
    ioctl: unsafe extern "C" fn(WDFQUEUE, WDFREQUEST, usize, usize, u32),
) -> Result<(), NTSTATUS> {
    let read: PFN_WDF_IO_QUEUE_IO_READ = Some(read);
    let write: PFN_WDF_IO_QUEUE_IO_WRITE = Some(write);
    let ioctl: PFN_WDF_IO_QUEUE_IO_DEVICE_CONTROL = Some(ioctl);
    let mut config = WDF_IO_QUEUE_CONFIG {
        Size: size_of_u32::<WDF_IO_QUEUE_CONFIG>(),
        PowerManaged: _WDF_TRI_STATE::WdfUseDefault,
        DefaultQueue: 1,
        DispatchType: _WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchSequential,
        EvtIoRead: read,
        EvtIoWrite: write,
        EvtIoDeviceControl: ioctl,
        ..WDF_IO_QUEUE_CONFIG::default()
    };
    let mut queue: WDFQUEUE = core::ptr::null_mut();
    // SAFETY: device is live; config lives across the call.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &raw mut config,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut queue,
        )
    })
}

/// A queue the driver pulls requests from itself. Requests parked here are
/// cancelled by the framework when their handle closes.
pub fn create_manual_queue(device: WDFDEVICE) -> Result<Handle, NTSTATUS> {
    let mut config = WDF_IO_QUEUE_CONFIG {
        Size: size_of_u32::<WDF_IO_QUEUE_CONFIG>(),
        PowerManaged: _WDF_TRI_STATE::WdfUseDefault,
        DispatchType: _WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchManual,
        ..WDF_IO_QUEUE_CONFIG::default()
    };
    let mut queue: WDFQUEUE = core::ptr::null_mut();
    // SAFETY: device is live; config lives across the call.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &raw mut config,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut queue,
        )
    })?;
    Ok(Handle::from(queue))
}

/// 10 ms periodic timer that drives read timeouts while a program has the port open.
pub fn create_periodic_timer(
    device: WDFDEVICE,
    callback: unsafe extern "C" fn(WDFTIMER),
) -> Result<Handle, NTSTATUS> {
    let callback: PFN_WDF_TIMER = Some(callback);
    let mut config = WDF_TIMER_CONFIG {
        Size: size_of_u32::<WDF_TIMER_CONFIG>(),
        EvtTimerFunc: callback,
        Period: 10,
        AutomaticSerialization: 0,
        TolerableDelay: 0,
        ..WDF_TIMER_CONFIG::default()
    };
    let mut attrs = WDF_OBJECT_ATTRIBUTES {
        ParentObject: device.cast(),
        ..attributes()
    };
    let mut timer: WDFTIMER = core::ptr::null_mut();
    // SAFETY: device is live; the timer is parented to it.
    check(unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerCreate, &raw mut config, &raw mut attrs, &raw mut timer)
    })?;
    Ok(Handle::from(timer))
}

pub fn timer_start(timer: Handle) {
    // Relative due time of 10 ms, in 100 ns units.
    // SAFETY: timer is a live child of the device.
    let _ = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStart, timer.as_ptr::<_>(), -100_000_i64) };
}

pub fn timer_stop(timer: Handle) {
    // SAFETY: timer is live. Do not wait: we may be called from a WDF callback.
    let _ = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer.as_ptr::<_>(), 0) };
}

pub fn timer_device(timer: WDFTIMER) -> WDFDEVICE {
    // SAFETY: timer is live; its parent is the device.
    unsafe { call_unsafe_wdf_function_binding!(WdfTimerGetParentObject, timer) }.cast()
}

pub fn queue_device(queue: WDFQUEUE) -> WDFDEVICE {
    // SAFETY: queue is live inside its callback.
    unsafe { call_unsafe_wdf_function_binding!(WdfIoQueueGetDevice, queue) }
}

pub fn file_device(file: WDFFILEOBJECT) -> WDFDEVICE {
    // SAFETY: file object is live inside its callback.
    unsafe { call_unsafe_wdf_function_binding!(WdfFileObjectGetDevice, file) }
}

/// Name the file was opened with after the device path, e.g. `\control`.
pub fn file_name(file: WDFFILEOBJECT) -> Option<String> {
    // SAFETY: file object is live inside its callback.
    let name = unsafe { call_unsafe_wdf_function_binding!(WdfFileObjectGetFileName, file) };
    if name.is_null() {
        return None;
    }
    // SAFETY: WDF returns a valid UNICODE_STRING for the file object's lifetime.
    let s = from_unicode(unsafe { &*name });
    (!s.is_empty()).then_some(s)
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// A request to complete once the port lock is released.
pub struct Completion {
    request: Handle,
    status: NTSTATUS,
    information: usize,
}

impl Completion {
    pub fn new(request: WDFREQUEST, status: NTSTATUS, information: usize) -> Self {
        Self {
            request: Handle::from(request),
            status,
            information,
        }
    }
}

pub fn complete(request: WDFREQUEST, status: NTSTATUS, information: usize) {
    // SAFETY: the driver owns `request` and completes it exactly once.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestCompleteWithInformation,
            request,
            status,
            information as u64,
        );
    }
}

pub fn complete_all(list: Vec<Completion>) {
    for c in list {
        complete(c.request.as_ptr(), c.status, c.information);
    }
}

/// Writes `value` into the output buffer and prepares the completion.
pub fn complete_with_u32(request: WDFREQUEST, value: u32) -> Completion {
    match output_bytes(request, 4) {
        Ok(buf) => {
            buf[..4].copy_from_slice(&value.to_le_bytes());
            Completion::new(request, STATUS_SUCCESS, 4)
        }
        Err(status) => Completion::new(request, status, 0),
    }
}

/// Copies `bytes` to the output buffer and completes the request.
pub fn complete_with_output(request: WDFREQUEST, bytes: &[u8]) {
    match output_bytes(request, bytes.len()) {
        Ok(buf) => {
            buf[..bytes.len()].copy_from_slice(bytes);
            complete(request, STATUS_SUCCESS, bytes.len());
        }
        Err(status) => complete(request, status, 0),
    }
}

pub fn request_file(request: WDFREQUEST) -> WDFFILEOBJECT {
    // SAFETY: request is owned by the driver.
    unsafe { call_unsafe_wdf_function_binding!(WdfRequestGetFileObject, request) }
}

pub fn input_bytes<'a>(request: WDFREQUEST, min: usize) -> Result<&'a [u8], NTSTATUS> {
    let mut ptr: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut len: usize = 0;
    // SAFETY: request is owned by the driver; WDF fills ptr/len.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveInputBuffer,
            request,
            min,
            &raw mut ptr,
            &raw mut len
        )
    })?;
    // SAFETY: WDF guarantees `len` readable bytes until the request completes.
    Ok(unsafe { core::slice::from_raw_parts(ptr.cast::<u8>(), len) })
}

pub fn output_bytes<'a>(request: WDFREQUEST, min: usize) -> Result<&'a mut [u8], NTSTATUS> {
    let mut ptr: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut len: usize = 0;
    // SAFETY: request is owned by the driver; WDF fills ptr/len.
    check(unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveOutputBuffer,
            request,
            min,
            &raw mut ptr,
            &raw mut len
        )
    })?;
    // SAFETY: WDF guarantees `len` writable bytes until the request completes.
    Ok(unsafe { core::slice::from_raw_parts_mut(ptr.cast::<u8>(), len) })
}

pub fn output_len(request: WDFREQUEST) -> usize {
    output_bytes(request, 0).map_or(0, |b| b.len())
}

pub fn input_u32(request: WDFREQUEST) -> Result<u32, NTSTATUS> {
    input_array::<4>(request).map(u32::from_le_bytes)
}

pub fn input_array<const N: usize>(request: WDFREQUEST) -> Result<[u8; N], NTSTATUS> {
    let bytes = input_bytes(request, N)?;
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[..N]);
    Ok(out)
}

pub fn forward(request: WDFREQUEST, queue: Handle) -> Result<(), NTSTATUS> {
    // SAFETY: request is owned by the driver; queue is a live manual queue.
    check(unsafe {
        call_unsafe_wdf_function_binding!(WdfRequestForwardToIoQueue, request, queue.as_ptr::<_>())
    })
}

pub fn requeue(request: WDFREQUEST) -> Result<(), NTSTATUS> {
    // SAFETY: request came from a manual queue and is owned by the driver.
    check(unsafe { call_unsafe_wdf_function_binding!(WdfRequestRequeue, request) })
}

pub fn retrieve_next(queue: Handle) -> Option<WDFREQUEST> {
    let mut request: WDFREQUEST = core::ptr::null_mut();
    // SAFETY: queue is a live manual queue.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueRetrieveNextRequest,
            queue.as_ptr::<_>(),
            &raw mut request
        )
    };
    (nt_success(status) && status != STATUS_NO_MORE_ENTRIES_CODE && !request.is_null()).then_some(request)
}

pub fn retrieve_for_file(queue: Handle, file: WDFFILEOBJECT) -> Option<WDFREQUEST> {
    let mut request: WDFREQUEST = core::ptr::null_mut();
    // SAFETY: queue is live; file is the object being cleaned up.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueRetrieveRequestByFileObject,
            queue.as_ptr::<_>(),
            file,
            &raw mut request,
        )
    };
    (nt_success(status) && !request.is_null()).then_some(request)
}

/// True when the queue holds at least one request.
pub fn has_pending(queue: Handle) -> bool {
    let mut queued: u32 = 0;
    let mut driver_owned: u32 = 0;
    // SAFETY: queue is live.
    let _ = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueGetState,
            queue.as_ptr::<_>(),
            &raw mut queued,
            &raw mut driver_owned,
        )
    };
    queued > 0
}
