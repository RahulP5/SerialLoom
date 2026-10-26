use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("{0}")]
    Driver(String),

    #[error("virtual port driver is not available: {0}")]
    DriverUnavailable(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Invalid(String),

    #[error("invalid configuration file: {0}")]
    Config(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
