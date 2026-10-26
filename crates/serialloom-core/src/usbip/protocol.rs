//! USB/IP wire format (all integers big-endian).
//!
//! Reference: Linux `Documentation/usb/usbip_protocol.rst`. Only what a
//! USB/IP *server* exporting non-isochronous devices needs is implemented.

use std::io::{self, Read, Write};

pub const USBIP_VERSION: u16 = 0x0111;

pub const OP_REQ_DEVLIST: u16 = 0x8005;
pub const OP_REP_DEVLIST: u16 = 0x0005;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;

pub const CMD_SUBMIT: u32 = 0x0000_0001;
pub const CMD_UNLINK: u32 = 0x0000_0002;
pub const RET_SUBMIT: u32 = 0x0000_0003;
pub const RET_UNLINK: u32 = 0x0000_0004;

pub const DIR_OUT: u32 = 0;
pub const DIR_IN: u32 = 1;

/// Linux errno values used in URB status fields.
pub const ECONNRESET: i32 = -104;
pub const EPIPE: i32 = -32;

/// `enum usb_device_speed` value for full speed.
pub const SPEED_FULL: u32 = 2;

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_i32(r: &mut impl Read) -> io::Result<i32> {
    read_u32(r).map(|v| v as i32)
}

/// Fixed-size, NUL-padded string field.
fn put_str(out: &mut Vec<u8>, s: &str, len: usize) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(len - 1);
    out.extend_from_slice(&bytes[..n]);
    out.resize(out.len() + (len - n), 0);
}

/// Identity of an exported device, as sent in OP_REP_DEVLIST / OP_REP_IMPORT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub path: String,
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configuration_value: u8,
    pub num_configurations: u8,
    /// (class, subclass, protocol) per interface.
    pub interfaces: Vec<(u8, u8, u8)>,
}

impl DeviceInfo {
    /// `struct usbip_usb_device` (312 bytes).
    pub fn encode(&self, out: &mut Vec<u8>) {
        put_str(out, &self.path, 256);
        put_str(out, &self.busid, 32);
        out.extend_from_slice(&self.busnum.to_be_bytes());
        out.extend_from_slice(&self.devnum.to_be_bytes());
        out.extend_from_slice(&self.speed.to_be_bytes());
        out.extend_from_slice(&self.vendor.to_be_bytes());
        out.extend_from_slice(&self.product.to_be_bytes());
        out.extend_from_slice(&self.bcd_device.to_be_bytes());
        out.extend_from_slice(&[
            self.class,
            self.subclass,
            self.protocol,
            self.configuration_value,
            self.num_configurations,
            self.interfaces.len() as u8,
        ]);
    }

    pub fn devid(&self) -> u32 {
        (self.busnum << 16) | self.devnum
    }
}

pub fn encode_op_header(out: &mut Vec<u8>, code: u16, status: u32) {
    out.extend_from_slice(&USBIP_VERSION.to_be_bytes());
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(&status.to_be_bytes());
}

pub fn encode_devlist(devices: &[DeviceInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_op_header(&mut out, OP_REP_DEVLIST, 0);
    out.extend_from_slice(&(devices.len() as u32).to_be_bytes());
    for d in devices {
        d.encode(&mut out);
        for &(c, s, p) in &d.interfaces {
            out.extend_from_slice(&[c, s, p, 0]);
        }
    }
    out
}

pub fn encode_import(device: Option<&DeviceInfo>) -> Vec<u8> {
    let mut out = Vec::new();
    match device {
        Some(d) => {
            encode_op_header(&mut out, OP_REP_IMPORT, 0);
            d.encode(&mut out);
        }
        None => encode_op_header(&mut out, OP_REP_IMPORT, 1),
    }
    out
}

/// First request on a new connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpRequest {
    DevList,
    Import { busid: String },
}

pub fn read_op_request(r: &mut impl Read) -> io::Result<OpRequest> {
    let _version = read_u16(r)?;
    let code = read_u16(r)?;
    let _status = read_u32(r)?;
    match code {
        OP_REQ_DEVLIST => Ok(OpRequest::DevList),
        OP_REQ_IMPORT => {
            let mut busid = [0u8; 32];
            r.read_exact(&mut busid)?;
            let end = busid.iter().position(|&b| b == 0).unwrap_or(32);
            Ok(OpRequest::Import {
                busid: String::from_utf8_lossy(&busid[..end]).into_owned(),
            })
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported USB/IP operation {other:#06x}"),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submit {
    pub seqnum: u32,
    pub devid: u32,
    pub direction: u32,
    pub ep: u32,
    pub transfer_flags: u32,
    pub length: u32,
    pub setup: [u8; 8],
    /// OUT payload (empty for IN transfers).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Submit(Submit),
    Unlink { seqnum: u32, victim: u32 },
}

/// Largest transfer we accept, to bound memory use from a bad client.
const MAX_TRANSFER: u32 = 1 << 20;

pub fn read_command(r: &mut impl Read) -> io::Result<Command> {
    let command = read_u32(r)?;
    let seqnum = read_u32(r)?;
    let devid = read_u32(r)?;
    let direction = read_u32(r)?;
    let ep = read_u32(r)?;
    match command {
        CMD_SUBMIT => {
            let transfer_flags = read_u32(r)?;
            let length = read_i32(r)?.max(0) as u32;
            let _start_frame = read_i32(r)?;
            let packets = read_i32(r)?;
            let _interval = read_i32(r)?;
            let mut setup = [0u8; 8];
            r.read_exact(&mut setup)?;
            if length > MAX_TRANSFER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "transfer too large"));
            }
            let mut data = Vec::new();
            if direction == DIR_OUT && length > 0 {
                data.resize(length as usize, 0);
                r.read_exact(&mut data)?;
            }
            if packets > 0 && packets != -1 {
                // Isochronous descriptors: not used by our devices; skip them.
                let mut skip = vec![0u8; packets as usize * 16];
                r.read_exact(&mut skip)?;
            }
            Ok(Command::Submit(Submit {
                seqnum,
                devid,
                direction,
                ep,
                transfer_flags,
                length,
                setup,
                data,
            }))
        }
        CMD_UNLINK => {
            let victim = read_u32(r)?;
            let mut pad = [0u8; 24];
            r.read_exact(&mut pad)?;
            Ok(Command::Unlink { seqnum, victim })
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported URB command {other:#x}"),
        )),
    }
}

fn header(out: &mut Vec<u8>, command: u32, seqnum: u32) {
    out.extend_from_slice(&command.to_be_bytes());
    out.extend_from_slice(&seqnum.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // devid
    out.extend_from_slice(&0u32.to_be_bytes()); // direction
    out.extend_from_slice(&0u32.to_be_bytes()); // ep
}

/// `USBIP_RET_SUBMIT`. `data` is only sent for IN transfers.
pub fn encode_ret_submit(seqnum: u32, status: i32, data: &[u8], actual_length: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + data.len());
    header(&mut out, RET_SUBMIT, seqnum);
    out.extend_from_slice(&status.to_be_bytes());
    out.extend_from_slice(&(actual_length as i32).to_be_bytes());
    out.extend_from_slice(&0i32.to_be_bytes()); // start_frame
    out.extend_from_slice(&0i32.to_be_bytes()); // number_of_packets
    out.extend_from_slice(&0i32.to_be_bytes()); // error_count
    out.extend_from_slice(&[0u8; 8]); // padding
    out.extend_from_slice(data);
    out
}

/// `USBIP_RET_UNLINK`.
pub fn encode_ret_unlink(seqnum: u32, status: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    header(&mut out, RET_UNLINK, seqnum);
    out.extend_from_slice(&status.to_be_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out
}

pub fn write_all(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceInfo {
        DeviceInfo {
            path: "/sys/devices/serialloom/1-1".into(),
            busid: "1-1".into(),
            busnum: 1,
            devnum: 1,
            speed: SPEED_FULL,
            vendor: 0x1209,
            product: 0x0001,
            bcd_device: 0x0100,
            class: 0xEF,
            subclass: 2,
            protocol: 1,
            configuration_value: 1,
            num_configurations: 1,
            interfaces: vec![(2, 2, 1), (0x0A, 0, 0)],
        }
    }

    #[test]
    fn device_record_is_312_bytes() {
        let mut out = Vec::new();
        device().encode(&mut out);
        assert_eq!(out.len(), 312);
        assert_eq!(&out[256..259], b"1-1");
    }

    #[test]
    fn import_reply_layout() {
        let rep = encode_import(Some(&device()));
        assert_eq!(rep.len(), 8 + 312);
        assert_eq!(&rep[0..4], &[0x01, 0x11, 0x00, 0x03]);
        let fail = encode_import(None);
        assert_eq!(&fail[4..8], &1u32.to_be_bytes());
    }

    #[test]
    fn parses_import_request() {
        let mut req = vec![0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0];
        let mut busid = [0u8; 32];
        busid[..3].copy_from_slice(b"1-2");
        req.extend_from_slice(&busid);
        let op = read_op_request(&mut req.as_slice()).unwrap();
        assert_eq!(op, OpRequest::Import { busid: "1-2".into() });
    }

    #[test]
    fn parses_submit_with_out_data() {
        let mut b = Vec::new();
        for v in [CMD_SUBMIT, 7, 0x0001_0001, DIR_OUT, 1, 0, 3, 0, 0, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&[0u8; 8]);
        b.extend_from_slice(b"abc");
        match read_command(&mut b.as_slice()).unwrap() {
            Command::Submit(s) => {
                assert_eq!(s.seqnum, 7);
                assert_eq!(s.ep, 1);
                assert_eq!(s.data, b"abc");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ret_submit_is_48_bytes_plus_data() {
        let r = encode_ret_submit(5, 0, b"xy", 2);
        assert_eq!(r.len(), 50);
        assert_eq!(&r[0..4], &RET_SUBMIT.to_be_bytes());
        assert_eq!(&r[24..28], &2i32.to_be_bytes());
        assert_eq!(encode_ret_unlink(9, ECONNRESET).len(), 48);
    }
}
