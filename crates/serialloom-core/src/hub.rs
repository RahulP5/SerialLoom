//! User-mode data router.
//!
//! A running hub has one reader thread per endpoint. Each thread owns its
//! connection: it opens (and re-opens) the port or socket, publishes a writer
//! handle, reads incoming bytes and pushes them straight to the writers of
//! the target endpoints. With no central router thread, latency stays low,
//! and a slow target applies back-pressure to its source only.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::driver::{ControlChannel, PortDriver};
use crate::model::{
    EndpointSpec, EndpointStatus, FlowControl, HubKind, HubSpec, MonitorRecord, Parity, SerialSettings,
    StopBits, route_targets,
};

const READ_TIMEOUT: Duration = Duration::from_millis(25);
const RETRY_DELAY: Duration = Duration::from_millis(1500);
const WRITE_DEADLINE: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const MONITOR_MAX_BYTES: usize = 256 * 1024;
const MONITOR_MAX_RECORDS: usize = 4000;
const MONITOR_PAGE: usize = 500;

type Writer = Box<dyn Write + Send>;

#[derive(Default)]
struct Counters {
    rx: AtomicU64,
    tx: AtomicU64,
    dropped: AtomicU64,
    connected: AtomicBool,
    peer_open: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl Counters {
    fn snapshot(&self) -> EndpointStatus {
        EndpointStatus {
            rx_bytes: self.rx.load(Ordering::Relaxed),
            tx_bytes: self.tx.load(Ordering::Relaxed),
            dropped_bytes: self.dropped.load(Ordering::Relaxed),
            connected: self.connected.load(Ordering::Relaxed),
            peer_open: self.peer_open.load(Ordering::Relaxed),
            last_error: self.last_error.lock().clone(),
        }
    }

    fn set_error(&self, message: String) {
        let mut slot = self.last_error.lock();
        if slot.as_deref() != Some(message.as_str()) {
            tracing::warn!("{message}");
            *slot = Some(message);
        }
    }

    fn clear_error(&self) {
        *self.last_error.lock() = None;
    }
}

#[derive(Default)]
struct Monitor {
    records: VecDeque<MonitorRecord>,
    bytes: usize,
    next_seq: u64,
}

impl Monitor {
    fn push(&mut self, source: usize, data: &[u8]) {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.next_seq += 1;
        self.bytes += data.len();
        self.records.push_back(MonitorRecord {
            seq: self.next_seq,
            timestamp_ms,
            source,
            data: data.to_vec(),
        });
        while self.bytes > MONITOR_MAX_BYTES || self.records.len() > MONITOR_MAX_RECORDS {
            match self.records.pop_front() {
                Some(r) => self.bytes -= r.data.len(),
                None => break,
            }
        }
    }

    fn since(&self, after: u64) -> Vec<MonitorRecord> {
        let start = self.records.partition_point(|r| r.seq <= after);
        let start = start.max(self.records.len().saturating_sub(MONITOR_PAGE));
        self.records.range(start..).cloned().collect()
    }
}

struct Shared {
    kind: HubKind,
    stop: AtomicBool,
    writers: Vec<Mutex<Option<Writer>>>,
    counters: Vec<Counters>,
    monitor: Mutex<Monitor>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps in small steps so `stop` takes effect quickly.
    fn sleep(&self, total: Duration) {
        let until = Instant::now() + total;
        while !self.stopped() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn route(&self, src: usize, data: &[u8]) {
        let n = data.len() as u64;
        self.counters[src].rx.fetch_add(n, Ordering::Relaxed);
        self.monitor.lock().push(src, data);

        for target in route_targets(self.kind, self.writers.len(), src) {
            let counters = &self.counters[target];
            let mut slot = self.writers[target].lock();
            match slot.as_mut() {
                Some(w) => match write_with_deadline(w.as_mut(), data) {
                    Ok(()) => {
                        counters.tx.fetch_add(n, Ordering::Relaxed);
                    }
                    Err(e) => {
                        counters.dropped.fetch_add(n, Ordering::Relaxed);
                        counters.set_error(format!("write failed: {e}"));
                    }
                },
                None => {
                    counters.dropped.fetch_add(n, Ordering::Relaxed);
                }
            }
        }
    }

    fn attach(&self, idx: usize, writer: Writer) {
        *self.writers[idx].lock() = Some(writer);
        self.counters[idx].connected.store(true, Ordering::Relaxed);
        self.counters[idx].clear_error();
    }

    fn detach(&self, idx: usize) {
        self.writers[idx].lock().take();
        self.counters[idx].connected.store(false, Ordering::Relaxed);
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn write_with_deadline(w: &mut dyn Write, mut data: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_DEADLINE;
    while !data.is_empty() {
        match w.write(data) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "endpoint accepted no data",
                ));
            }
            Ok(n) => data = &data[n..],
            Err(e) if is_transient(&e) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// A started hub. Dropping it stops all threads and closes every port.
pub struct HubRuntime {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl HubRuntime {
    pub fn start(spec: &HubSpec, driver: Arc<dyn PortDriver>) -> Self {
        let endpoints: Vec<EndpointSpec> = spec.endpoints().map(|e| e.spec.clone()).collect();
        let count = endpoints.len();
        let shared = Arc::new(Shared {
            kind: spec.kind,
            stop: AtomicBool::new(false),
            writers: (0..count).map(|_| Mutex::new(None)).collect(),
            counters: (0..count).map(|_| Counters::default()).collect(),
            monitor: Mutex::new(Monitor::default()),
        });

        let threads = endpoints
            .into_iter()
            .enumerate()
            .filter_map(|(idx, ep)| {
                let shared = Arc::clone(&shared);
                let driver = Arc::clone(&driver);
                std::thread::Builder::new()
                    .name(format!("hub-{}-{idx}", spec.name))
                    .spawn(move || run_endpoint(idx, ep, &driver, shared))
                    .map_err(|e| tracing::error!("cannot spawn endpoint thread: {e}"))
                    .ok()
            })
            .collect();

        Self { shared, threads }
    }

    pub fn status(&self) -> Vec<EndpointStatus> {
        self.shared.counters.iter().map(Counters::snapshot).collect()
    }

    /// Traffic records with `seq > after` (at most the newest 500).
    pub fn monitor_since(&self, after: u64) -> Vec<MonitorRecord> {
        self.shared.monitor.lock().since(after)
    }
}

impl Drop for HubRuntime {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn run_endpoint(idx: usize, spec: EndpointSpec, driver: &Arc<dyn PortDriver>, shared: Arc<Shared>) {
    match &spec {
        EndpointSpec::Serial { port, settings } => serial_loop(idx, port, settings, &shared),
        EndpointSpec::TcpServer { bind } => tcp_server_loop(idx, bind, &shared),
        EndpointSpec::TcpClient { address } => tcp_client_loop(idx, address, &shared),
        EndpointSpec::Virtual { port } => virtual_loop(idx, port, driver.as_ref(), &shared),
    }
    shared.detach(idx);
}

// ---------------------------------------------------------------------------
// Serial
// ---------------------------------------------------------------------------

fn open_serial(port: &str, s: &SerialSettings) -> serialport::Result<Box<dyn serialport::SerialPort>> {
    use serialport::{DataBits, FlowControl as F, Parity as P, StopBits as S};
    serialport::new(port, s.baud_rate)
        .data_bits(match s.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })
        .parity(match s.parity {
            Parity::None => P::None,
            Parity::Odd => P::Odd,
            Parity::Even => P::Even,
        })
        .stop_bits(match s.stop_bits {
            StopBits::One => S::One,
            StopBits::Two => S::Two,
        })
        .flow_control(match s.flow_control {
            FlowControl::None => F::None,
            FlowControl::Software => F::Software,
            FlowControl::Hardware => F::Hardware,
        })
        .timeout(READ_TIMEOUT)
        .open()
}

fn serial_loop(idx: usize, port: &str, settings: &SerialSettings, sh: &Shared) {
    let counters = &sh.counters[idx];
    let mut buf = vec![0u8; 4096];

    while !sh.stopped() {
        let mut reader = match open_serial(port, settings) {
            Ok(p) => p,
            Err(e) => {
                counters.set_error(format!("cannot open {port}: {e}"));
                sh.sleep(RETRY_DELAY);
                continue;
            }
        };
        match reader.try_clone() {
            Ok(writer) => sh.attach(idx, Box::new(writer)),
            Err(e) => {
                counters.set_error(format!("cannot share {port}: {e}"));
                sh.sleep(RETRY_DELAY);
                continue;
            }
        }
        tracing::info!("opened {port}");

        while !sh.stopped() {
            match reader.read(&mut buf) {
                Ok(0) => std::thread::sleep(Duration::from_millis(1)),
                Ok(n) => sh.route(idx, &buf[..n]),
                Err(e) if is_transient(&e) => {}
                Err(e) => {
                    counters.set_error(format!("{port}: {e}"));
                    break;
                }
            }
        }
        sh.detach(idx);
        drop(reader);
        if !sh.stopped() {
            sh.sleep(RETRY_DELAY);
        }
    }
}

// ---------------------------------------------------------------------------
// SerialLoom virtual ports
// ---------------------------------------------------------------------------

/// Writes into a virtual port's control side.
struct ChannelWriter(Arc<dyn ControlChannel>);

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const STATE_POLL: Duration = Duration::from_millis(500);

fn virtual_loop(idx: usize, port: &str, driver: &dyn PortDriver, sh: &Shared) {
    let counters = &sh.counters[idx];
    let mut buf = vec![0u8; 4096];
    while !sh.stopped() {
        let channel: Arc<dyn ControlChannel> = match driver.open_control(port) {
            Ok(c) => Arc::from(c),
            Err(e) => {
                counters.set_error(format!("cannot attach to {port}: {e}"));
                sh.sleep(RETRY_DELAY);
                continue;
            }
        };
        sh.attach(idx, Box::new(ChannelWriter(Arc::clone(&channel))));
        tracing::info!("attached to virtual port {port}");

        let mut polled = Instant::now() - STATE_POLL;
        while !sh.stopped() {
            if polled.elapsed() >= STATE_POLL {
                polled = Instant::now();
                let open = channel.state().is_some_and(|s| s.com_open != 0);
                counters.peer_open.store(open, Ordering::Relaxed);
            }
            match channel.read(&mut buf, READ_TIMEOUT * 2) {
                Ok(0) => {}
                Ok(n) => sh.route(idx, &buf[..n]),
                Err(e) if is_transient(&e) => {}
                Err(e) => {
                    counters.set_error(format!("{port}: {e}"));
                    break;
                }
            }
        }
        sh.detach(idx);
        counters.peer_open.store(false, Ordering::Relaxed);
        if !sh.stopped() {
            sh.sleep(RETRY_DELAY);
        }
    }
}

// ---------------------------------------------------------------------------
// TCP
// ---------------------------------------------------------------------------

fn attach_stream(idx: usize, stream: &TcpStream, sh: &Shared) -> io::Result<()> {
    // Sockets accepted from a non-blocking listener inherit that mode on Windows.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_DEADLINE))?;
    stream.set_nodelay(true)?;
    sh.attach(idx, Box::new(stream.try_clone()?));
    Ok(())
}

/// Reads one chunk. Returns false when the connection is gone.
fn pump(idx: usize, stream: &mut TcpStream, buf: &mut [u8], sh: &Shared) -> bool {
    match stream.read(buf) {
        Ok(0) => false,
        Ok(n) => {
            sh.route(idx, &buf[..n]);
            true
        }
        Err(e) if is_transient(&e) => true,
        Err(e) => {
            sh.counters[idx].set_error(format!("connection lost: {e}"));
            false
        }
    }
}

fn tcp_server_loop(idx: usize, bind: &str, sh: &Shared) {
    let counters = &sh.counters[idx];
    let listener = loop {
        if sh.stopped() {
            return;
        }
        match TcpListener::bind(bind) {
            Ok(l) => break l,
            Err(e) => {
                counters.set_error(format!("cannot listen on {bind}: {e}"));
                sh.sleep(RETRY_DELAY);
            }
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        counters.set_error(format!("listener error: {e}"));
        return;
    }
    counters.clear_error();
    tracing::info!("listening on {bind}");

    let mut current: Option<TcpStream> = None;
    let mut buf = vec![0u8; 4096];
    while !sh.stopped() {
        match listener.accept() {
            Ok((stream, peer)) => {
                tracing::info!("client {peer} connected to {bind}");
                match attach_stream(idx, &stream, sh) {
                    Ok(()) => current = Some(stream),
                    Err(e) => counters.set_error(format!("cannot accept {peer}: {e}")),
                }
            }
            Err(e) if is_transient(&e) => {}
            Err(e) => counters.set_error(format!("accept failed: {e}")),
        }

        match current.as_mut() {
            None => std::thread::sleep(Duration::from_millis(50)),
            Some(stream) => {
                if !pump(idx, stream, &mut buf, sh) {
                    sh.detach(idx);
                    current = None;
                }
            }
        }
    }
}

fn tcp_client_loop(idx: usize, address: &str, sh: &Shared) {
    let counters = &sh.counters[idx];
    let mut buf = vec![0u8; 4096];
    while !sh.stopped() {
        let Some(addr) = address.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
            counters.set_error(format!("cannot resolve {address}"));
            sh.sleep(RETRY_DELAY);
            continue;
        };
        let mut stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => s,
            Err(e) => {
                counters.set_error(format!("cannot connect to {address}: {e}"));
                sh.sleep(RETRY_DELAY);
                continue;
            }
        };
        if let Err(e) = attach_stream(idx, &stream, sh) {
            counters.set_error(format!("socket setup failed: {e}"));
            sh.sleep(RETRY_DELAY);
            continue;
        }
        tracing::info!("connected to {address}");
        while !sh.stopped() && pump(idx, &mut stream, &mut buf, sh) {}
        sh.detach(idx);
        if !sh.stopped() {
            sh.sleep(RETRY_DELAY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HubEndpoint, HubKind};

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn ep(spec: EndpointSpec) -> HubEndpoint {
        HubEndpoint { spec, created: false }
    }

    fn connect(port: u16) -> TcpStream {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    return s;
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("cannot connect: {e}"),
            }
        }
    }

    fn wait_connected(hub: &HubRuntime, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while hub.status().iter().filter(|s| s.connected).count() < n {
            assert!(Instant::now() < deadline, "endpoints did not connect");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A split hub built from TCP servers: master data reaches every member,
    /// member data reaches only the master.
    #[test]
    fn split_routes_over_tcp() {
        let ports = [free_port(), free_port(), free_port()];
        let mk = |p: u16| {
            ep(EndpointSpec::TcpServer {
                bind: format!("127.0.0.1:{p}"),
            })
        };
        let spec = HubSpec {
            id: "t".into(),
            name: "test".into(),
            kind: HubKind::Split,
            master: mk(ports[0]),
            members: vec![mk(ports[1]), mk(ports[2])],
            autostart: false,
        };
        let hub = HubRuntime::start(&spec, Arc::new(crate::driver::demo::DemoDriver::new()));
        let mut master = connect(ports[0]);
        let mut m1 = connect(ports[1]);
        let mut m2 = connect(ports[2]);
        wait_connected(&hub, 3);

        master.write_all(b"hello").unwrap();
        let mut buf = [0u8; 5];
        m1.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        m2.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");

        m1.write_all(b"ack").unwrap();
        let mut ack = [0u8; 3];
        master.read_exact(&mut ack).unwrap();
        assert_eq!(&ack, b"ack");

        // m2 must not hear m1.
        m2.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        assert!(m2.read(&mut buf).is_err());

        let status = hub.status();
        assert_eq!(status[0].rx_bytes, 5);
        assert_eq!(status[0].tx_bytes, 3);
        assert_eq!(status[1].tx_bytes, 5);
        assert!(!hub.monitor_since(0).is_empty());
    }

    #[test]
    fn monitor_is_bounded() {
        let mut m = Monitor::default();
        for _ in 0..(MONITOR_MAX_RECORDS + 10) {
            m.push(0, b"x");
        }
        assert_eq!(m.records.len(), MONITOR_MAX_RECORDS);
        assert_eq!(m.since(0).len(), MONITOR_PAGE);
        let last = m.next_seq;
        assert!(m.since(last).is_empty());
    }
}
