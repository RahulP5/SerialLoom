//! Contract between the SerialLoom virtual port driver and the SerialLoom app.
//!
//! Every virtual port is one device with two sides:
//!
//! * the **COM side**, opened by any program as `\\.\COM20`, which behaves
//!   like a normal serial port;
//! * the **control side**, opened only by SerialLoom through the device
//!   interface [`CONTROL_INTERFACE_GUID`] plus the reference string
//!   [`CONTROL_REFERENCE`]. Bytes the program writes to the COM side are
//!   read here, and bytes written here are what the program reads.
//!
//! This crate is `no_std` and has no dependencies so the driver can use it too.

#![no_std]

/// Hardware ID of a SerialLoom virtual port (root-enumerated).
pub const HARDWARE_ID: &str = "Root\\SerialLoomPort";

/// Reference string appended to the control interface path.
pub const CONTROL_REFERENCE: &str = "control";

/// `{6F1C3E2A-9B4D-4C8E-A2F7-5D3B9E1C0A47}`: device interface for the control side.
pub const CONTROL_INTERFACE_GUID: Guid = Guid {
    data1: 0x6F1C_3E2A,
    data2: 0x9B4D,
    data3: 0x4C8E,
    data4: [0xA2, 0xF7, 0x5D, 0x3B, 0x9E, 0x1C, 0x0A, 0x47],
};

/// `{86E0D1E0-8089-11D0-9CE4-08003E301F73}`: `GUID_DEVINTERFACE_COMPORT`.
pub const COMPORT_INTERFACE_GUID: Guid = Guid {
    data1: 0x86E0_D1E0,
    data2: 0x8089,
    data3: 0x11D0,
    data4: [0x9C, 0xE4, 0x08, 0x00, 0x3E, 0x30, 0x1F, 0x73],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

const FILE_DEVICE_SERIAL_PORT: u32 = 0x1b;

/// `CTL_CODE(DeviceType, Function, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
pub const fn ctl_code(device_type: u32, function: u32) -> u32 {
    (device_type << 16) | (function << 2)
}

/// Control side: returns [`PortState`].
pub const IOCTL_SERIALLOOM_GET_STATE: u32 = ctl_code(FILE_DEVICE_SERIAL_PORT, 0x800);

/// Control side: input is a `u32` of modem status bits
/// ([`MODEM_CTS`] | [`MODEM_DSR`] | [`MODEM_RI`] | [`MODEM_DCD`]) that the COM
/// side should see, or [`MODEM_AUTOMATIC`] to follow the control connection.
pub const IOCTL_SERIALLOOM_SET_MODEM_STATUS: u32 = ctl_code(FILE_DEVICE_SERIAL_PORT, 0x801);

pub const MODEM_CTS: u32 = 0x10;
pub const MODEM_DSR: u32 = 0x20;
pub const MODEM_RI: u32 = 0x40;
pub const MODEM_DCD: u32 = 0x80;
pub const MODEM_AUTOMATIC: u32 = u32::MAX;

pub const PORT_STATE_VERSION: u32 = 1;

/// Snapshot of what the program on the COM side has configured.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortState {
    pub version: u32,
    /// 1 when a program has the COM side open.
    pub com_open: u32,
    pub baud_rate: u32,
    /// 5 to 8.
    pub data_bits: u8,
    /// 0 none, 1 odd, 2 even, 3 mark, 4 space.
    pub parity: u8,
    /// 0 = 1 bit, 1 = 1.5 bits, 2 = 2 bits.
    pub stop_bits: u8,
    pub _reserved: u8,
    /// Bit 0 DTR, bit 1 RTS.
    pub dtr_rts: u32,
    /// Bytes waiting for the program to read.
    pub to_com_queued: u32,
    /// Bytes the program wrote that SerialLoom has not read yet.
    pub from_com_queued: u32,
    /// Total bytes the program wrote.
    pub from_com_total: u64,
    /// Total bytes delivered to the program.
    pub to_com_total: u64,
}

pub const PORT_STATE_SIZE: usize = core::mem::size_of::<PortState>();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_values() {
        // Same formula as IOCTL_SERIAL_SET_BAUD_RATE (0x1b0004).
        assert_eq!(ctl_code(0x1b, 1), 0x001b_0004);
        assert_eq!(IOCTL_SERIALLOOM_GET_STATE, 0x001b_2000);
    }

    #[test]
    fn state_layout_is_stable() {
        assert_eq!(PORT_STATE_SIZE, 48);
    }
}
