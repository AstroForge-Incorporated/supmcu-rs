//! # pumpkin_supmcu-rs
//!
//! This crate is a rust rewrite of the [pumpkin_supmcu](https://gitlab.com/pumpkin-space-systems/public/pumpkin-supmcu) python package.
//! Its purpose is to interact with modules by disovering and parsing telemetry data and communicating via I2C

use i2cdev::linux::LinuxI2CError;
use supmcu::parsing::{SupMCUValue, TelemetryType};
use thiserror::Error;

pub mod supmcu;

#[derive(Error, Debug)]
pub enum SupMCUError {
    #[error("IoError: {0}")]
    IoError(#[from] std::io::Error),
    #[error("{device} (addr {address}): {error}")]
    I2CDevError {
        device: String,
        address: u16,
        error: LinuxI2CError,
    },
    #[error("Failed sending command over I2C ({0:#04x}) {1}")]
    I2CCommandError(u16, String),
    #[error("Failed reading telemetry over I2C ({0:#04x}) {1}")]
    I2CTelemetryError(u16, String),
    #[error("ParsingError: {0}")]
    ParsingError(#[from] ParsingError),
    #[error("Failed to find {0} telemetry item at index {1}")]
    TelemetryIndexError(TelemetryType, usize),
    #[error("module@{0:#04X}: {1} returned a non-ready response.  Try increasing `response_delay`")]
    NonReadyError(u16, String),
    #[error("Failed to validate data with checksum.")]
    ValidationError,
    #[error("SupMCUModuleDefinition not found. Have you run discover?")]
    MissingDefinitionError,
    #[error("AsyncError: {0}")]
    AsyncError(#[from] tokio::task::JoinError),
    #[error("JSONError: {0}")]
    JSONError(#[from] serde_json::Error),
    #[error("Module not found: {0} {1}")]
    ModuleNotFound(String, u16),
    #[error("Unexpected value for {0}: {1}")]
    UnexpectedValue(String, SupMCUValue),
    #[error("Unknown telemetry name {0}")]
    UnknownTelemName(String),
}

impl From<std::string::FromUtf8Error> for SupMCUError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        SupMCUError::ParsingError(ParsingError::StringParsingError(e))
    }
}

#[derive(Error, Debug)]
pub enum ParsingError {
    #[error("Failed to convert bytes into object: {0}")]
    InvalidBytes(String),
    #[error("Invalid format string {0} for bytes {1:?}")]
    InvalidFormatString(String, Vec<u8>),
    #[error("Invalid format character {0}")]
    InvalidFormatCharacter(char),
    #[error("Failed to parse primitive bytes: {0}")]
    ByteParsingError(#[from] std::io::Error),
    #[error("Failed to parse UTF-8 encoded string")]
    StringParsingError(#[from] std::string::FromUtf8Error),
    #[error("Failed to parse command name from version string {0}")]
    VersionParsingError(String),
    #[error("Error parsing command {0}")]
    CommandParsingError(String),
    #[error("Unknown MCU ID {0}")]
    McuIdParsingError(u8),
}
