//! System serial port enumeration.

use std::cmp::Ordering;

use serialport::SerialPortType;

use crate::model::{PortInfo, PortKind};

/// Ports reported by the operating system.
pub fn system_ports() -> crate::Result<Vec<PortInfo>> {
    let ports = serialport::available_ports()?;
    Ok(ports
        .into_iter()
        .map(|p| {
            let mut info = PortInfo {
                name: p.port_name,
                kind: PortKind::Unknown,
                description: None,
                manufacturer: None,
                vid: None,
                pid: None,
                serial_number: None,
                in_use_by: None,
                present: true,
            };
            match p.port_type {
                SerialPortType::UsbPort(usb) => {
                    info.kind = PortKind::Usb;
                    info.vid = Some(usb.vid);
                    info.pid = Some(usb.pid);
                    info.description = usb.product;
                    info.manufacturer = usb.manufacturer;
                    info.serial_number = usb.serial_number;
                }
                SerialPortType::PciPort => info.kind = PortKind::Pci,
                SerialPortType::BluetoothPort => info.kind = PortKind::Bluetooth,
                SerialPortType::Unknown => {}
            }
            info
        })
        .collect())
}

/// Orders `COM2` before `COM10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn split(s: &str) -> (String, u64) {
        let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (head, tail) = s.split_at(s.len() - digits);
        (head.to_ascii_uppercase(), tail.parse().unwrap_or(0))
    }
    split(a).cmp(&split(b))
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;

    #[test]
    fn sorts_naturally() {
        let mut v = vec!["COM10", "CNCB0", "COM2", "COM1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["CNCB0", "COM1", "COM2", "COM10"]);
    }
}
