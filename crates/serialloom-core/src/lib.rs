//! # serialloom-core
//!
//! The engine behind SerialLoom. It has no UI dependency and can be embedded
//! in other tools or driven from tests.
//!
//! * [`driver`]: creates and removes virtual COM ports
//!   (the SerialLoom UMDF driver on Windows, or an in-memory demo driver).
//! * [`hub`]: moves bytes between endpoints (serial ports, TCP) in user mode.
//!   Splitting, joining, bridging and bus sharing are all hubs with different
//!   routing rules.
//! * [`engine`]: owns the hubs, persists the topology and joins the two layers.

pub mod config;
pub mod driver;
pub mod engine;
pub mod error;
pub mod hub;
pub mod logging;
pub mod model;
pub mod ports;
pub mod usbip;

pub use engine::Engine;
pub use error::{Error, Result};
