use crate::ParsingError;
use byteorder::{ReadBytesExt, LE};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;
use std::io::{BufRead, Cursor};
use std::mem::size_of;

use async_graphql::{Enum, SimpleObject};

#[cfg(feature = "pumqry")]
use clap::ValueEnum;

#[cfg(test)]
use rand::rngs::SmallRng;

use super::DEFAULT_RESPONSE_DELAY;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Enum)]
#[repr(u8)]
/// Different possible data types that can be returned from SupMCU Telemetry
pub enum DataType {
    Str = b'S',
    Char = b'c',
    UINT8 = b'u',
    INT8 = b't',
    UINT16 = b's',
    INT16 = b'n',
    UINT32 = b'i',
    INT32 = b'd',
    UINT64 = b'l',
    INT64 = b'k',
    Float = b'f',
    Double = b'F',
    Hex8 = b'x',
    Hex16 = b'z',
}

// e.g. SupMCUValue::I8.into() == 't'
impl Into<char> for DataType {
    fn into(self) -> char {
        self as u8 as char
    }
}

impl TryFrom<char> for DataType {
    type Error = ParsingError;

    fn try_from(c: char) -> Result<Self, ParsingError> {
        match c {
            'S' => Ok(DataType::Str),
            'c' => Ok(DataType::Char),
            'u' => Ok(DataType::UINT8),
            't' => Ok(DataType::INT8),
            's' => Ok(DataType::UINT16),
            'n' => Ok(DataType::INT16),
            'i' => Ok(DataType::UINT32),
            'd' => Ok(DataType::INT32),
            'l' => Ok(DataType::UINT64),
            'k' => Ok(DataType::INT64),
            'f' => Ok(DataType::Float),
            'F' => Ok(DataType::Double),
            'x' => Ok(DataType::Hex8),
            'X' => Ok(DataType::Hex8),
            'z' => Ok(DataType::Hex16),
            'Z' => Ok(DataType::Hex16),
            _ => Err(ParsingError::InvalidFormatCharacter(c)),
        }
    }
}

impl DataType {
    /// Returns the size in bytes of the data type, unless the type is Str
    pub fn get_byte_length(&self) -> Option<usize> {
        match self {
            DataType::Str => None,
            DataType::Char => Some(1),
            DataType::UINT8 => Some(size_of::<u8>()),
            DataType::INT8 => Some(size_of::<i8>()),
            DataType::UINT16 => Some(size_of::<u16>()),
            DataType::INT16 => Some(size_of::<i16>()),
            DataType::UINT32 => Some(size_of::<u32>()),
            DataType::INT32 => Some(size_of::<i32>()),
            DataType::UINT64 => Some(size_of::<u64>()),
            DataType::INT64 => Some(size_of::<i64>()),
            DataType::Float => Some(size_of::<f32>()),
            DataType::Double => Some(size_of::<f64>()),
            DataType::Hex8 => Some(1),
            DataType::Hex16 => Some(2),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, SimpleObject)]
/// A format to describe the module telemetry data
pub struct SupMCUFormat {
    format: Vec<DataType>,
}

impl IntoIterator for SupMCUFormat {
    type Item = DataType;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.format.into_iter()
    }
}

impl SupMCUFormat {
    /// Creates a new SupMCUFormat from the valid format characters in a string
    pub fn new(fmt_str: &str) -> Self {
        let mut format = vec![];
        for c in fmt_str.chars() {
            if let Ok(t) = DataType::try_from(c) {
                format.push(t);
            }
        }
        SupMCUFormat { format }
    }

    /// Returns the byte length of the data that the format
    /// specifies or `None` if there is a string type
    pub fn get_byte_length(&self) -> Option<usize> {
        let mut sum: usize = 0;
        for b in self.format.as_slice() {
            if let Some(l) = b.get_byte_length() {
                sum += l;
            } else {
                return None;
            }
        }
        Some(sum)
    }

    /// Returns the stored format string
    pub fn get_format_str(&self) -> String {
        let mut s = String::new();
        for c in self.format.as_slice() {
            s.push((*c).into());
        }
        s
    }

    /// Parses telemetry data into a vector of `SupMCUValue`s
    pub fn parse_data(
        &self,
        rdr: &mut Cursor<&Vec<u8>>,
    ) -> Result<Vec<SupMCUValue>, ParsingError> {
        let mut out = vec![];

        for dt in self.format.as_slice() {
            out.push(match dt {
                DataType::Str => {
                    let mut buf = vec![];
                    rdr.read_until(0, &mut buf)?;
                    buf.pop();
                    SupMCUValue::Str(String::from_utf8(buf)?)
                }
                DataType::Char => SupMCUValue::Char(rdr.read_u8()? as char),
                DataType::UINT8 => SupMCUValue::U8(rdr.read_u8()?),
                DataType::INT8 => SupMCUValue::I8(rdr.read_i8()?),
                DataType::UINT16 => SupMCUValue::U16(rdr.read_u16::<LE>()?),
                DataType::INT16 => SupMCUValue::I16(rdr.read_i16::<LE>()?),
                DataType::UINT32 => SupMCUValue::U32(rdr.read_u32::<LE>()?),
                DataType::INT32 => SupMCUValue::I32(rdr.read_i32::<LE>()?),
                DataType::UINT64 => SupMCUValue::U64(rdr.read_u64::<LE>()?),
                DataType::INT64 => SupMCUValue::I64(rdr.read_i64::<LE>()?),
                DataType::Float => SupMCUValue::Float(rdr.read_f32::<LE>()?),
                DataType::Double => SupMCUValue::Double(rdr.read_f64::<LE>()?),
                DataType::Hex8 => SupMCUValue::Hex8(rdr.read_u8()?),
                DataType::Hex16 => SupMCUValue::Hex16(rdr.read_u16::<LE>()?),
            });
        }
        Ok(out)
    }

    /// Generates random data as a vector of `SupMCUValue`s
    #[cfg(test)]
    pub fn random_data(&self, rng: &mut SmallRng) -> Vec<SupMCUValue> {
        use rand::Rng;

        let mut out = vec![];

        for dt in self.format.as_slice() {
            out.push(match dt {
                DataType::Str => SupMCUValue::Str("A random string".into()),
                DataType::Char => SupMCUValue::Char(rng.gen::<u8>() as char),
                DataType::UINT8 => SupMCUValue::U8(rng.gen()),
                DataType::INT8 => SupMCUValue::I8(rng.gen()),
                DataType::UINT16 => SupMCUValue::U16(rng.gen()),
                DataType::INT16 => SupMCUValue::I16(rng.gen()),
                DataType::UINT32 => SupMCUValue::U32(rng.gen()),
                DataType::INT32 => SupMCUValue::I32(rng.gen()),
                DataType::UINT64 => SupMCUValue::U64(rng.gen()),
                DataType::INT64 => SupMCUValue::I64(rng.gen()),
                DataType::Float => SupMCUValue::Float(rng.gen()),
                DataType::Double => SupMCUValue::Double(rng.gen()),
                DataType::Hex8 => SupMCUValue::Hex8(rng.gen()),
                DataType::Hex16 => SupMCUValue::Hex16(rng.gen()),
            });
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum SupMCUValue {
    Str(String),
    Char(char),
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    Float(f32),
    Double(f64),
    Hex8(u8),
    Hex16(u16),
}

impl fmt::Display for SupMCUValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SupMCUValue::Str(i) => write!(f, "{i}"),
            SupMCUValue::Char(i) => write!(f, "{i}"),
            SupMCUValue::U8(i) => write!(f, "{i}"),
            SupMCUValue::I8(i) => write!(f, "{i}"),
            SupMCUValue::U16(i) => write!(f, "{i}"),
            SupMCUValue::I16(i) => write!(f, "{i}"),
            SupMCUValue::U32(i) => write!(f, "{i}"),
            SupMCUValue::I32(i) => write!(f, "{i}"),
            SupMCUValue::U64(i) => write!(f, "{i}"),
            SupMCUValue::I64(i) => write!(f, "{i}"),
            SupMCUValue::Float(i) => write!(f, "{i}"),
            SupMCUValue::Double(i) => write!(f, "{i}"),
            SupMCUValue::Hex8(i) => write!(f, "0x{i:x}"),
            SupMCUValue::Hex16(i) => write!(f, "0x{i:x}"),
        }
    }
}

impl Into<Vec<u8>> for SupMCUValue {
    fn into(self) -> Vec<u8> {
        match self {
            SupMCUValue::Str(i) => i.into_bytes(),
            SupMCUValue::Char(i) => (i as u8).to_le_bytes().to_vec(),
            SupMCUValue::U8(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::I8(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::U16(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::I16(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::U32(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::I32(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::U64(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::I64(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::Float(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::Double(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::Hex8(i) => i.to_le_bytes().to_vec(),
            SupMCUValue::Hex16(i) => i.to_le_bytes().to_vec(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SupMCUHDR {
    pub ready: bool,
    pub timestamp: u32,
}

impl TryFrom<&mut Cursor<&Vec<u8>>> for SupMCUHDR {
    type Error = ParsingError;

    fn try_from(rdr: &mut Cursor<&Vec<u8>>) -> Result<Self, Self::Error> {
        Ok(SupMCUHDR {
            ready: rdr.read_u8()? & 0b01 == 1,
            timestamp: rdr.read_u32::<LE>()?,
        })
    }
}

#[cfg(test)]
impl Into<Vec<u8>> for SupMCUHDR {
    fn into(self) -> Vec<u8> {
        let mut buf = vec![self.ready as u8];
        buf.extend(self.timestamp.to_le_bytes());
        buf
    }
}

pub type SupMCUTelemetryData = Vec<SupMCUValue>;

#[derive(Debug, Serialize, Deserialize)]
pub struct SupMCUTelemetry {
    pub definition: SupMCUTelemetryDefinition,
    pub header: SupMCUHDR,
    pub data: SupMCUTelemetryData,
}

impl SupMCUTelemetry {
    pub fn from_bytes(
        buff: Vec<u8>,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<Self, ParsingError> {
        let mut rdr = Cursor::new(&buff);

        Ok(SupMCUTelemetry {
            definition: def.clone(),
            header: SupMCUHDR::try_from(&mut rdr)?,
            data: def.format.parse_data(&mut rdr)?,
        })
    }
}

#[cfg(test)]
impl<'a> Into<&'a [u8]> for SupMCUTelemetry {
    fn into(self) -> &'a [u8] {
        todo!()
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize, Default, Copy, Enum)]
#[cfg_attr(feature = "pumqry", derive(ValueEnum))]
#[cfg_attr(feature = "pumqry", clap(rename_all = "lower"))]
pub enum TelemetryType {
    #[default]
    SupMCU,
    Module,
}

impl fmt::Display for TelemetryType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TelemetryType::SupMCU => write!(f, "SupMCU"),
            TelemetryType::Module => write!(f, "Module"),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize, Copy, Enum, Default)]
pub enum McuType {
    #[default]
    UNKNOWN,
    PIC24EP256MC206,
    PIC24EP512MC206,
}

impl fmt::Display for McuType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            McuType::UNKNOWN => write!(f, "UKNOWN"),
            McuType::PIC24EP256MC206 => write!(f, "PIC24EP256MC206"),
            McuType::PIC24EP512MC206 => write!(f, "PIC24EP512MC206"),
        }
    }
}

impl TryFrom<&u8> for McuType {
    type Error = ParsingError;
    fn try_from(value: &u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::PIC24EP256MC206),
            2 => Ok(Self::PIC24EP512MC206),
            _ => Err(ParsingError::McuIdParsingError(*value)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, SimpleObject)]
pub struct SupMCUTelemetryDefinition {
    pub name: String,
    #[serde(flatten)]
    pub format: SupMCUFormat,
    pub length: Option<usize>,
    #[graphql(skip)]
    pub default_sim_value: Option<Vec<SupMCUValue>>,
    pub idx: usize,
    pub telemetry_type: TelemetryType,
}

impl Default for SupMCUTelemetryDefinition {
    fn default() -> Self {
        SupMCUTelemetryDefinition {
            name: "".into(),
            format: SupMCUFormat::new(""),
            length: None,
            default_sim_value: None,
            idx: 0,
            telemetry_type: TelemetryType::SupMCU,
        }
    }
}

impl SupMCUTelemetryDefinition {
    pub fn simulatable(&self) -> bool {
        self.default_sim_value.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, SimpleObject)]
pub struct SupMCUCommand {
    pub name: String,
    pub idx: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, SimpleObject)]
pub struct SupMCUModuleDefinition {
    /// This is the prefix to every SCPI MODULE command (e.g. `{cmd_name}:TEL? 15`)
    pub name: String,
    pub address: u16,
    pub simulatable: bool,
    pub telemetry: Vec<SupMCUTelemetryDefinition>,
    pub commands: Vec<SupMCUCommand>,
    pub mcu: McuType,
    pub response_delay: f32,
}

impl Default for SupMCUModuleDefinition {
    fn default() -> Self {
        SupMCUModuleDefinition {
            name: "".into(),
            address: 0,
            simulatable: false,
            telemetry: vec![],
            commands: vec![],
            mcu: McuType::UNKNOWN,
            response_delay: DEFAULT_RESPONSE_DELAY,
        }
    }
}

impl fmt::Display for SupMCUModuleDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @ {}", self.name, self.address)
    }
}

impl SupMCUModuleDefinition {
    pub fn get_supmcu_telemetry(&self) -> Vec<SupMCUTelemetryDefinition> {
        self.telemetry
            .clone()
            .into_iter()
            .filter(|def| def.telemetry_type == TelemetryType::SupMCU)
            .sorted_by_key(|def| def.idx)
            .collect()
    }

    pub fn get_module_telemetry(&self) -> Vec<SupMCUTelemetryDefinition> {
        self.telemetry
            .clone()
            .into_iter()
            .filter(|def| def.telemetry_type == TelemetryType::Module)
            .sorted_by_key(|def| def.idx)
            .collect()
    }
}
