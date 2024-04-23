use crate::{
    supmcu::{discovery::PremadeTelemetryDefs, parsing::*, FOOTER_SIZE, HEADER_SIZE},
    SupMCUError,
};
use i2cdev::core::I2CDevice;
use rand::{distributions::Bernoulli, prelude::Distribution, random, rngs::SmallRng};

#[cfg(checksum)]
use crate::supmcu::CRC32;

pub struct TestI2CDevice {
    /// PRNG to generate telemetry values from
    rng: SmallRng,
    hdr_rng: Bernoulli,
    pub definition: SupMCUModuleDefinition,
    next_response: Option<Vec<u8>>,
}

impl TestI2CDevice {
    pub fn new(rng: SmallRng, def: SupMCUModuleDefinition, nonreadys: bool) -> Self {
        TestI2CDevice {
            rng,
            hdr_rng: Bernoulli::new(if nonreadys { 0.9 } else { 1.0 }).unwrap(),
            definition: def,
            next_response: None,
        }
    }

    /// Parses command strings and returns a vec of bytes as a response.  
    fn parse_cmd(&mut self, cmd: &str) -> Result<Vec<u8>, SupMCUError> {
        println!("Parsing command {cmd:?}");
        let (module, cmd) = cmd.trim_end().split_once(':').unwrap();

        let mut buf = self.make_header();

        // Checking if request is for telemetry or a command
        if cmd.starts_with("TEL?") {
            // Checking for suffix like ',NAME' or ',LENGTH'
            if let Some(split) = cmd.split_once(',') {
                // Suffix is present, parse it and create an appropriate response
                let idx = split.0.replace("TEL? ", "").parse::<usize>().unwrap();
                let resp_def: SupMCUTelemetryDefinition =
                    PremadeTelemetryDefs::try_from(split.1)?.into();
                let len = resp_def
                    .format
                    .get_byte_length()
                    .unwrap_or_else(|| resp_def.length.unwrap())
                    + HEADER_SIZE;

                buf.extend(match resp_def.name.to_uppercase().as_str() {
                    "NAME" => {
                        (self.definition.telemetry[idx].name.clone() + "\0").into_bytes()
                    }
                    "FORMAT" => self.definition.telemetry[idx]
                        .format
                        .get_format_str()
                        .into_bytes(),
                    "LENGTH" => (self.definition.telemetry[idx].length.unwrap() as u16)
                        .to_le_bytes()
                        .to_vec(),
                    "SIMULATABLE" => {
                        vec![self.definition.telemetry[idx].simulatable() as u8]
                    }
                    _ => panic!("Invalid command suffix {}", split.1),
                });
                buf.resize(len, 0);
                Ok(self.add_footer(buf))
            } else {
                // Suffix isn't present, command is requesting telemetry data
                let tel = if module == "SUP" {
                    self.definition.get_supmcu_telemetry()
                } else {
                    self.definition.get_module_telemetry()
                };
                let idx = cmd.replace("TEL? ", "").parse::<usize>().unwrap();
                let len = tel[idx]
                    .format
                    .get_byte_length()
                    .unwrap_or_else(|| tel[idx].length.unwrap())
                    + HEADER_SIZE;
                buf.extend(self.make_data(&tel[idx]));
                buf.resize(len, 0);
                Ok(self.add_footer(buf))
            }
        } else if cmd.starts_with("COM?") {
            // Request is for a command.
            let idx = cmd.replace("COM? ", "").parse::<usize>().unwrap();
            // This len stuff could maybe be a constant
            let cmd_def: SupMCUTelemetryDefinition = PremadeTelemetryDefs::CmdName.into();
            let len = cmd_def.length.unwrap() + HEADER_SIZE;

            buf.extend(self.definition.commands[idx].name.clone().into_bytes());
            buf.resize(len, 0);
            Ok(self.add_footer(buf))
        } else {
            // Needed an else condition to satisfy the compiler, but this shouldn't ever run
            // unless other random commands are being sent during testing and need to be handled.
            unimplemented!()
        }
    }

    /// Makes a header with a random timestamp and random readiness
    fn make_header(&mut self) -> Vec<u8> {
        SupMCUHDR {
            ready: self.hdr_rng.sample(&mut rand::thread_rng()),
            timestamp: random(),
        }
        .into()
    }

    #[cfg(not(checksum))]
    fn add_footer(&mut self, mut data: Vec<u8>) -> Vec<u8> {
        data.extend(std::iter::repeat(0).take(FOOTER_SIZE));
        data
    }

    #[cfg(checksum)]
    fn add_footer(&mut self, mut data: Vec<u8>) -> Vec<u8> {
        data.extend(CRC32.checksum(data.as_slice()).to_le_bytes());
        data
    }

    /// Creates a response to a telemetry reqeust using random data
    fn make_data(&mut self, def: &SupMCUTelemetryDefinition) -> Vec<u8> {
        // Some telemetry items require special handling, specifically the ones in discovery.rs
        match (def.idx, &def.telemetry_type) {
            // Version string request.  This currently works to provide the cmd name.
            (0, TelemetryType::SupMCU) => {
                format!("{} something", self.definition.name).into_bytes()
            }
            // Request for the number of supmcu and module telemetry items
            (14, TelemetryType::SupMCU) => {
                let supmcu_len = self.definition.get_supmcu_telemetry().len() as u16;
                let module_len = self.definition.get_module_telemetry().len() as u16;
                let mut buf = supmcu_len.to_le_bytes().to_vec();
                buf.extend(module_len.to_le_bytes());
                buf
            }
            // Request for the number of commands
            (17, TelemetryType::SupMCU) => (self.definition.commands.len() as u16)
                .to_le_bytes()
                .to_vec(),
            // MCU ID
            (19, TelemetryType::SupMCU) => (self.definition.mcu as u8)
                .to_le_bytes()
                .to_vec(),
            _ => {
                let data = def.format.random_data(&mut self.rng);
                let mut buf = vec![];
                for item in data {
                    buf.extend::<Vec<u8>>(item.into())
                }
                buf
            }
        }
    }
}

impl I2CDevice for TestI2CDevice {
    type Error = SupMCUError;

    fn read(&mut self, data: &mut [u8]) -> Result<(), Self::Error> {
        data.clone_from_slice(self.next_response.clone().unwrap().as_mut_slice());
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        self.next_response = Some(self.parse_cmd(&String::from_utf8(data.to_vec())?)?);
        Ok(())
    }

    fn smbus_write_quick(&mut self, _bit: bool) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn smbus_read_block_data(&mut self, _register: u8) -> Result<Vec<u8>, Self::Error> {
        unimplemented!()
    }

    fn smbus_write_block_data(
        &mut self,
        _register: u8,
        _values: &[u8],
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn smbus_process_block(
        &mut self,
        _register: u8,
        _values: &[u8],
    ) -> Result<Vec<u8>, Self::Error> {
        unimplemented!()
    }

    fn smbus_read_i2c_block_data(
        &mut self,
        _register: u8,
        _len: u8,
    ) -> Result<Vec<u8>, Self::Error> {
        unimplemented!()
    }

    fn smbus_write_i2c_block_data(
        &mut self,
        _register: u8,
        _values: &[u8],
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }
}
