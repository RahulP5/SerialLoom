//! A USB CDC-ACM ("USB serial") device, implemented in software.
//!
//! Windows binds its inbox, Microsoft-signed `usbser.sys` to this device and
//! creates a COM port for it. The device is composite (IAD), which makes
//! Windows load `usbccgp.sys` and match the CDC function by its standard
//! compatible ID `USB\Class_02&SubClass_02&Prot_01`.
//!
//! This module is pure logic: it turns control requests into replies and
//! keeps the line settings. Data flow and timing live in `server.rs`.

/// pid.codes open-source vendor ID.
pub const VENDOR_ID: u16 = 0x1209;
/// pid.codes test PID. Replace with a PID allocated to SerialLoom before release.
pub const PRODUCT_ID: u16 = 0x0001;
pub const BCD_DEVICE: u16 = 0x0100;

/// Endpoint numbers (without the direction bit).
pub const EP_NOTIFY: u32 = 3; // interrupt IN 0x83
pub const EP_DATA_OUT: u32 = 1; // bulk OUT 0x01
pub const EP_DATA_IN: u32 = 2; // bulk IN 0x82

const MAX_PACKET: u8 = 64;

pub fn device_descriptor() -> [u8; 18] {
    let [vl, vh] = VENDOR_ID.to_le_bytes();
    let [pl, ph] = PRODUCT_ID.to_le_bytes();
    let [bl, bh] = BCD_DEVICE.to_le_bytes();
    [
        18, 0x01, // bLength, DEVICE
        0x00, 0x02, // bcdUSB 2.00
        0xEF, 0x02, 0x01, // Miscellaneous / Common / IAD
        MAX_PACKET, vl, vh, pl, ph, bl, bh, //
        1, 2, 3, // iManufacturer, iProduct, iSerialNumber
        1, // bNumConfigurations
    ]
}

pub fn config_descriptor() -> Vec<u8> {
    let mut d = vec![
        // Configuration header; wTotalLength patched below.
        9, 0x02, 0, 0, 2, 1, 0, 0x80, 50,
        // Interface association: interfaces 0-1 form one CDC ACM function.
        8, 0x0B, 0, 2, 0x02, 0x02, 0x01, 0,
        // Interface 0: CDC communication, ACM, AT commands, 1 endpoint.
        9, 0x04, 0, 0, 1, 0x02, 0x02, 0x01, 0, 5, 0x24, 0x00, 0x10,
        0x01, // Header functional descriptor, CDC 1.10
        5, 0x24, 0x01, 0x00, 0x01, // Call management: data interface 1
        4, 0x24, 0x02, 0x02, // ACM: line coding + control line state
        5, 0x24, 0x06, 0x00, 0x01, // Union: master 0, slave 1
        7, 0x05, 0x83, 0x03, 16, 0, 16, // Interrupt IN 0x83, 16 bytes, 16 ms
        // Interface 1: CDC data, 2 endpoints.
        9, 0x04, 1, 0, 2, 0x0A, 0x00, 0x00, 0, 7, 0x05, 0x01, 0x02, MAX_PACKET, 0, 0, // Bulk OUT 0x01
        7, 0x05, 0x82, 0x02, MAX_PACKET, 0, 0, // Bulk IN 0x82
    ];
    let total = (d.len() as u16).to_le_bytes();
    d[2] = total[0];
    d[3] = total[1];
    d
}

fn string_descriptor(s: &str) -> Vec<u8> {
    let mut d = vec![0u8, 0x03];
    for unit in s.encode_utf16() {
        d.extend_from_slice(&unit.to_le_bytes());
    }
    d[0] = d.len() as u8;
    d
}

/// CDC `LINE_CODING`: baud rate, stop bits, parity, data bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCoding {
    pub baud_rate: u32,
    /// 0 = 1, 1 = 1.5, 2 = 2 stop bits.
    pub stop_bits: u8,
    /// 0 none, 1 odd, 2 even, 3 mark, 4 space.
    pub parity: u8,
    pub data_bits: u8,
}

impl Default for LineCoding {
    fn default() -> Self {
        Self {
            baud_rate: 9600,
            stop_bits: 0,
            parity: 0,
            data_bits: 8,
        }
    }
}

impl LineCoding {
    fn to_bytes(self) -> [u8; 7] {
        let b = self.baud_rate.to_le_bytes();
        [
            b[0],
            b[1],
            b[2],
            b[3],
            self.stop_bits,
            self.parity,
            self.data_bits,
        ]
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        (b.len() >= 7).then(|| Self {
            baud_rate: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            stop_bits: b[4],
            parity: b[5],
            data_bits: b[6],
        })
    }
}

/// Result of a control transfer on endpoint 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlReply {
    /// Success; IN data (empty for OUT requests).
    Data(Vec<u8>),
    /// Request not supported: the host sees a STALL.
    Stall,
}

/// Settings and line state of one virtual serial device.
#[derive(Debug, Clone)]
pub struct CdcState {
    pub serial_number: String,
    pub line: LineCoding,
    pub dtr: bool,
    pub rts: bool,
    pub configured: bool,
}

impl CdcState {
    pub fn new(serial_number: impl Into<String>) -> Self {
        Self {
            serial_number: serial_number.into(),
            line: LineCoding::default(),
            dtr: false,
            rts: false,
            configured: false,
        }
    }

    /// True when the program on the COM side has the port open (DTR raised).
    pub fn port_open(&self) -> bool {
        self.dtr
    }

    /// Handles a SETUP packet plus its OUT data stage.
    pub fn control(&mut self, setup: &[u8; 8], out_data: &[u8]) -> ControlReply {
        let request_type = setup[0];
        let request = setup[1];
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        let reply = |mut v: Vec<u8>| {
            v.truncate(length);
            ControlReply::Data(v)
        };

        match (request_type, request) {
            // GET_DESCRIPTOR
            (0x80, 0x06) => {
                let [index, kind] = value.to_le_bytes();
                match kind {
                    0x01 => reply(device_descriptor().to_vec()),
                    0x02 => reply(config_descriptor()),
                    0x03 => match index {
                        0 => reply(vec![4, 0x03, 0x09, 0x04]), // English (US)
                        1 => reply(string_descriptor("SerialLoom")),
                        2 => reply(string_descriptor("SerialLoom Virtual COM Port")),
                        3 => reply(string_descriptor(&self.serial_number)),
                        _ => ControlReply::Stall,
                    },
                    // Device qualifier, BOS, OS descriptors: not a high-speed device.
                    _ => ControlReply::Stall,
                }
            }
            // GET_STATUS (device, interface, endpoint)
            (0x80..=0x82, 0x00) => reply(vec![0, 0]),
            // GET_CONFIGURATION
            (0x80, 0x08) => reply(vec![u8::from(self.configured)]),
            // SET_CONFIGURATION
            (0x00, 0x09) => {
                self.configured = value != 0;
                ControlReply::Data(Vec::new())
            }
            // SET_ADDRESS, SET/CLEAR_FEATURE, SET_INTERFACE: accept.
            (0x00, 0x05) | (0x00..=0x02, 0x01) | (0x00..=0x02, 0x03) | (0x01, 0x0B) => {
                ControlReply::Data(Vec::new())
            }
            // GET_INTERFACE
            (0x81, 0x0A) => reply(vec![0]),
            // CDC SET_LINE_CODING
            (0x21, 0x20) => match LineCoding::from_bytes(out_data) {
                Some(l) => {
                    self.line = l;
                    ControlReply::Data(Vec::new())
                }
                None => ControlReply::Stall,
            },
            // CDC GET_LINE_CODING
            (0xA1, 0x21) => reply(self.line.to_bytes().to_vec()),
            // CDC SET_CONTROL_LINE_STATE
            (0x21, 0x22) => {
                self.dtr = value & 0x1 != 0;
                self.rts = value & 0x2 != 0;
                ControlReply::Data(Vec::new())
            }
            // CDC SEND_BREAK
            (0x21, 0x23) => ControlReply::Data(Vec::new()),
            _ => ControlReply::Stall,
        }
    }
}

/// CDC SERIAL_STATE notification telling the host DCD and DSR are up.
pub fn serial_state_notification(dcd: bool, dsr: bool) -> Vec<u8> {
    let bits = u16::from(dcd) | (u16::from(dsr) << 1);
    let b = bits.to_le_bytes();
    vec![0xA1, 0x20, 0, 0, 0, 0, 2, 0, b[0], b[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(rt: u8, req: u8, value: u16, index: u16, len: u16) -> [u8; 8] {
        let v = value.to_le_bytes();
        let i = index.to_le_bytes();
        let l = len.to_le_bytes();
        [rt, req, v[0], v[1], i[0], i[1], l[0], l[1]]
    }

    #[test]
    fn descriptors_are_consistent() {
        let cfg = config_descriptor();
        assert_eq!(cfg.len(), 75);
        assert_eq!(u16::from_le_bytes([cfg[2], cfg[3]]) as usize, cfg.len());
        // Walk the descriptor chain: every bLength must land on the next one.
        let mut at = 0;
        while at < cfg.len() {
            assert!(cfg[at] > 0);
            at += cfg[at] as usize;
        }
        assert_eq!(at, cfg.len());
        assert_eq!(device_descriptor()[4], 0xEF);
    }

    #[test]
    fn answers_enumeration_requests() {
        let mut s = CdcState::new("SL0001");
        match s.control(&setup(0x80, 6, 0x0100, 0, 64), &[]) {
            ControlReply::Data(d) => assert_eq!(d.len(), 18),
            other => panic!("{other:?}"),
        }
        // Host asks for the first 9 bytes of the configuration first.
        match s.control(&setup(0x80, 6, 0x0200, 0, 9), &[]) {
            ControlReply::Data(d) => assert_eq!(d.len(), 9),
            other => panic!("{other:?}"),
        }
        match s.control(&setup(0x80, 6, 0x0303, 0x0409, 255), &[]) {
            ControlReply::Data(d) => assert_eq!(d[0] as usize, d.len()),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            s.control(&setup(0x80, 6, 0x0600, 0, 10), &[]),
            ControlReply::Stall
        );
        s.control(&setup(0x00, 9, 1, 0, 0), &[]);
        assert!(s.configured);
    }

    #[test]
    fn line_coding_round_trip() {
        let mut s = CdcState::new("x");
        let coding = [0x00, 0xC2, 0x01, 0x00, 0, 0, 8]; // 115200 8N1
        s.control(&setup(0x21, 0x20, 0, 0, 7), &coding);
        assert_eq!(s.line.baud_rate, 115_200);
        assert_eq!(
            s.control(&setup(0xA1, 0x21, 0, 0, 7), &[]),
            ControlReply::Data(coding.to_vec())
        );
    }

    #[test]
    fn control_lines() {
        let mut s = CdcState::new("x");
        s.control(&setup(0x21, 0x22, 0x3, 0, 0), &[]);
        assert!(s.dtr && s.rts && s.port_open());
        s.control(&setup(0x21, 0x22, 0, 0, 0), &[]);
        assert!(!s.port_open());
    }

    #[test]
    fn notification_layout() {
        let n = serial_state_notification(true, true);
        assert_eq!(n.len(), 10);
        assert_eq!(n[8], 0x03);
    }
}
