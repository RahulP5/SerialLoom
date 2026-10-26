//! USB/IP server exporting virtual CDC-ACM devices on 127.0.0.1.
//!
//! A USB/IP client (the usbip-win2 virtual host controller on Windows)
//! "imports" a device by bus ID. From then on the TCP connection carries USB
//! requests (URBs). Bulk OUT data from the host is what the program wrote to
//! the COM port; bulk IN requests stay pending until SerialLoom has data for
//! the program.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use super::cdc::{self, CdcState, ControlReply};
use super::protocol::{self as p, Command, DeviceInfo, OpRequest, Submit};
use crate::driver::{ControlChannel, ControlState};

/// Bytes buffered in each direction before data is dropped.
const BUFFER_LIMIT: usize = 256 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

struct Pending {
    seqnum: u32,
    length: u32,
}

struct DevState {
    cdc: CdcState,
    /// Data for the program (bulk IN).
    to_host: VecDeque<u8>,
    /// Data the program wrote (bulk OUT).
    from_host: VecDeque<u8>,
    pending_in: VecDeque<Pending>,
    pending_notify: VecDeque<Pending>,
    /// Last SERIAL_STATE sent; `None` before the first one.
    notified: Option<u16>,
    /// Writer half of the client connection while imported.
    conn: Option<TcpStream>,
    /// SerialLoom has the control side open.
    control_open: bool,
}

/// One exported virtual serial device.
pub struct VirtualDevice {
    pub info: DeviceInfo,
    state: Mutex<DevState>,
    data_ready: Condvar,
}

impl VirtualDevice {
    fn new(info: DeviceInfo, serial: &str) -> Self {
        Self {
            info,
            state: Mutex::new(DevState {
                cdc: CdcState::new(serial),
                to_host: VecDeque::new(),
                from_host: VecDeque::new(),
                pending_in: VecDeque::new(),
                pending_notify: VecDeque::new(),
                notified: None,
                conn: None,
                control_open: false,
            }),
            data_ready: Condvar::new(),
        }
    }

    pub fn busid(&self) -> &str {
        &self.info.busid
    }

    /// Imported by a USB/IP client (i.e. plugged into Windows).
    pub fn is_attached(&self) -> bool {
        self.state.lock().conn.is_some()
    }

    /// A program has the COM port open (DTR raised).
    pub fn is_port_open(&self) -> bool {
        self.state.lock().cdc.port_open()
    }

    /// Closes the client connection, which unplugs the device.
    pub fn disconnect(&self) {
        if let Some(conn) = self.state.lock().conn.take() {
            let _ = conn.shutdown(Shutdown::Both);
        }
    }

    /// Opens the SerialLoom side. Only one at a time.
    pub fn open_control(self: &Arc<Self>) -> io::Result<DeviceChannel> {
        let mut st = self.state.lock();
        if st.control_open {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the port is already attached to another SerialLoom hub",
            ));
        }
        st.control_open = true;
        st.from_host.clear();
        flush_notify(&mut st);
        Ok(DeviceChannel(Arc::clone(self)))
    }

    fn attach(&self, conn: TcpStream) {
        let mut st = self.state.lock();
        st.conn = Some(conn);
        st.notified = None;
        st.to_host.clear();
        st.from_host.clear();
    }

    fn detach(&self) {
        let mut st = self.state.lock();
        st.conn = None;
        st.pending_in.clear();
        st.pending_notify.clear();
        st.cdc.dtr = false;
        st.cdc.rts = false;
        st.cdc.configured = false;
    }

    fn on_submit(&self, s: Submit) {
        let mut st = self.state.lock();
        match s.ep {
            0 => {
                let reply = st.cdc.control(&s.setup, &s.data);
                let msg = match reply {
                    ControlReply::Data(d) if s.direction == p::DIR_IN => {
                        p::encode_ret_submit(s.seqnum, 0, &d, d.len())
                    }
                    ControlReply::Data(_) => p::encode_ret_submit(s.seqnum, 0, &[], s.data.len()),
                    ControlReply::Stall => p::encode_ret_submit(s.seqnum, p::EPIPE, &[], 0),
                };
                send(&mut st, &msg);
                flush_notify(&mut st);
            }
            cdc::EP_DATA_OUT if s.direction == p::DIR_OUT => {
                let n = s.data.len();
                if st.control_open {
                    st.from_host.extend(&s.data);
                    let excess = st.from_host.len().saturating_sub(BUFFER_LIMIT);
                    st.from_host.drain(..excess);
                    self.data_ready.notify_all();
                }
                send(&mut st, &p::encode_ret_submit(s.seqnum, 0, &[], n));
            }
            cdc::EP_DATA_IN if s.direction == p::DIR_IN => {
                st.pending_in.push_back(Pending {
                    seqnum: s.seqnum,
                    length: s.length,
                });
                flush_in(&mut st);
            }
            cdc::EP_NOTIFY if s.direction == p::DIR_IN => {
                st.pending_notify.push_back(Pending {
                    seqnum: s.seqnum,
                    length: s.length,
                });
                flush_notify(&mut st);
            }
            _ => send(&mut st, &p::encode_ret_submit(s.seqnum, p::EPIPE, &[], 0)),
        }
    }

    fn on_unlink(&self, seqnum: u32, victim: u32) {
        let mut st = self.state.lock();
        let before = st.pending_in.len() + st.pending_notify.len();
        st.pending_in.retain(|q| q.seqnum != victim);
        st.pending_notify.retain(|q| q.seqnum != victim);
        let removed = before != st.pending_in.len() + st.pending_notify.len();
        let status = if removed { p::ECONNRESET } else { 0 };
        send(&mut st, &p::encode_ret_unlink(seqnum, status));
    }
}

fn send(st: &mut DevState, bytes: &[u8]) {
    let failed = match st.conn.as_mut() {
        Some(conn) => p::write_all(conn, bytes).is_err(),
        None => false,
    };
    if failed && let Some(conn) = st.conn.take() {
        let _ = conn.shutdown(Shutdown::Both);
    }
}

/// Completes pending bulk IN requests with queued data.
fn flush_in(st: &mut DevState) {
    while !st.to_host.is_empty() {
        let Some(req) = st.pending_in.pop_front() else {
            break;
        };
        let n = (req.length as usize).min(st.to_host.len());
        let chunk: Vec<u8> = st.to_host.drain(..n).collect();
        let msg = p::encode_ret_submit(req.seqnum, 0, &chunk, n);
        send(st, &msg);
    }
}

/// Reports DCD/DSR (SerialLoom attached or not) when it changes.
fn flush_notify(st: &mut DevState) {
    if !st.cdc.configured {
        return;
    }
    let bits: u16 = if st.control_open { 0x3 } else { 0x0 };
    if st.notified == Some(bits) {
        return;
    }
    if let Some(req) = st.pending_notify.pop_front() {
        let mut data = cdc::serial_state_notification(bits & 1 != 0, bits & 2 != 0);
        data.truncate(req.length as usize);
        let msg = p::encode_ret_submit(req.seqnum, 0, &data, data.len());
        send(st, &msg);
        st.notified = Some(bits);
    }
}

/// The SerialLoom side of a virtual USB serial device.
pub struct DeviceChannel(Arc<VirtualDevice>);

impl ControlChannel for DeviceChannel {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let mut st = self.0.state.lock();
        if st.from_host.is_empty() {
            self.0.data_ready.wait_for(&mut st, timeout);
        }
        let n = buf.len().min(st.from_host.len());
        for (slot, b) in buf.iter_mut().zip(st.from_host.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }

    fn write(&self, data: &[u8]) -> io::Result<usize> {
        let mut st = self.0.state.lock();
        if st.conn.is_none() || !st.cdc.port_open() {
            // Nothing listening: accept and drop, like an unplugged cable.
            return Ok(data.len());
        }
        let room = BUFFER_LIMIT.saturating_sub(st.to_host.len());
        let n = room.min(data.len());
        st.to_host.extend(&data[..n]);
        flush_in(&mut st);
        Ok(n)
    }

    fn state(&self) -> Option<ControlState> {
        let st = self.0.state.lock();
        Some(ControlState {
            version: serialloom_proto::PORT_STATE_VERSION,
            com_open: u32::from(st.cdc.port_open()),
            baud_rate: st.cdc.line.baud_rate,
            data_bits: st.cdc.line.data_bits,
            parity: st.cdc.line.parity,
            stop_bits: st.cdc.line.stop_bits,
            _reserved: 0,
            dtr_rts: u32::from(st.cdc.dtr) | (u32::from(st.cdc.rts) << 1),
            to_com_queued: st.to_host.len() as u32,
            from_com_queued: st.from_host.len() as u32,
            from_com_total: 0,
            to_com_total: 0,
        })
    }
}

impl Drop for DeviceChannel {
    fn drop(&mut self) {
        let mut st = self.0.state.lock();
        st.control_open = false;
        st.from_host.clear();
        flush_notify(&mut st);
    }
}

/// Listens on 127.0.0.1 and serves the registered devices.
pub struct UsbipServer {
    port: u16,
    devices: Arc<Mutex<HashMap<String, Arc<VirtualDevice>>>>,
    next_devnum: AtomicU32,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl UsbipServer {
    /// Binds to a free port on the loopback interface (never the network).
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let devices: Arc<Mutex<HashMap<String, Arc<VirtualDevice>>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let devices = Arc::clone(&devices);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("usbip-server".into())
                .spawn(move || {
                    for conn in listener.incoming() {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let Ok(conn) = conn else { continue };
                        let devices = Arc::clone(&devices);
                        let _ = std::thread::Builder::new()
                            .name("usbip-conn".into())
                            .spawn(move || handle_connection(conn, &devices));
                    }
                })?
        };
        tracing::info!("USB/IP server listening on 127.0.0.1:{port}");
        Ok(Self {
            port,
            devices,
            next_devnum: AtomicU32::new(1),
            stop,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Registers a new virtual serial device and returns it.
    pub fn add_device(&self, serial: &str) -> Arc<VirtualDevice> {
        let devnum = self.next_devnum.fetch_add(1, Ordering::Relaxed);
        let busid = format!("1-{devnum}");
        let info = DeviceInfo {
            path: format!("/sys/devices/serialloom/{busid}"),
            busid: busid.clone(),
            busnum: 1,
            devnum,
            speed: p::SPEED_FULL,
            vendor: cdc::VENDOR_ID,
            product: cdc::PRODUCT_ID,
            bcd_device: cdc::BCD_DEVICE,
            class: 0xEF,
            subclass: 0x02,
            protocol: 0x01,
            configuration_value: 1,
            num_configurations: 1,
            interfaces: vec![(0x02, 0x02, 0x01), (0x0A, 0x00, 0x00)],
        };
        let device = Arc::new(VirtualDevice::new(info, serial));
        self.devices.lock().insert(busid, Arc::clone(&device));
        device
    }

    /// Unplugs and forgets a device.
    pub fn remove_device(&self, busid: &str) {
        if let Some(d) = self.devices.lock().remove(busid) {
            d.disconnect();
        }
    }
}

impl Drop for UsbipServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for d in self.devices.lock().values() {
            d.disconnect();
        }
        // Wake the accept loop.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn handle_connection(mut conn: TcpStream, devices: &Mutex<HashMap<String, Arc<VirtualDevice>>>) {
    let _ = conn.set_nodelay(true);
    let _ = conn.set_write_timeout(Some(WRITE_TIMEOUT));
    let request = match p::read_op_request(&mut conn) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("USB/IP handshake failed: {e}");
            return;
        }
    };
    match request {
        OpRequest::DevList => {
            let list: Vec<DeviceInfo> = devices
                .lock()
                .values()
                .filter(|d| !d.is_attached())
                .map(|d| d.info.clone())
                .collect();
            let _ = p::write_all(&mut conn, &p::encode_devlist(&list));
        }
        OpRequest::Import { busid } => {
            let device = devices.lock().get(&busid).cloned();
            let Some(device) = device.filter(|d| !d.is_attached()) else {
                let _ = p::write_all(&mut conn, &p::encode_import(None));
                return;
            };
            if p::write_all(&mut conn, &p::encode_import(Some(&device.info))).is_err() {
                return;
            }
            let Ok(writer) = conn.try_clone() else { return };
            device.attach(writer);
            tracing::info!("virtual device {busid} imported");
            loop {
                match p::read_command(&mut conn) {
                    Ok(Command::Submit(s)) => device.on_submit(s),
                    Ok(Command::Unlink { seqnum, victim }) => device.on_unlink(seqnum, victim),
                    Err(e) => {
                        tracing::info!("virtual device {busid} disconnected: {e}");
                        break;
                    }
                }
            }
            device.detach();
        }
    }
    let _ = conn.flush();
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    /// A tiny USB/IP client playing the role of Windows.
    struct Client(TcpStream);

    impl Client {
        fn import(port: u16, busid: &str) -> Self {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut req = Vec::new();
            p::encode_op_header(&mut req, p::OP_REQ_IMPORT, 0);
            let mut b = [0u8; 32];
            b[..busid.len()].copy_from_slice(busid.as_bytes());
            req.extend_from_slice(&b);
            c.write_all(&req).unwrap();
            let mut rep = [0u8; 8 + 312];
            c.read_exact(&mut rep).unwrap();
            assert_eq!(&rep[4..8], &[0, 0, 0, 0], "import refused");
            Self(c)
        }

        fn submit(&mut self, seq: u32, dir: u32, ep: u32, len: u32, setup: [u8; 8], data: &[u8]) {
            let mut b = Vec::new();
            for v in [p::CMD_SUBMIT, seq, 0x0001_0001, dir, ep, 0, len, 0, 0, 0] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend_from_slice(&setup);
            b.extend_from_slice(data);
            self.0.write_all(&b).unwrap();
        }

        /// Reads the completion of an IN transfer: (seqnum, status, data).
        fn ret(&mut self) -> (u32, i32, Vec<u8>) {
            let mut h = [0u8; 48];
            self.0.read_exact(&mut h).unwrap();
            let seq = u32::from_be_bytes(h[4..8].try_into().unwrap());
            let status = i32::from_be_bytes(h[20..24].try_into().unwrap());
            let len = i32::from_be_bytes(h[24..28].try_into().unwrap()) as usize;
            let mut data = vec![0u8; len];
            self.0.read_exact(&mut data).unwrap();
            (seq, status, data)
        }
    }

    /// Reads the completion of an OUT transfer (no payload follows).
    fn ret_out(c: &mut Client) -> (u32, i32) {
        let mut h = [0u8; 48];
        c.0.read_exact(&mut h).unwrap();
        (
            u32::from_be_bytes(h[4..8].try_into().unwrap()),
            i32::from_be_bytes(h[20..24].try_into().unwrap()),
        )
    }

    #[test]
    fn end_to_end_serial_data() {
        let server = UsbipServer::start().unwrap();
        let dev = server.add_device("SLTEST1");
        let mut host = Client::import(server.port(), dev.busid());
        let channel = dev.open_control().unwrap();

        // Enumerate: device descriptor, then configure.
        host.submit(1, p::DIR_IN, 0, 18, [0x80, 6, 0, 1, 0, 0, 18, 0], &[]);
        let (seq, status, desc) = host.ret();
        assert_eq!((seq, status, desc.len()), (1, 0, 18));
        host.submit(2, p::DIR_OUT, 0, 0, [0x00, 9, 1, 0, 0, 0, 0, 0], &[]);
        assert_eq!(ret_out(&mut host), (2, 0));

        // Program opens the port: DTR + RTS.
        host.submit(3, p::DIR_OUT, 0, 0, [0x21, 0x22, 3, 0, 0, 0, 0, 0], &[]);
        assert_eq!(ret_out(&mut host), (3, 0));
        assert!(dev.is_port_open());

        // Program writes "hi": SerialLoom reads it.
        host.submit(4, p::DIR_OUT, cdc::EP_DATA_OUT, 2, [0; 8], b"hi");
        assert_eq!(ret_out(&mut host), (4, 0));
        let mut buf = [0u8; 8];
        let n = channel.read(&mut buf, Duration::from_secs(2)).unwrap();
        assert_eq!(&buf[..n], b"hi");

        // Program waits for data (pending IN), SerialLoom writes "yo".
        host.submit(5, p::DIR_IN, cdc::EP_DATA_IN, 64, [0; 8], &[]);
        assert_eq!(channel.write(b"yo").unwrap(), 2);
        let (seq, status, data) = host.ret();
        assert_eq!((seq, status), (5, 0));
        assert_eq!(data, b"yo");

        // Cancelling a pending read.
        host.submit(6, p::DIR_IN, cdc::EP_DATA_IN, 64, [0; 8], &[]);
        let mut unlink = Vec::new();
        for v in [p::CMD_UNLINK, 7, 0, 0, 0, 6] {
            unlink.extend_from_slice(&v.to_be_bytes());
        }
        unlink.extend_from_slice(&[0u8; 24]);
        host.0.write_all(&unlink).unwrap();
        assert_eq!(ret_out(&mut host), (7, p::ECONNRESET));

        // Unsupported request stalls.
        host.submit(8, p::DIR_IN, 0, 10, [0x80, 6, 0, 6, 0, 0, 10, 0], &[]);
        assert_eq!(ret_out(&mut host), (8, p::EPIPE));
    }

    #[test]
    fn second_import_is_refused() {
        let server = UsbipServer::start().unwrap();
        let dev = server.add_device("SLTEST2");
        let _first = Client::import(server.port(), dev.busid());
        std::thread::sleep(Duration::from_millis(100));
        let mut c = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut req = Vec::new();
        p::encode_op_header(&mut req, p::OP_REQ_IMPORT, 0);
        let mut b = [0u8; 32];
        b[..dev.busid().len()].copy_from_slice(dev.busid().as_bytes());
        req.extend_from_slice(&b);
        c.write_all(&req).unwrap();
        let mut rep = [0u8; 8];
        c.read_exact(&mut rep).unwrap();
        assert_eq!(&rep[4..8], &1u32.to_be_bytes());
    }

    #[test]
    fn writes_without_program_are_dropped() {
        let server = UsbipServer::start().unwrap();
        let dev = server.add_device("SLTEST3");
        let channel = dev.open_control().unwrap();
        assert_eq!(channel.write(b"lost").unwrap(), 4);
        assert!(dev.open_control().is_err(), "control side is exclusive");
    }
}
