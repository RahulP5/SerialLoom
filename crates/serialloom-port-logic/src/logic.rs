//! Serial port behaviour with no WDF calls, so it can be unit tested.
//!
//! The WDF glue in `lib.rs` keeps one [`PortState`] per device behind a mutex
//! and asks it what to do with each request.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serialloom_proto as proto;

/// Bytes buffered in each direction before new data is dropped.
pub const BUFFER_SIZE: usize = 64 * 1024;

// Wait-mask events (SERIAL_EV_*).
pub const EV_RXCHAR: u32 = 0x0001;
pub const EV_RXFLAG: u32 = 0x0002;
pub const EV_TXEMPTY: u32 = 0x0004;
pub const EV_CTS: u32 = 0x0008;
pub const EV_DSR: u32 = 0x0010;
pub const EV_RLSD: u32 = 0x0020;
pub const EV_RING: u32 = 0x0100;

// IOCTL_SERIAL_PURGE flags.
pub const PURGE_RXABORT: u32 = 0x2;
pub const PURGE_TXCLEAR: u32 = 0x4;
pub const PURGE_RXCLEAR: u32 = 0x8;

#[derive(Debug, Default)]
pub struct Ring {
    data: VecDeque<u8>,
}

impl Ring {
    /// Appends as much as fits and returns the number of bytes accepted.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let room = BUFFER_SIZE.saturating_sub(self.data.len());
        let n = room.min(bytes.len());
        self.data.extend(&bytes[..n]);
        n
    }

    pub fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.data.len());
        for (slot, byte) in out.iter_mut().zip(self.data.drain(..n)) {
            *slot = byte;
        }
        n
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

/// `SERIAL_TIMEOUTS`, all in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timeouts {
    pub read_interval: u32,
    pub read_multiplier: u32,
    pub read_constant: u32,
    pub write_multiplier: u32,
    pub write_constant: u32,
}

impl Timeouts {
    pub fn from_bytes(b: &[u8; 20]) -> Self {
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Self {
            read_interval: u(0),
            read_multiplier: u(4),
            read_constant: u(8),
            write_multiplier: u(12),
            write_constant: u(16),
        }
    }

    pub fn to_bytes(self) -> [u8; 20] {
        let mut out = [0u8; 20];
        for (i, v) in [
            self.read_interval,
            self.read_multiplier,
            self.read_constant,
            self.write_multiplier,
            self.write_constant,
        ]
        .into_iter()
        .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// The one combination serial.sys rejects.
    pub fn is_valid(&self) -> bool {
        !(self.read_interval == u32::MAX
            && self.read_multiplier == u32::MAX
            && self.read_constant == u32::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDecision {
    /// Keep the request pending.
    Wait,
    /// Complete with `bytes` bytes. `timed_out` selects STATUS_TIMEOUT.
    Complete { bytes: usize, timed_out: bool },
}

/// Applies Win32 `COMMTIMEOUTS` rules to the read at the head of the queue.
///
/// Data is copied only when the read completes, so `available` is everything
/// buffered for the program right now.
pub fn decide_read(
    t: &Timeouts,
    wanted: usize,
    available: usize,
    started: Instant,
    last_rx: Option<Instant>,
    now: Instant,
) -> ReadDecision {
    const MAX: u32 = u32::MAX;
    let done = |bytes: usize, timed_out: bool| ReadDecision::Complete {
        bytes: bytes.min(wanted),
        timed_out,
    };

    if wanted == 0 || available >= wanted {
        return done(wanted, false);
    }

    // Return immediately with whatever is there.
    if t.read_interval == MAX && t.read_multiplier == 0 && t.read_constant == 0 {
        return done(available, false);
    }

    // Return as soon as anything is there, or after read_constant ms.
    if t.read_interval == MAX && t.read_multiplier == MAX && t.read_constant > 0 && t.read_constant < MAX {
        if available > 0 {
            return done(available, false);
        }
        if now.duration_since(started) >= ms(t.read_constant) {
            return done(0, true);
        }
        return ReadDecision::Wait;
    }

    if t.read_multiplier != 0 || t.read_constant != 0 {
        let per_byte = u64::from(if t.read_multiplier == MAX {
            0
        } else {
            t.read_multiplier
        });
        let total = per_byte
            .saturating_mul(wanted as u64)
            .saturating_add(u64::from(t.read_constant));
        if now.duration_since(started) >= Duration::from_millis(total) {
            return done(available, true);
        }
    }

    if t.read_interval != 0 && t.read_interval != MAX && available > 0 {
        let since = last_rx.map_or(started, |l| l.max(started));
        if now.duration_since(since) >= ms(t.read_interval) {
            return done(available, true);
        }
    }

    ReadDecision::Wait
}

fn ms(v: u32) -> Duration {
    Duration::from_millis(u64::from(v))
}

/// Everything the driver remembers about one virtual port.
#[derive(Debug)]
pub struct PortState {
    /// Bytes waiting for the program on the COM side.
    pub to_com: Ring,
    /// Bytes the program wrote, waiting for SerialLoom.
    pub from_com: Ring,
    pub baud_rate: u32,
    /// SERIAL_LINE_CONTROL: stop bits, parity, word length.
    pub line_control: [u8; 3],
    /// SERIAL_CHARS: eof, error, break, event, xon, xoff.
    pub chars: [u8; 6],
    /// SERIAL_HANDFLOW, stored as raw bytes.
    pub handflow: [u8; 16],
    pub timeouts: Timeouts,
    pub dtr: bool,
    pub rts: bool,
    pub wait_mask: u32,
    /// Events seen since the last wait completed, already filtered by the mask.
    pub events: u32,
    pub com_open: bool,
    pub control_open: bool,
    /// `None` means "follow the control connection".
    pub modem_override: Option<u32>,
    pub last_rx: Option<Instant>,
    pub to_com_total: u64,
    pub from_com_total: u64,
}

impl Default for PortState {
    fn default() -> Self {
        Self {
            to_com: Ring::default(),
            from_com: Ring::default(),
            baud_rate: 9600,
            line_control: [0, 0, 8],
            chars: [0, 0, 0, 0, 0x11, 0x13],
            handflow: [0; 16],
            timeouts: Timeouts::default(),
            dtr: false,
            rts: false,
            wait_mask: 0,
            events: 0,
            com_open: false,
            control_open: false,
            modem_override: None,
            last_rx: None,
            to_com_total: 0,
            from_com_total: 0,
        }
    }
}

impl PortState {
    /// SERIAL_*_STATE bits the program sees from GetCommModemStatus.
    pub fn modem_status(&self) -> u32 {
        match self.modem_override {
            Some(bits) => bits,
            None if self.control_open => proto::MODEM_CTS | proto::MODEM_DSR | proto::MODEM_DCD,
            None => 0,
        }
    }

    pub fn raise(&mut self, events: u32) {
        self.events |= events & self.wait_mask;
    }

    /// Applies a change of modem lines and raises the matching events.
    pub fn set_modem(&mut self, f: impl FnOnce(&mut Self)) {
        let before = self.modem_status();
        f(self);
        let changed = before ^ self.modem_status();
        let mut ev = 0;
        if changed & proto::MODEM_CTS != 0 {
            ev |= EV_CTS;
        }
        if changed & proto::MODEM_DSR != 0 {
            ev |= EV_DSR;
        }
        if changed & proto::MODEM_DCD != 0 {
            ev |= EV_RLSD;
        }
        if changed & proto::MODEM_RI != 0 {
            ev |= EV_RING;
        }
        self.raise(ev);
    }

    /// Program wrote bytes. Data is discarded while SerialLoom is not
    /// attached, like a serial cable with nothing on the other end.
    pub fn com_write(&mut self, bytes: &[u8]) {
        if self.control_open {
            self.from_com.push(bytes);
        }
        self.from_com_total += bytes.len() as u64;
        self.raise(EV_TXEMPTY);
    }

    /// SerialLoom wrote bytes for the program. Returns how many were accepted.
    pub fn control_write(&mut self, bytes: &[u8], now: Instant) -> usize {
        if !self.com_open {
            // Nobody listening: accept and drop so the router does not stall.
            return bytes.len();
        }
        let n = self.to_com.push(bytes);
        if n > 0 {
            self.last_rx = Some(now);
            let mut ev = EV_RXCHAR;
            let event_char = self.chars[3];
            if event_char != 0 && bytes[..n].contains(&event_char) {
                ev |= EV_RXFLAG;
            }
            self.raise(ev);
        }
        n
    }

    pub fn open_com(&mut self) {
        self.com_open = true;
        self.to_com.clear();
        self.from_com.clear();
        self.wait_mask = 0;
        self.events = 0;
        self.timeouts = Timeouts::default();
    }

    pub fn close_com(&mut self) {
        self.com_open = false;
        self.dtr = false;
        self.rts = false;
        self.wait_mask = 0;
        self.events = 0;
        self.to_com.clear();
    }

    pub fn open_control(&mut self) {
        self.from_com.clear();
        self.set_modem(|s| s.control_open = true);
    }

    pub fn close_control(&mut self) {
        self.from_com.clear();
        self.set_modem(|s| {
            s.control_open = false;
            s.modem_override = None;
        });
    }

    pub fn purge(&mut self, flags: u32) {
        if flags & PURGE_RXCLEAR != 0 {
            self.to_com.clear();
        }
        if flags & PURGE_TXCLEAR != 0 {
            self.from_com.clear();
        }
    }

    pub fn snapshot(&self) -> proto::PortState {
        proto::PortState {
            version: proto::PORT_STATE_VERSION,
            com_open: u32::from(self.com_open),
            baud_rate: self.baud_rate,
            stop_bits: self.line_control[0],
            parity: self.line_control[1],
            data_bits: self.line_control[2],
            _reserved: 0,
            dtr_rts: u32::from(self.dtr) | (u32::from(self.rts) << 1),
            to_com_queued: self.to_com.len() as u32,
            from_com_queued: self.from_com.len() as u32,
            from_com_total: self.from_com_total,
            to_com_total: self.to_com_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ri: u32, rm: u32, rc: u32) -> Timeouts {
        Timeouts {
            read_interval: ri,
            read_multiplier: rm,
            read_constant: rc,
            ..Timeouts::default()
        }
    }

    #[test]
    fn ring_caps_and_drains() {
        let mut r = Ring::default();
        assert_eq!(r.push(&vec![1; BUFFER_SIZE + 10]), BUFFER_SIZE);
        let mut out = [0u8; 4];
        assert_eq!(r.pop_into(&mut out), 4);
        assert_eq!(r.len(), BUFFER_SIZE - 4);
    }

    #[test]
    fn full_read_completes() {
        let now = Instant::now();
        let d = decide_read(&t(0, 0, 0), 4, 10, now, None, now);
        assert_eq!(
            d,
            ReadDecision::Complete {
                bytes: 4,
                timed_out: false
            }
        );
    }

    #[test]
    fn no_timeouts_waits_for_all_bytes() {
        let now = Instant::now();
        assert_eq!(
            decide_read(&t(0, 0, 0), 4, 2, now, Some(now), now + ms(5000)),
            ReadDecision::Wait
        );
    }

    #[test]
    fn immediate_mode() {
        let now = Instant::now();
        let d = decide_read(&t(u32::MAX, 0, 0), 64, 0, now, None, now);
        assert_eq!(
            d,
            ReadDecision::Complete {
                bytes: 0,
                timed_out: false
            }
        );
    }

    #[test]
    fn wait_for_any_mode() {
        let start = Instant::now();
        let tm = t(u32::MAX, u32::MAX, 100);
        assert_eq!(
            decide_read(&tm, 64, 0, start, None, start + ms(50)),
            ReadDecision::Wait
        );
        assert_eq!(
            decide_read(&tm, 64, 3, start, None, start + ms(50)),
            ReadDecision::Complete {
                bytes: 3,
                timed_out: false
            }
        );
        assert_eq!(
            decide_read(&tm, 64, 0, start, None, start + ms(100)),
            ReadDecision::Complete {
                bytes: 0,
                timed_out: true
            }
        );
    }

    #[test]
    fn total_timeout() {
        let start = Instant::now();
        let tm = t(0, 10, 50); // 10 ms per byte + 50 ms
        assert_eq!(
            decide_read(&tm, 5, 2, start, Some(start), start + ms(90)),
            ReadDecision::Wait
        );
        assert_eq!(
            decide_read(&tm, 5, 2, start, Some(start), start + ms(100)),
            ReadDecision::Complete {
                bytes: 2,
                timed_out: true
            }
        );
    }

    #[test]
    fn interval_timeout_after_first_byte() {
        let start = Instant::now();
        let tm = t(20, 0, 0);
        // No bytes yet: interval timer has not started.
        assert_eq!(
            decide_read(&tm, 8, 0, start, None, start + ms(500)),
            ReadDecision::Wait
        );
        let last = start + ms(10);
        assert_eq!(
            decide_read(&tm, 8, 3, start, Some(last), start + ms(25)),
            ReadDecision::Wait
        );
        assert_eq!(
            decide_read(&tm, 8, 3, start, Some(last), start + ms(30)),
            ReadDecision::Complete {
                bytes: 3,
                timed_out: true
            }
        );
    }

    #[test]
    fn modem_lines_follow_control_side() {
        let mut s = PortState::default();
        s.open_com();
        s.wait_mask = EV_CTS | EV_DSR;
        assert_eq!(s.modem_status(), 0);
        s.open_control();
        assert_eq!(s.modem_status() & proto::MODEM_CTS, proto::MODEM_CTS);
        assert_eq!(s.events, EV_CTS | EV_DSR);
    }

    #[test]
    fn data_flows_both_ways() {
        let mut s = PortState::default();
        let now = Instant::now();
        s.open_com();
        s.open_control();
        s.wait_mask = EV_RXCHAR;
        assert_eq!(s.control_write(b"hi", now), 2);
        assert_eq!(s.events, EV_RXCHAR);
        s.com_write(b"yo");
        let mut out = [0u8; 2];
        assert_eq!(s.from_com.pop_into(&mut out), 2);
        assert_eq!(&out, b"yo");
    }

    #[test]
    fn writes_without_listener_are_dropped() {
        let mut s = PortState::default();
        s.com_write(b"lost");
        assert!(s.from_com.is_empty());
        assert_eq!(s.control_write(b"lost", Instant::now()), 4);
        assert!(s.to_com.is_empty());
    }

    #[test]
    fn timeouts_roundtrip() {
        let tm = Timeouts {
            read_interval: 1,
            read_multiplier: 2,
            read_constant: 3,
            write_multiplier: 4,
            write_constant: 5,
        };
        assert_eq!(Timeouts::from_bytes(&tm.to_bytes()), tm);
        assert!(!t(u32::MAX, u32::MAX, u32::MAX).is_valid());
    }
}
