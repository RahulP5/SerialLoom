//! Virtual USB serial devices served over USB/IP.
//!
//! Windows sees each device as a USB serial adapter and creates a COM port
//! with its own Microsoft-signed `usbser.sys`. The USB/IP client on Windows
//! is the usbip-win2 virtual host controller, whose drivers are signed by
//! Microsoft, so no SerialLoom driver signing is needed.

pub mod cdc;
pub mod protocol;
pub mod server;

pub use server::{UsbipServer, VirtualDevice};
