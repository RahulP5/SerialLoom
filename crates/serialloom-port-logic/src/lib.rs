//! What the SerialLoom driver does with each serial request, without any
//! WDF calls. The driver (`drivers/serialloom-port`) wraps this in UMDF glue;
//! keeping it separate lets the regular test suite cover it.

pub mod logic;
pub mod serial;

pub use logic::*;
