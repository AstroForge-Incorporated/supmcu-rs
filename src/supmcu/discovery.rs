use crate::{supmcu::parsing::*, ParsingError};

pub enum PremadeTelemetryDefs {
    FirmwareVersion,
    Length,
    Name,
    Format,
    TlmAmount,
    CmdAmount,
    CmdName,
    Simulatable,
    McuId,
}

impl Into<SupMCUTelemetryDefinition> for PremadeTelemetryDefs {
    fn into(self) -> SupMCUTelemetryDefinition {
        match self {
            PremadeTelemetryDefs::FirmwareVersion => SupMCUTelemetryDefinition {
                name: "Firmware Version".into(),
                format: SupMCUFormat::new("S"),
                length: Some(77),
                telemetry_type: TelemetryType::SupMCU,
                ..Default::default()
            },
            PremadeTelemetryDefs::Length | PremadeTelemetryDefs::Simulatable => {
                SupMCUTelemetryDefinition {
                    name: "Length".into(),
                    format: SupMCUFormat::new("s"),
                    ..Default::default()
                }
            }
            PremadeTelemetryDefs::Name => SupMCUTelemetryDefinition {
                name: "Name".into(),
                format: SupMCUFormat::new("S"),
                length: Some(33),
                ..Default::default()
            },
            PremadeTelemetryDefs::Format => SupMCUTelemetryDefinition {
                name: "Format".into(),
                format: SupMCUFormat::new("S"),
                length: Some(25),
                ..Default::default()
            },
            PremadeTelemetryDefs::TlmAmount => SupMCUTelemetryDefinition {
                name: "Amount".into(),
                format: SupMCUFormat::new("ss"),
                idx: 14,
                telemetry_type: TelemetryType::SupMCU,
                ..Default::default()
            },
            PremadeTelemetryDefs::CmdAmount => SupMCUTelemetryDefinition {
                name: "Commands".into(),
                format: SupMCUFormat::new("s"),
                idx: 17,
                telemetry_type: TelemetryType::SupMCU,
                ..Default::default()
            },
            PremadeTelemetryDefs::CmdName => SupMCUTelemetryDefinition {
                name: "Command Name".into(),
                format: SupMCUFormat::new("S"),
                length: Some(33),
                telemetry_type: TelemetryType::SupMCU,
                ..Default::default()
            },
            PremadeTelemetryDefs::McuId => SupMCUTelemetryDefinition {
                name: "MCU ID".into(),
                format: SupMCUFormat::new("u"),
                idx: 19,
                telemetry_type: TelemetryType::SupMCU,
                ..Default::default()
            },
        }
    }
}

impl TryFrom<&str> for PremadeTelemetryDefs {
    type Error = ParsingError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s.to_uppercase().as_str() {
            "NAME" => Ok(PremadeTelemetryDefs::Name),
            "LENGTH" => Ok(PremadeTelemetryDefs::Length),
            "FORMAT" => Ok(PremadeTelemetryDefs::Format),
            "SIMULATABLE" => Ok(PremadeTelemetryDefs::Simulatable),
            "MCU_ID" => Ok(PremadeTelemetryDefs::McuId),
            "VERSION" => Ok(PremadeTelemetryDefs::FirmwareVersion),
            _ => Err(ParsingError::CommandParsingError(format!(
                "Invalid suffix {s}"
            ))),
        }
    }
}
