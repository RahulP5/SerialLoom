//! Serial IOCTL codes and output structures from `ntddser.h`.

const fn serial(function: u32) -> u32 {
    serialloom_proto::ctl_code(0x1b, function)
}

pub const IOCTL_SERIAL_SET_BAUD_RATE: u32 = serial(1);
pub const IOCTL_SERIAL_SET_QUEUE_SIZE: u32 = serial(2);
pub const IOCTL_SERIAL_SET_LINE_CONTROL: u32 = serial(3);
pub const IOCTL_SERIAL_SET_BREAK_ON: u32 = serial(4);
pub const IOCTL_SERIAL_SET_BREAK_OFF: u32 = serial(5);
pub const IOCTL_SERIAL_IMMEDIATE_CHAR: u32 = serial(6);
pub const IOCTL_SERIAL_SET_TIMEOUTS: u32 = serial(7);
pub const IOCTL_SERIAL_GET_TIMEOUTS: u32 = serial(8);
pub const IOCTL_SERIAL_SET_DTR: u32 = serial(9);
pub const IOCTL_SERIAL_CLR_DTR: u32 = serial(10);
pub const IOCTL_SERIAL_RESET_DEVICE: u32 = serial(11);
pub const IOCTL_SERIAL_SET_RTS: u32 = serial(12);
pub const IOCTL_SERIAL_CLR_RTS: u32 = serial(13);
pub const IOCTL_SERIAL_SET_XOFF: u32 = serial(14);
pub const IOCTL_SERIAL_SET_XON: u32 = serial(15);
pub const IOCTL_SERIAL_GET_WAIT_MASK: u32 = serial(16);
pub const IOCTL_SERIAL_SET_WAIT_MASK: u32 = serial(17);
pub const IOCTL_SERIAL_WAIT_ON_MASK: u32 = serial(18);
pub const IOCTL_SERIAL_PURGE: u32 = serial(19);
pub const IOCTL_SERIAL_GET_BAUD_RATE: u32 = serial(20);
pub const IOCTL_SERIAL_GET_LINE_CONTROL: u32 = serial(21);
pub const IOCTL_SERIAL_GET_CHARS: u32 = serial(22);
pub const IOCTL_SERIAL_SET_CHARS: u32 = serial(23);
pub const IOCTL_SERIAL_GET_HANDFLOW: u32 = serial(24);
pub const IOCTL_SERIAL_SET_HANDFLOW: u32 = serial(25);
pub const IOCTL_SERIAL_GET_MODEMSTATUS: u32 = serial(26);
pub const IOCTL_SERIAL_GET_COMMSTATUS: u32 = serial(27);
pub const IOCTL_SERIAL_XOFF_COUNTER: u32 = serial(28);
pub const IOCTL_SERIAL_GET_PROPERTIES: u32 = serial(29);
pub const IOCTL_SERIAL_GET_DTRRTS: u32 = serial(30);
pub const IOCTL_SERIAL_LSRMST_INSERT: u32 = serial(31);
pub const IOCTL_SERIAL_CONFIG_SIZE: u32 = serial(32);
pub const IOCTL_SERIAL_GET_STATS: u32 = serial(35);
pub const IOCTL_SERIAL_CLEAR_STATS: u32 = serial(36);
pub const IOCTL_SERIAL_GET_MODEM_CONTROL: u32 = serial(37);
pub const IOCTL_SERIAL_SET_MODEM_CONTROL: u32 = serial(38);
pub const IOCTL_SERIAL_SET_FIFO_CONTROL: u32 = serial(39);
pub const IOCTL_SERIAL_APPLY_DEFAULT_CONFIGURATION: u32 = serial(40);

struct Writer<const N: usize> {
    buf: [u8; N],
    at: usize,
}

impl<const N: usize> Writer<N> {
    fn new() -> Self {
        Self { buf: [0; N], at: 0 }
    }
    fn u16(mut self, v: u16) -> Self {
        self.buf[self.at..self.at + 2].copy_from_slice(&v.to_le_bytes());
        self.at += 2;
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.buf[self.at..self.at + 4].copy_from_slice(&v.to_le_bytes());
        self.at += 4;
        self
    }
    fn done(self) -> [u8; N] {
        self.buf
    }
}

/// `SERIAL_STATUS` (20 bytes with padding).
pub fn comm_status(in_queue: u32) -> [u8; 20] {
    Writer::<20>::new()
        .u32(0) // Errors
        .u32(0) // HoldReasons
        .u32(in_queue) // AmountInInQueue
        .u32(0) // AmountInOutQueue
        .done() // EofReceived, WaitForImmediate = 0
}

/// `SERIALPERF_STATS`.
pub fn perf_stats(received: u64, transmitted: u64) -> [u8; 24] {
    Writer::<24>::new()
        .u32(received as u32)
        .u32(transmitted as u32)
        .done()
}

/// `SERIAL_COMMPROP` (64 bytes) describing a port that accepts any setting.
pub fn comm_properties(rx_queue: u32) -> [u8; 64] {
    Writer::<64>::new()
        .u16(64) // PacketLength
        .u16(2) // PacketVersion
        .u32(0x0000_0001) // ServiceMask: SERIAL_SP_SERIALCOMM
        .u32(0) // Reserved1
        .u32(0) // MaxTxQueue (no limit)
        .u32(0) // MaxRxQueue
        .u32(0x1000_0000) // MaxBaud: SERIAL_BAUD_USER
        .u32(0x0000_0001) // ProvSubType: SERIAL_SP_RS232
        .u32(0x0000_00FF) // ProvCapabilities: DTR/DSR, RTS/CTS, RLSD, parity, XON/XOFF, timeouts
        .u32(0x0000_007F) // SettableParams
        .u32(0x1007_FFFF) // SettableBaud: all standard rates + user
        .u16(0x000F) // SettableData: 5-8 bits
        .u16(0x1F07) // SettableStopParity
        .u32(0) // CurrentTxQueue
        .u32(rx_queue) // CurrentRxQueue
        .u32(0) // ProvSpec1
        .u32(0) // ProvSpec2
        .done()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_ntddser() {
        assert_eq!(IOCTL_SERIAL_SET_BAUD_RATE, 0x001B_0004);
        assert_eq!(IOCTL_SERIAL_WAIT_ON_MASK, 0x001B_0048);
        assert_eq!(IOCTL_SERIAL_GET_PROPERTIES, 0x001B_0074);
    }

    #[test]
    fn commprop_header() {
        let p = comm_properties(1024);
        assert_eq!(u16::from_le_bytes([p[0], p[1]]), 64);
        assert_eq!(u32::from_le_bytes([p[48], p[49], p[50], p[51]]), 1024);
    }
}
