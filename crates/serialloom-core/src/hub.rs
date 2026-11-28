//! User-mode data router.
//!
//! A reader per endpoint queues data for one writer thread per destination,
//! so a slow destination never blocks the writes to the others.
//!
//! When a destination's queue is full, a source that can be held back (a
//! virtual port, TCP, a file) waits, so the sending program waits instead of
//! losing data. A source that cannot be held back (a real serial port, UDP)
//! is never held up: the full destination loses that data, counted as dropped.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::{Condvar, Mutex};

use crate::capture::{self, Capture, CaptureAction, CaptureStatus, Direction};
use crate::driver::{ControlChannel, PortDriver};
use crate::model::{
    EndpointSpec, EndpointStatus, FlowControl, HubKind, HubSpec, LineStatus, MonitorRecord, Parity,
    SerialSettings, StopBits, route_targets,
};

const READ_TIMEOUT: Duration = Duration::from_millis(25);
const RETRY_DELAY: Duration = Duration::from_millis(1500);
const OUTBOX_MAX_BYTES: usize = 64 * 1024;
const OUTBOX_MAX_CHUNKS: usize = 256;
/// Longest a source that can be held back waits for a full target queue.
const HOLD_LIMIT: Duration = Duration::from_secs(5);
const WRITE_DEADLINE: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const MONITOR_MAX_BYTES: usize = 256 * 1024;
const MONITOR_MAX_RECORDS: usize = 4000;
const MONITOR_PAGE: usize = 500;

/// Where an endpoint's incoming data is written.
enum Writer {
    /// A serial port or TCP connection.
    Stream(Box<dyn Write + Send>),
    /// A SerialLoom virtual port.
    Channel(Arc<dyn ControlChannel>),
    Udp(crate::udp::UdpWriter),
}

impl Writer {
    /// `hold`: the data's source can be held back, so the write may wait
    /// for a slow program. Without it, what does not fit right now is not
    /// accepted, and other targets are not held up.
    fn write(&mut self, data: &[u8], hold: bool) -> io::Result<usize> {
        match self {
            Writer::Stream(w) => w.write(data),
            Writer::Udp(_) => unreachable!("UDP writes use datagram framing"),
            Writer::Channel(c) if hold => c.write(data),
            Writer::Channel(c) => c.write_now(data),
        }
    }
}

#[derive(Default)]
struct Counters {
    rx: AtomicU64,
    tx: AtomicU64,
    dropped: AtomicU64,
    connected: AtomicBool,
    peer_open: AtomicBool,
    /// The endpoint's own problem (cannot open, connection lost, ...).
    last_error: Mutex<Option<String>>,
    /// Why data for this endpoint is being dropped. Clears once a write
    /// succeeds again.
    warning: Mutex<Option<String>>,
    line: Mutex<Option<LineStatus>>,
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
            warning: self.warning.lock().clone(),
            line: self.line.lock().clone(),
        }
    }

    /// A write to this endpoint failed, so its data is being dropped.
    fn set_warning(&self, e: &io::Error) {
        let message = if e.kind() == io::ErrorKind::WriteZero {
            "program not reading; data is being dropped".to_string()
        } else {
            format!("write failed: {e}")
        };
        let mut slot = self.warning.lock();
        if slot.as_deref() != Some(message.as_str()) {
            tracing::warn!("{message}");
            *slot = Some(message);
        }
    }

    fn warn_full_outbox(&self) {
        let mut warning = self.warning.lock();
        let message = "output queue full; data is being dropped";
        if warning.as_deref() != Some(message) {
            tracing::warn!("{message}");
            *warning = Some(message.into());
        }
    }

    fn clear_warning(&self) {
        if self.warning.lock().take().is_some() {
            tracing::info!("data is being delivered again");
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
        *self.warning.lock() = None;
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

type AttachedWriter = Arc<Mutex<Writer>>;

#[derive(Default)]
struct OutboxState {
    writer: Option<AttachedWriter>,
    channel: Option<Arc<dyn ControlChannel>>,
    pending: VecDeque<(Arc<[u8]>, bool, usize)>,
    bytes: usize,
}

impl OutboxState {
    fn replace(&mut self, writer: Option<Writer>, counters: &Counters) {
        counters.dropped.fetch_add(self.bytes as u64, Ordering::Relaxed);
        self.pending.clear();
        self.bytes = 0;
        self.channel = match &writer {
            Some(Writer::Channel(channel)) => Some(Arc::clone(channel)),
            _ => None,
        };
        counters.connected.store(writer.is_some(), Ordering::Relaxed);
        self.writer = writer.map(|writer| Arc::new(Mutex::new(writer)));
    }
}

#[derive(Default)]
struct Outbox {
    state: Mutex<OutboxState>,
    /// Signalled when data is queued.
    ready: Condvar,
    /// Signalled when queued data leaves, the writer changes, or the hub stops.
    space: Condvar,
}

impl OutboxState {
    fn full(&self, len: usize) -> bool {
        // A chunk always fits into an empty queue, however large.
        !self.pending.is_empty()
            && (self.bytes + len > OUTBOX_MAX_BYTES || self.pending.len() >= OUTBOX_MAX_CHUNKS)
    }
}

struct Shared {
    kind: HubKind,
    /// Two virtual ports bridged together: each program's DTR reaches the
    /// other as DSR and DCD, as through a null-modem cable. Elsewhere DSR
    /// and DCD are on while SerialLoom is attached.
    null_modem: bool,
    stop: AtomicBool,
    /// Per endpoint: its sender can be held back while a target is slow.
    /// Not a real serial port: its device keeps sending (SerialLoom opens
    /// it with RTS on, so even RTS/CTS flow control does not pause it).
    /// Not UDP either: datagrams nobody reads in time are lost.
    holdable: Vec<bool>,
    sources_only: Vec<bool>,
    outboxes: Vec<Outbox>,
    counters: Vec<Counters>,
    monitor: Mutex<Monitor>,
    captures: Vec<Mutex<Option<Capture>>>,
    /// Per endpoint: a GPS simulator's settings, which can change while it runs.
    simulators: Vec<Option<Arc<Mutex<crate::gps::LiveSettings>>>>,
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
        self.record(src, Direction::Rx, data);

        let hold = self.holdable[src];
        let mut payload = None;
        for target in route_targets(self.kind, self.outboxes.len(), src) {
            if self.sources_only[target] {
                continue;
            }
            let counters = &self.counters[target];
            let outbox = &self.outboxes[target];
            let mut state = outbox.state.lock();
            if hold {
                // The source can be held back: wait for the target to catch up,
                // so the sending program waits instead of losing data. The
                // target's writer always moves on (a stalled program is given
                // up on after READ_STALL), so this ends; HOLD_LIMIT is a backstop.
                let deadline = Instant::now() + HOLD_LIMIT;
                while state.writer.is_some()
                    && !self.stopped()
                    && state.full(data.len())
                    && !outbox.space.wait_until(&mut state, deadline).timed_out()
                {}
            }
            if state.writer.is_none() || self.stopped() {
                counters.dropped.fetch_add(n, Ordering::Relaxed);
            } else if state.full(data.len()) {
                counters.dropped.fetch_add(n, Ordering::Relaxed);
                counters.warn_full_outbox();
            } else {
                let payload = payload.get_or_insert_with(|| Arc::<[u8]>::from(data));
                state.pending.push_back((Arc::clone(payload), hold, src));
                state.bytes += data.len();
                outbox.ready.notify_one();
            }
        }
    }

    fn record(&self, idx: usize, direction: Direction, data: &[u8]) {
        if let Some(capture) = self.captures[idx].lock().as_ref() {
            capture.record(direction, data);
        }
    }

    fn write_next(&self, idx: usize, wait: bool) -> bool {
        let outbox = &self.outboxes[idx];
        let counters = &self.counters[idx];
        let (writer, data, hold, source) = {
            let mut state = outbox.state.lock();
            while wait && !self.stopped() && state.pending.is_empty() {
                outbox.ready.wait(&mut state);
            }
            if self.stopped() {
                return false;
            }
            let Some((data, hold, source)) = state.pending.pop_front() else {
                return false;
            };
            state.bytes -= data.len();
            outbox.space.notify_all();
            (
                Arc::clone(state.writer.as_ref().expect("queued data requires a writer")),
                data,
                hold,
                source,
            )
        };

        let (written, dropped, error) = match &mut *writer.lock() {
            Writer::Udp(writer) => writer.send(source, &data, |bytes| self.record(idx, Direction::Tx, bytes)),
            writer => {
                let (written, error) = write_with_deadline(writer, &data, hold, |bytes| {
                    self.record(idx, Direction::Tx, bytes)
                });
                (written, data.len() - written, error)
            }
        };
        counters.tx.fetch_add(written as u64, Ordering::Relaxed);
        counters.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
        let state = outbox.state.lock();
        // An in-flight write belongs to the old connection if the reader replaced it.
        if state
            .writer
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &writer))
        {
            if let Some(error) = error {
                counters.set_warning(&error);
            } else if state.pending.is_empty() {
                counters.clear_warning();
            }
        }
        true
    }

    fn attach(&self, idx: usize, writer: Writer) {
        let mut state = self.outboxes[idx].state.lock();
        state.replace(Some(writer), &self.counters[idx]);
        self.outboxes[idx].space.notify_all();
        self.counters[idx].clear_error();
    }

    fn detach(&self, idx: usize) {
        self.outboxes[idx].state.lock().replace(None, &self.counters[idx]);
        self.outboxes[idx].space.notify_all();
    }

    /// The virtual port attached at `idx`, if any.
    fn channel(&self, idx: usize) -> Option<Arc<dyn ControlChannel>> {
        self.outboxes[idx].state.lock().channel.clone()
    }

    /// Null-modem pair: the other end of `idx`.
    fn peer(&self, idx: usize) -> Option<Arc<dyn ControlChannel>> {
        self.channel(1 - idx)
    }

    /// Null-modem pair: the program at `idx` sees the other end's DTR as it
    /// is now (off while nothing is attached there). Changes are passed on
    /// as they happen; this also runs now and then, so a change that
    /// crossed an attach cannot leave a stale line behind.
    fn follow_peer(&self, idx: usize, channel: &dyn ControlChannel) {
        let dtr = self.peer(idx).and_then(|p| p.output_dtr()).unwrap_or(false);
        channel.set_peer_dtr(dtr);
    }

    /// Null-modem pair: `channel` (at `idx`) was just attached. Both ends
    /// take over each other's DTR. Returns the DTR passed on.
    fn wire_attached(&self, idx: usize, channel: &dyn ControlChannel) -> Option<bool> {
        self.follow_peer(idx, channel);
        let own = channel.output_dtr();
        if let (Some(dtr), Some(peer)) = (own, self.peer(idx)) {
            peer.set_peer_dtr(dtr);
        }
        own
    }

    /// Null-modem pair: passes a change of the program's DTR at `idx` to
    /// the other end. Returns the DTR passed on.
    fn pass_dtr(&self, idx: usize, channel: &dyn ControlChannel, last: Option<bool>) -> Option<bool> {
        let now = channel.output_dtr();
        if let Some(dtr) = now
            && now != last
            && let Some(peer) = self.peer(idx)
        {
            peer.set_peer_dtr(dtr);
        }
        now
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn write_with_deadline(
    w: &mut Writer,
    mut data: &[u8],
    hold: bool,
    mut sent: impl FnMut(&[u8]),
) -> (usize, Option<io::Error>) {
    let len = data.len();
    let deadline = Instant::now() + WRITE_DEADLINE;
    let error = loop {
        if data.is_empty() {
            break None;
        }
        match w.write(data, hold) {
            Ok(0) => {
                break Some(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "endpoint accepted no data",
                ));
            }
            Ok(n) => {
                sent(&data[..n]);
                data = &data[n..];
            }
            Err(e) if is_transient(&e) && Instant::now() < deadline => {}
            Err(e) => break Some(e),
        }
    };
    (len - data.len(), error)
}

/// A started hub. Dropping it stops all threads and closes every port.
pub struct HubRuntime {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl HubRuntime {
    pub fn start(spec: &HubSpec, driver: Arc<dyn PortDriver>) -> Self {
        Self::start_with_captures(spec, driver, Default::default())
    }

    pub(crate) fn start_with_captures(
        spec: &HubSpec,
        driver: Arc<dyn PortDriver>,
        mut armed: std::collections::HashMap<usize, Capture>,
    ) -> Self {
        let endpoints: Vec<EndpointSpec> = spec.endpoints().map(|e| e.spec.clone()).collect();
        let count = endpoints.len();
        let shared = Arc::new(Shared {
            kind: spec.kind,
            null_modem: spec.kind == HubKind::Bridge
                && count == 2
                && endpoints
                    .iter()
                    .all(|e| matches!(e, EndpointSpec::Virtual { .. })),
            stop: AtomicBool::new(false),
            sources_only: endpoints
                .iter()
                .map(|ep| match ep {
                    EndpointSpec::Replay { .. } => true,
                    EndpointSpec::Udp { settings } => settings.destination.trim().is_empty(),
                    _ => false,
                })
                .collect(),
            holdable: endpoints
                .iter()
                .map(|e| !matches!(e, EndpointSpec::Serial { .. } | EndpointSpec::Udp { .. }))
                .collect(),
            outboxes: (0..count).map(|_| Outbox::default()).collect(),
            counters: (0..count).map(|_| Counters::default()).collect(),
            monitor: Mutex::new(Monitor::default()),
            captures: (0..count).map(|index| Mutex::new(armed.remove(&index))).collect(),
            simulators: endpoints
                .iter()
                .map(|e| match e {
                    EndpointSpec::Replay { settings } if settings.format.is_gps() => {
                        settings.gps.as_deref().cloned().map(|gps| {
                            Arc::new(Mutex::new(crate::gps::LiveSettings {
                                gps,
                                cycle_ms: settings.cycle_ms,
                                version: 0,
                            }))
                        })
                    }
                    _ => None,
                })
                .collect(),
        });

        let mut threads = Vec::with_capacity(2 * count);
        for (idx, ep) in endpoints.into_iter().enumerate() {
            let writer = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name(format!("hub-{}-{idx}-writer", spec.name))
                .spawn(move || while writer.write_next(idx, true) {})
            {
                Ok(thread) => threads.push(thread),
                Err(error) => {
                    shared.counters[idx].set_error(format!("cannot spawn writer: {error}"));
                    continue;
                }
            }
            let reader = Arc::clone(&shared);
            let driver = Arc::clone(&driver);
            match std::thread::Builder::new()
                .name(format!("hub-{}-{idx}-reader", spec.name))
                .spawn(move || run_endpoint(idx, ep, &driver, reader))
            {
                Ok(thread) => threads.push(thread),
                Err(error) => shared.counters[idx].set_error(format!("cannot spawn reader: {error}")),
            }
        }

        Self { shared, threads }
    }

    pub fn status(&self) -> Vec<EndpointStatus> {
        self.shared.counters.iter().map(Counters::snapshot).collect()
    }

    /// Gives a running GPS simulator new settings (already checked).
    pub(crate) fn update_simulator(
        &self,
        index: usize,
        gps: crate::model::GpsSettings,
        cycle_ms: u32,
    ) -> bool {
        let Some(Some(live)) = self.shared.simulators.get(index) else {
            return false;
        };
        let mut live = live.lock();
        live.gps = gps;
        live.cycle_ms = cycle_ms;
        live.version += 1;
        true
    }

    pub fn capture(&self, index: usize, action: CaptureAction) -> crate::Result<CaptureStatus> {
        let slot = self
            .shared
            .captures
            .get(index)
            .ok_or_else(|| crate::Error::Invalid("invalid capture endpoint".into()))?;
        let include_records = !matches!(action, CaptureAction::Summary);
        match action {
            CaptureAction::Start(request) => {
                {
                    let current = slot.lock();
                    if current.as_ref().is_some_and(Capture::running) {
                        return Err(crate::Error::Invalid(
                            "a capture is already running on this endpoint".into(),
                        ));
                    }
                }
                // File creation and free space checks take place outside the routing lock.
                let capture = Capture::start(request)?;
                *slot.lock() = Some(capture);
            }
            CaptureAction::Stop => {
                if let Some(capture) = slot.lock().as_ref() {
                    capture.stop();
                }
            }
            CaptureAction::CloseWindow => {
                if let Some(capture) = slot.lock().as_ref().filter(|capture| !capture.is_file()) {
                    capture.stop();
                }
            }
            CaptureAction::Status | CaptureAction::Summary => {}
        }
        Ok(capture::status(slot.lock().as_ref(), include_records))
    }

    /// Traffic records with `seq > after` (at most the newest 500).
    pub fn monitor_since(&self, after: u64) -> Vec<MonitorRecord> {
        self.shared.monitor.lock().since(after)
    }
}

impl Drop for HubRuntime {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for outbox in &self.shared.outboxes {
            let _state = outbox.state.lock();
            outbox.ready.notify_one();
            outbox.space.notify_all();
        }
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
        EndpointSpec::Udp { settings } => udp_loop(idx, settings, &shared),
        EndpointSpec::Replay { settings } => {
            let counters = &shared.counters[idx];
            let emit = |data: &[u8]| {
                counters.connected.store(true, Ordering::Relaxed);
                shared.route(idx, data);
            };
            let result = match shared.simulators[idx].clone() {
                Some(live) => crate::replay::run_gps(settings.format, &live, || shared.stopped(), emit),
                None => crate::replay::run(settings, || shared.stopped(), emit),
            };
            if let Err(error) = result {
                counters.set_error(format!("replay failed: {error}"));
            }
        }
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
            Ok(writer) => sh.attach(idx, Writer::Stream(Box::new(writer))),
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

const STATE_POLL: Duration = Duration::from_millis(500);

fn virtual_loop(idx: usize, port: &str, driver: &dyn PortDriver, sh: &Shared) {
    let counters = &sh.counters[idx];
    let mut buf = vec![0u8; 4096];
    while !sh.stopped() {
        let opened = if sh.null_modem {
            driver.open_paired(port)
        } else {
            driver.open_control(port)
        };
        let channel: Arc<dyn ControlChannel> = match opened {
            Ok(c) => Arc::from(c),
            Err(e) => {
                counters.set_error(format!("cannot attach to {port}: {e}"));
                sh.sleep(RETRY_DELAY);
                continue;
            }
        };
        sh.attach(idx, Writer::Channel(Arc::clone(&channel)));
        tracing::info!("attached to virtual port {port}");
        let mut dtr = if sh.null_modem {
            sh.wire_attached(idx, channel.as_ref())
        } else {
            None
        };

        let mut polled = Instant::now() - STATE_POLL;
        while !sh.stopped() {
            if sh.null_modem {
                dtr = sh.pass_dtr(idx, channel.as_ref(), dtr);
            }
            if polled.elapsed() >= STATE_POLL {
                polled = Instant::now();
                if sh.null_modem {
                    sh.follow_peer(idx, channel.as_ref());
                }
                let state = channel.state().filter(|s| s.com_open != 0);
                counters.peer_open.store(state.is_some(), Ordering::Relaxed);
                *counters.line.lock() = state.map(|s| {
                    LineStatus::from_raw(
                        s.baud_rate,
                        s.data_bits,
                        s.parity,
                        s.stop_bits,
                        s.dtr_rts & 1 != 0,
                        s.dtr_rts & 2 != 0,
                    )
                });
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
        if sh.null_modem
            && let Some(peer) = sh.peer(idx)
        {
            // Nothing drives this end any more, like an unplugged cable.
            peer.set_peer_dtr(false);
        }
        counters.peer_open.store(false, Ordering::Relaxed);
        *counters.line.lock() = None;
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
    sh.attach(idx, Writer::Stream(Box::new(stream.try_clone()?)));
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

fn udp_loop(idx: usize, settings: &crate::model::UdpSettings, sh: &Shared) {
    let counters = &sh.counters[idx];
    let (bind, destination) = match crate::udp::addresses(settings) {
        Ok(addresses) => addresses,
        Err(error) => {
            counters.set_error(error.to_string());
            return;
        }
    };

    let mut buffer = vec![0; 65_536];
    while !sh.stopped() {
        let setup = (|| {
            let socket = UdpSocket::bind(bind)?;
            socket.set_read_timeout(Some(READ_TIMEOUT))?;
            if let Some(destination) = destination {
                if destination.is_ipv4() {
                    socket.set_broadcast(true)?;
                }
                socket.set_write_timeout(Some(READ_TIMEOUT))?;
                let writer = crate::udp::UdpWriter::new(socket.try_clone()?, destination, settings.framing);
                sh.attach(idx, Writer::Udp(writer));
            } else {
                counters.clear_error();
                counters.connected.store(true, Ordering::Relaxed);
            }
            Ok::<_, io::Error>(socket)
        })();

        match setup {
            Ok(socket) => {
                while !sh.stopped() {
                    match socket.recv_from(&mut buffer) {
                        Ok((count, _)) if settings.receive && count > 0 => sh.route(idx, &buffer[..count]),
                        Ok(_) => {}
                        Err(error) if is_transient(&error) => {}
                        Err(error) => {
                            // Winsock reports ICMP Port Unreachable on the next receive.
                            if cfg!(windows) && error.raw_os_error() == Some(10054) {
                                continue;
                            }
                            counters.set_error(format!("UDP receive failed on {bind}: {error}"));
                            break;
                        }
                    }
                }
                sh.detach(idx);
            }
            Err(error) => counters.set_error(format!("cannot open UDP socket on {bind}: {error}")),
        }
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

    #[test]
    fn bridge_forwards_bytes_between_tcp_and_udp() {
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let port = free_port();
        let local = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = local.local_addr().unwrap();
        drop(local);

        let spec = HubSpec {
            id: "udp-test".into(),
            name: "UDP test".into(),
            kind: HubKind::Bridge,
            master: ep(EndpointSpec::TcpServer {
                bind: format!("127.0.0.1:{port}"),
            }),
            members: vec![ep(EndpointSpec::Udp {
                settings: crate::model::UdpSettings {
                    bind: address.to_string(),
                    destination: peer.local_addr().unwrap().to_string(),
                    receive: true,
                    framing: crate::model::UdpFraming::Raw,
                },
            })],
            autostart: false,
        };
        let hub = HubRuntime::start(&spec, Arc::new(crate::driver::demo::DemoDriver::new()));
        let mut tcp = connect(port);
        wait_connected(&hub, 2);
        let mut buffer = [0; 3];
        tcp.write_all(b"\0\xFFx").unwrap();
        let mut received = Vec::new();
        while received.len() < buffer.len() {
            let (count, source) = peer.recv_from(&mut buffer).unwrap();
            assert_eq!(source, address);
            received.extend_from_slice(&buffer[..count]);
        }
        assert_eq!(received, b"\0\xFFx");
        peer.send_to(b"ack", address).unwrap();
        tcp.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"ack");
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

    /// Records how a virtual port was written to.
    #[derive(Default)]
    struct Recorder {
        waiting: AtomicU64,
        now: AtomicU64,
        /// Accepts nothing, like a port whose program stopped reading.
        full: AtomicBool,
    }

    impl ControlChannel for Recorder {
        fn read(&self, _buf: &mut [u8], _timeout: Duration) -> io::Result<usize> {
            Ok(0)
        }

        fn write(&self, data: &[u8]) -> io::Result<usize> {
            self.waiting.fetch_add(1, Ordering::Relaxed);
            Ok(data.len())
        }

        fn write_now(&self, data: &[u8]) -> io::Result<usize> {
            self.now.fetch_add(1, Ordering::Relaxed);
            Ok(if self.full.load(Ordering::Relaxed) {
                0
            } else {
                data.len()
            })
        }

        fn state(&self) -> Option<crate::driver::ControlState> {
            None
        }
    }

    fn calls(r: &Recorder) -> (u64, u64) {
        (r.now.load(Ordering::Relaxed), r.waiting.load(Ordering::Relaxed))
    }

    /// A hub without endpoint threads; `holdable[i]` false means endpoint
    /// `i` is a real serial port.
    fn shared(kind: HubKind, holdable: Vec<bool>) -> Shared {
        let n = holdable.len();
        Shared {
            kind,
            null_modem: false,
            stop: AtomicBool::new(false),
            holdable,
            sources_only: vec![false; n],
            outboxes: (0..n).map(|_| Outbox::default()).collect(),
            counters: (0..n).map(|_| Counters::default()).collect(),
            monitor: Mutex::new(Monitor::default()),
            captures: (0..n).map(|_| Mutex::new(None)).collect(),
            simulators: (0..n).map(|_| None).collect(),
        }
    }

    /// Data from a real serial port must not wait for a virtual port: the
    /// device cannot be held back, and other members would be held up.
    #[test]
    fn serial_data_does_not_wait_for_virtual_ports() {
        let ports: Vec<Arc<Recorder>> = (0..3).map(|_| Arc::new(Recorder::default())).collect();
        let sh = shared(HubKind::Split, vec![false, true, true]);
        for (i, p) in ports.iter().enumerate() {
            sh.attach(i, Writer::Channel(Arc::clone(p) as Arc<dyn ControlChannel>));
        }

        // Member 1 is full: it loses the data, member 2 still gets it.
        ports[1].full.store(true, Ordering::Relaxed);
        sh.route(0, b"device");
        assert!(sh.write_next(1, false));
        assert!(sh.write_next(2, false));
        assert_eq!(calls(&ports[1]), (1, 0));
        assert_eq!(calls(&ports[2]), (1, 0));
        let status: Vec<EndpointStatus> = sh.counters.iter().map(Counters::snapshot).collect();
        assert_eq!((status[1].tx_bytes, status[1].dropped_bytes), (0, 6));
        assert_eq!((status[2].tx_bytes, status[2].dropped_bytes), (6, 0));

        // A program's data can be held back, so it may wait as before.
        sh.route(2, b"program");
        assert!(sh.write_next(0, false));
        assert_eq!(calls(&ports[0]), (0, 1));
    }

    #[test]
    fn overflow_message_clears_once_data_flows_again() {
        let port = Arc::new(Recorder::default());
        let sh = shared(HubKind::Bridge, vec![false, true]);
        sh.attach(1, Writer::Channel(Arc::clone(&port) as Arc<dyn ControlChannel>));
        let status = |sh: &Shared| sh.counters[1].snapshot();

        // Dropping is a warning, not an error.
        port.full.store(true, Ordering::Relaxed);
        sh.route(0, b"lost");
        assert!(sh.write_next(1, false));
        assert_eq!(
            status(&sh).warning.as_deref(),
            Some("program not reading; data is being dropped")
        );
        assert_eq!(status(&sh).last_error, None);

        // Data flows again: the warning goes, the dropped count stays.
        port.full.store(false, Ordering::Relaxed);
        sh.route(0, b"kept");
        assert!(sh.write_next(1, false));
        assert_eq!(status(&sh).warning, None);
        assert_eq!((status(&sh).tx_bytes, status(&sh).dropped_bytes), (4, 4));

        // The endpoint's own problem is not cleared by data flowing.
        sh.counters[1].set_error("connection lost: reset".into());
        sh.route(0, b"more");
        assert!(sh.write_next(1, false));
        assert_eq!(status(&sh).last_error.as_deref(), Some("connection lost: reset"));
    }

    #[test]
    fn slow_split_output_does_not_delay_healthy_output_or_replay_on_reconnect() {
        use std::sync::mpsc;

        struct BlockedWriter {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }

        impl Write for BlockedWriter {
            fn write(&mut self, _data: &[u8]) -> io::Result<usize> {
                self.entered.send(()).unwrap();
                self.release
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(io::Error::other)?;
                Ok(2)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        // The master is a real serial port: it cannot be held back, so a slow
        // member loses its own data and the others are not held up.
        let sh = shared(HubKind::Split, vec![false, true, true]);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        sh.attach(
            1,
            Writer::Stream(Box::new(BlockedWriter {
                entered: entered_tx,
                release: release_rx,
            })),
        );
        let healthy = Arc::new(Recorder::default());
        sh.attach(2, Writer::Channel(healthy.clone()));
        sh.route(0, b"aa");

        std::thread::scope(|scope| {
            let writer = scope.spawn(|| sh.write_next(1, false));
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(sh.write_next(2, false));

            let sample = [0_u8; 4096];
            for _ in 0..=OUTBOX_MAX_BYTES / sample.len() {
                sh.route(0, &sample);
                assert!(sh.write_next(2, false));
            }
            assert_eq!(sh.counters[2].snapshot().dropped_bytes, 0);
            assert_eq!(sh.counters[1].snapshot().dropped_bytes, sample.len() as u64);

            sh.detach(1);
            let reconnected = Arc::new(Recorder::default());
            sh.attach(1, Writer::Channel(reconnected.clone()));
            sh.route(0, b"new");
            release_tx.send(()).unwrap();
            assert!(writer.join().unwrap());
            assert_eq!(
                sh.counters[1].snapshot().dropped_bytes,
                (OUTBOX_MAX_BYTES + sample.len()) as u64
            );
            assert!(sh.write_next(1, false));
            assert!(!sh.write_next(1, false));
            assert_eq!(calls(&reconnected), (1, 0));
            assert_eq!(sh.counters[1].snapshot().tx_bytes, 5);
        });
    }

    /// A source that can be held back (here a virtual port) waits for a full
    /// destination queue instead of losing data, so its program's write waits.
    #[test]
    fn held_back_source_waits_for_a_full_queue() {
        let target = Arc::new(Recorder::default());
        let sh = shared(HubKind::Bridge, vec![true, true]);
        sh.attach(1, Writer::Channel(target.clone()));
        let chunk = [7_u8; 4096];
        for _ in 0..OUTBOX_MAX_BYTES / chunk.len() {
            sh.route(0, &chunk);
        }

        let routed = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                sh.route(0, b"more");
                routed.store(true, Ordering::Relaxed);
            });
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                !routed.load(Ordering::Relaxed),
                "a full queue must hold the source back"
            );
            assert!(sh.write_next(1, false));
        });
        assert!(routed.load(Ordering::Relaxed));
        assert_eq!(sh.counters[1].snapshot().dropped_bytes, 0);
    }

    /// A virtual port whose program's lines the test sets. Records what the
    /// hub tells it about the other end.
    #[derive(Default)]
    struct Wire {
        dtr: AtomicBool,
        /// `None` unless opened as one end of a pair.
        peer_dtr: Mutex<Option<bool>>,
        attached: AtomicBool,
        /// Reads fail, as when the port goes away.
        broken: AtomicBool,
    }

    struct WireChannel(Arc<Wire>);

    impl ControlChannel for WireChannel {
        fn read(&self, _buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
            if self.0.broken.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
            }
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
            Ok(0)
        }

        fn write(&self, data: &[u8]) -> io::Result<usize> {
            Ok(data.len())
        }

        fn state(&self) -> Option<crate::driver::ControlState> {
            None
        }

        fn output_dtr(&self) -> Option<bool> {
            Some(self.0.dtr.load(Ordering::Relaxed))
        }

        fn set_peer_dtr(&self, dtr: bool) {
            let mut slot = self.0.peer_dtr.lock();
            if slot.is_some() {
                *slot = Some(dtr);
            }
        }
    }

    impl Drop for WireChannel {
        fn drop(&mut self) {
            *self.0.peer_dtr.lock() = None;
            self.0.attached.store(false, Ordering::Relaxed);
        }
    }

    /// Serves `Wire`s by port name.
    struct WireDriver(Vec<(String, Arc<Wire>)>);

    impl WireDriver {
        fn open(&self, port: &str, paired: bool) -> crate::error::Result<Box<dyn ControlChannel>> {
            let (_, wire) = self.0.iter().find(|(n, _)| n == port).expect("known port");
            *wire.peer_dtr.lock() = paired.then_some(false);
            wire.attached.store(true, Ordering::Relaxed);
            Ok(Box::new(WireChannel(Arc::clone(wire))))
        }
    }

    impl PortDriver for WireDriver {
        fn info(&self) -> crate::model::DriverInfo {
            crate::model::DriverInfo {
                id: "wire".into(),
                name: "test".into(),
                available: true,
                installable: false,
                location: None,
                message: None,
            }
        }

        fn list_ports(&self) -> crate::error::Result<Vec<crate::model::VirtualPortInfo>> {
            Ok(Vec::new())
        }

        fn create_ports(
            &self,
            _names: &[String],
        ) -> crate::error::Result<Vec<crate::model::VirtualPortInfo>> {
            Ok(Vec::new())
        }

        fn remove_ports(&self, _names: &[String]) -> crate::error::Result<()> {
            Ok(())
        }

        fn open_control(&self, port: &str) -> crate::error::Result<Box<dyn ControlChannel>> {
            self.open(port, false)
        }

        fn open_paired(&self, port: &str) -> crate::error::Result<Box<dyn ControlChannel>> {
            self.open(port, true)
        }
    }

    fn virt(port: &str) -> HubEndpoint {
        ep(EndpointSpec::Virtual { port: port.into() })
    }

    fn start_wired(
        kind: HubKind,
        master: HubEndpoint,
        members: Vec<HubEndpoint>,
        wires: &[Arc<Wire>],
    ) -> HubRuntime {
        let names = ["COM1", "COM2", "COM3"];
        let driver = WireDriver(
            wires
                .iter()
                .zip(names)
                .map(|(w, n)| (n.to_string(), Arc::clone(w)))
                .collect(),
        );
        let spec = HubSpec {
            id: "w".into(),
            name: "wired".into(),
            kind,
            master,
            members,
            autostart: false,
        };
        HubRuntime::start(&spec, Arc::new(driver))
    }

    /// Waits until `f` holds.
    fn eventually(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn peer_dtr(w: &Wire) -> Option<bool> {
        *w.peer_dtr.lock()
    }

    /// Issue #17: a virtual pair is a null-modem cable. Each program's DTR
    /// reaches the other end as DSR/DCD.
    #[test]
    fn virtual_pair_wires_dtr() {
        let wires: Vec<Arc<Wire>> = (0..2).map(|_| Arc::new(Wire::default())).collect();
        let (a, b) = (&wires[0], &wires[1]);
        // A program already has COM1 open with DTR on when the pair starts.
        a.dtr.store(true, Ordering::Relaxed);
        let hub = start_wired(HubKind::Bridge, virt("COM1"), vec![virt("COM2")], &wires);
        eventually("both attached", || {
            a.attached.load(Ordering::Relaxed) && b.attached.load(Ordering::Relaxed)
        });
        eventually("COM2 sees COM1's DTR", || peer_dtr(b) == Some(true));
        eventually("COM1 sees COM2's DTR off", || peer_dtr(a) == Some(false));

        b.dtr.store(true, Ordering::Relaxed);
        eventually("COM1 sees COM2 raise DTR", || peer_dtr(a) == Some(true));
        a.dtr.store(false, Ordering::Relaxed);
        eventually("COM2 sees COM1 drop DTR", || peer_dtr(b) == Some(false));
        a.dtr.store(true, Ordering::Relaxed);
        eventually("COM2 sees COM1 raise DTR", || peer_dtr(b) == Some(true));

        // Stopping the pair releases both ports from the wiring.
        drop(hub);
        assert_eq!((peer_dtr(a), peer_dtr(b)), (None, None));
    }

    /// When SerialLoom loses one end, the other sees DTR drop, like an
    /// unplugged cable; once it is back, the wiring is restored.
    #[test]
    fn losing_one_end_drops_its_dtr() {
        let wires: Vec<Arc<Wire>> = (0..2).map(|_| Arc::new(Wire::default())).collect();
        let (a, b) = (&wires[0], &wires[1]);
        a.dtr.store(true, Ordering::Relaxed);
        b.dtr.store(true, Ordering::Relaxed);
        let _hub = start_wired(HubKind::Bridge, virt("COM1"), vec![virt("COM2")], &wires);
        eventually("wired", || peer_dtr(a) == Some(true) && peer_dtr(b) == Some(true));

        a.broken.store(true, Ordering::Relaxed);
        eventually("COM2 sees DTR drop", || peer_dtr(b) == Some(false));
        a.broken.store(false, Ordering::Relaxed);
        eventually("COM1 is back", || peer_dtr(a) == Some(true));
        eventually("COM2 sees DTR again", || peer_dtr(b) == Some(true));
    }

    /// Only two virtual ports bridged together are wired. Elsewhere the
    /// lines keep following SerialLoom's side, as before.
    #[test]
    fn other_hubs_are_not_wired() {
        let tcp = ep(EndpointSpec::TcpServer {
            bind: format!("127.0.0.1:{}", free_port()),
        });
        let cases = [
            (HubKind::Bridge, virt("COM1"), vec![tcp], 1),
            (HubKind::Split, virt("COM1"), vec![virt("COM2"), virt("COM3")], 3),
            (HubKind::Bus, virt("COM1"), vec![virt("COM2")], 2),
        ];
        for (kind, master, members, ports) in cases {
            let wires: Vec<Arc<Wire>> = (0..ports).map(|_| Arc::new(Wire::default())).collect();
            for w in &wires {
                w.dtr.store(true, Ordering::Relaxed);
            }
            let _hub = start_wired(kind, master, members, &wires);
            eventually("attached", || {
                wires.iter().all(|w| w.attached.load(Ordering::Relaxed))
            });
            std::thread::sleep(Duration::from_millis(100));
            for w in &wires {
                assert_eq!(peer_dtr(w), None, "{kind:?}: not opened as a pair");
            }
        }
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
