#![allow(clippy::from_over_into)]

/*!
# SupMCU

The SupMCUModule and SupMCUMaster structs allow easy interactions with SupMCU modules over I2C
by encapsulating functionality like sending commands, requesting and reading telemetry,
discovering modules on an I2C bus, and loading definition files.

## Examples
Discovering modules on an I2C bus
```no_run
# use supmcu_rs::SupMCUError;
use supmcu_rs::supmcu::SupMCUMaster;
use std::time::Duration;

let mut master = SupMCUMaster::new("/dev/i2c-1", None)?;
master.discover_modules()?;

print!("Modules:");
for module in master.modules.iter() {
print!(" {}", module.get_definition()?.name);
}
println!();
# Ok::<(), SupMCUError>(())
```

Loading a definition file

```no_run
# use supmcu_rs::SupMCUError;
use supmcu_rs::supmcu::SupMCUMaster;
use std::{
time::Duration,
path::Path
};

let mut master = SupMCUMaster::new("/dev/i2c-1", None)?;
master.load_def_file(Path::new("definition.json"))?;
# Ok::<(), SupMCUError>(())
```
*/

use crate::{ParsingError, SupMCUError};
use async_graphql::Json;
use async_scoped::TokioScope;

use futures::Future;
use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;
use log::{error, info, trace};
use parsing::*;
use regex::Regex;
use std::{
    collections::HashMap,
    fmt::Debug,
    fs::File,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
use tokio::{runtime, time};

#[cfg(checksum)]
use crc::{Crc, CRC_32_CKSUM};

#[cfg(not(test))]
use log::debug; // Use log crate when building application
#[cfg(test)]
use std::println as debug;

mod discovery;

#[cfg(test)]
mod i2c;
/// Data structures and associated functions to parse data received from modules
pub mod parsing;

// Telemetry system in SupMCU modules steps:
//
// 1. We send a single command to initiate a telemetry request
// 2. We then read X amount of bytes where X is the number of bytes for the telemetry response
// 3. We verify the `ready` flag is set to `1`
// 4. We parse the bytes into one or more primitive types (e.g. Vec<Value> where Value is a type that is u8, u16, u32, u64, i8, i16, i32, i64, String)

// How to parse telemetry:
//
// 1. Get the `MOD:TEL? #,FORMAT` string from the module (cached)
// 2. We read the format string one character at a time to decode the bytes
// 3. For each character, decode X amount of bytes as primitive type Y
// 4. Return vector of parsed primitive values

const HEADER_SIZE: usize = 5;
const FOOTER_SIZE: usize = 8;
const DEFAULT_RESPONSE_DELAY: f32 = 0.05;
const DEFAULT_RETRIES: u8 = 5;
// The amount of extra time allowed when retrying a non-ready response
const RETRY_TIME_INCREMENT: f64 = 0.1;
#[cfg(checksum)]
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_CKSUM);

/**
  A struct to represent/interact with a SupMCU Module connected to via I2C

  In most cases this struct won't have to be created manually, but will be
  initialized during the creation of a [`SupMCUMaster`].

  This struct has methods for interacting with a module by sending commands
  as well as requesting and parsing telemetry data.  It also handles
  discovery of a module at a given I2C address.

  Many of the methods also have async variants with the same basic
  functionality.  These async methods only really differ in the type of
  sleep function used: synchronous or asynchronous.  The IO is all
  synchronous because there are no async I2C crates available that I'm
  aware of.

  ```no_run
# use supmcu_rs::SupMCUError;
use supmcu_rs::supmcu::SupMCUModule;
use std::time::Duration;

let mut module = SupMCUModule::new("/dev/i2c-1", 0x35, Some(5))?;

module.send_command("SUP:LED ON");
# Ok::<(), SupMCUError>(())
```
 **/

pub struct SupMCUModule<T: I2CDevice + Send + Sync> {
    i2c_dev: Box<T>,
    /// Time to wait between requesting data and trying to read data
    last_cmd: String,
    definition: Option<SupMCUModuleDefinition>,
    address: u16,
    max_retries: Option<u8>,
}

impl<T> SupMCUModule<T>
where
    T: I2CDevice + Send + Sync,
{
    /// Sends provided command to the module.
    ///
    /// Also appends a trailing newline if one isn't already present.
    pub fn send_command<S: AsRef<str>>(&mut self, cmd: S) -> Result<(), SupMCUError> {
        let mut cmd = cmd.as_ref().to_string();
        if !cmd.ends_with('\n') {
            cmd += "\n";
        }
        self.i2c_dev
            .write(cmd.as_bytes())
            .map_err(|e| SupMCUError::I2CCommandError(self.address, e.to_string()))?;
        self.last_cmd = cmd[..cmd.len() - 1].to_string();
        if let Ok(def) = self.get_definition() {
            debug!(
                "{}@{:#04X}: sent command: `{}`",
                def.name, self.address, self.last_cmd
            );
        } else {
            debug!("{}: sent command: `{}`", self.address, self.last_cmd);
        }
        Ok(())
    }

    /// Requests telemetry from the module using a telemetry definition found in the module definition.
    pub fn request_telemetry(
        &mut self,
        telemetry_type: TelemetryType,
        idx: usize,
    ) -> Result<(), SupMCUError> {
        let mut def = self
            .get_definition()?
            .telemetry
            .iter()
            .filter(|x| x.idx == idx && x.telemetry_type == telemetry_type);
        let d = def
            .next()
            .ok_or(SupMCUError::TelemetryIndexError(telemetry_type, idx))?
            .to_owned();
        self.request_telemetry_by_def(&d)
    }

    /// Requests and parses telemetry from the module using a telemetry definition found in the module definition.
    pub fn get_telemetry(
        &mut self,
        telemetry_type: TelemetryType,
        idx: usize,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        let mut def = self
            .get_definition()?
            .telemetry
            .iter()
            .filter(|x| x.idx == idx && x.telemetry_type == telemetry_type);
        let d = def
            .next()
            .ok_or(SupMCUError::TelemetryIndexError(telemetry_type, idx))?
            .to_owned();
        self.get_telemetry_by_def(&d)
    }

    /// Requests and parses telemetry from the module using a telemetry definition found in the module definition.
    pub async fn get_telemetry_async(
        &mut self,
        telemetry_type: TelemetryType,
        idx: usize,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        let mut def = self
            .get_definition()?
            .telemetry
            .iter()
            .filter(|x| x.idx == idx && x.telemetry_type == telemetry_type);
        let d = def
            .next()
            .ok_or(SupMCUError::TelemetryIndexError(telemetry_type, idx))?
            .to_owned();
        self.get_telemetry_by_def_async(&d).await
    }

    /// Requests telemetry from the module using the provided definitions.
    pub fn request_telemetry_by_def(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<(), SupMCUError> {
        self.send_command(self.create_tlm_command(def)?)
    }

    /// Requests and parses telemetry from the module using the provided definition.
    pub fn get_telemetry_by_def(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        self.request_telemetry_by_def(def)?;
        self.i2c_delay();
        self.read_telemetry_response_safe(def)
    }

    /// Requests and parses telemetry from the module using the provided definition asynchronously
    pub async fn get_telemetry_by_def_async(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        self.request_telemetry_by_def(def)?;
        self.i2c_delay_async().await;
        self.read_telemetry_response_safe_async(def).await
    }

    /// Requests and parses all telemetry from the module
    pub fn get_all_telemetry(
        &mut self,
    ) -> Result<HashMap<String, Json<SupMCUTelemetryData>>, SupMCUError> {
        let mut telemetry = HashMap::new();
        self.get_definition()?
            .telemetry
            .to_owned()
            .iter()
            .for_each(|d| {
                match self.get_telemetry_by_def(d) {
                    Ok(t) => telemetry.insert(d.name.clone(), Json(t.data)),
                    Err(e) => {
                        let v = Json(vec![SupMCUValue::Str(e.to_string())]);
                        telemetry.insert(d.name.clone(), v)
                    }
                };
            });
        Ok(telemetry)
    }

    /// Requests and parses telemetry by name from module
    pub fn get_telemetry_by_names(
        &mut self,
        names: Vec<String>,
    ) -> Result<HashMap<String, Json<SupMCUTelemetryData>>, SupMCUError> {
        let available_names: Vec<&String> = self
            .get_definition()?
            .telemetry
            .iter()
            .map(|d| &d.name)
            .collect();
        for n in &names {
            if !available_names.contains(&n) {
                return Err(SupMCUError::UnknownTelemName(n.to_owned()));
            }
        }
        let mut telemetry = HashMap::new();
        self.get_definition()?
            .telemetry
            .to_owned()
            .iter()
            .filter(|d| names.contains(&d.name))
            .for_each(|d| {
                match self.get_telemetry_by_def(d) {
                    Ok(t) => telemetry.insert(d.name.clone(), Json(t.data)),
                    Err(e) => {
                        let v = Json(vec![SupMCUValue::Str(e.to_string())]);
                        telemetry.insert(d.name.clone(), v)
                    }
                };
            });
        Ok(telemetry)
    }

    /// Requests and parses all telemetry from the module asynchronously
    pub async fn get_all_telemetry_async(
        &mut self,
    ) -> Result<Vec<Result<SupMCUTelemetry, SupMCUError>>, SupMCUError> {
        let mut telemetry = vec![];
        for tlm_def in self.get_definition()?.telemetry.clone() {
            telemetry.push(self.get_telemetry_by_def_async(&tlm_def).await);
        }
        Ok(telemetry)
    }

    /// Reads a response to a telemetry request from the module.
    pub fn read_telemetry_response(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        let size = SupMCUModule::<T>::telemetry_response_size(def);
        let mut buff = vec![0u8; size];
        self.i2c_dev
            .read(buff.as_mut_slice())
            .map_err(|e| SupMCUError::I2CTelemetryError(self.address, e.to_string()))?;

        #[cfg(checksum)]
        {
            let checksum = buff.split_off(buff.capacity() - FOOTER_SIZE);
            self.validate(&buff, checksum)?;
        }

        trace!("Received telemetry response: {:?}", buff);
        let tel =
            SupMCUTelemetry::from_bytes(buff, def).map_err(SupMCUError::ParsingError)?;
        if tel.header.ready {
            Ok(tel)
        } else {
            Err(SupMCUError::NonReadyError(
                self.address,
                self.last_cmd.clone(),
            ))
        }
    }

    /// Reads a response to a telemetry request and retries the request asynchronously if it comes back non-ready.
    pub async fn read_telemetry_response_safe_async(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        let resp = self.read_telemetry_response(def);
        if let Err(SupMCUError::NonReadyError(..)) = resp {
            self.retry_nonready_async(def, resp).await
        } else {
            resp
        }
    }

    /// Reads a response to a telemetry request and retries the request if it comes back non-ready.
    pub fn read_telemetry_response_safe(
        &mut self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        let resp = self.read_telemetry_response(def);
        if let Err(SupMCUError::NonReadyError(..)) = resp {
            self.retry_nonready(def, resp)
        } else {
            resp
        }
    }

    /// Creates a telemetry request command from a telmetry definition
    fn create_tlm_command(
        &self,
        def: &SupMCUTelemetryDefinition,
    ) -> Result<String, SupMCUError> {
        let cmd = if def.telemetry_type == TelemetryType::SupMCU {
            "SUP"
        } else {
            &self.get_definition()?.name
        };
        Ok(format!("{cmd}:TEL? {}", def.idx))
    }

    /// Get the response delay of this module
    fn response_delay(&self) -> f32 {
        match &self.definition {
            Some(def) => def.response_delay,
            None => DEFAULT_RESPONSE_DELAY,
        }
    }

    /// Sleeps for `self.response_delay` seconds.
    fn i2c_delay(&self) {
        thread::sleep(Duration::from_secs_f32(self.response_delay()));
    }

    /// Sleeps for `self.response_delay` seconds asynchronously.
    async fn i2c_delay_async(&self) {
        time::sleep(Duration::from_secs_f32(self.response_delay())).await;
    }

    /// Returns the length of a telemetry response using the definition.
    ///
    /// Shouldn't ever panic as long as the definition isn't broken, becuase either there
    /// is a string, and the definition's length field should be Some, or there isn't a string,
    /// and you can calculate the size from the format.
    fn telemetry_response_size(def: &SupMCUTelemetryDefinition) -> usize {
        def.format
            .get_byte_length()
            .unwrap_or_else(|| def.length.unwrap())
            + HEADER_SIZE
            + FOOTER_SIZE
    }

    /// Validates data received from a module using a CRC32 checksum.
    #[cfg(checksum)]
    fn validate(&self, data: &Vec<u8>, checksum: Vec<u8>) -> Result<(), SupMCUError> {
        let mut rdr = Cursor::new(&checksum);
        if CRC32.checksum(data) != rdr.read_u32::<LE>()? {
            Err(SupMCUError::ValidationError())
        } else {
            Ok(())
        }
    }

    /// Discovers the command name by parsing the version string.
    async fn discover_cmd_name(&mut self) -> Result<(), SupMCUError> {
        debug!(
            "Discovering module command name for address {}",
            self.address
        );
        if let SupMCUValue::Str(version) = &self
            .get_telemetry_by_def_async(
                &discovery::PremadeTelemetryDefs::FirmwareVersion.into(),
            )
            .await?
            .data[0]
        {
            let v = version.to_string();
            info!("{:#04X}: {}", self.address, v);
            let def = self.get_definition_mut()?;
            let mut cmd_name = v
                .split(' ')
                .next()
                .ok_or_else(|| ParsingError::VersionParsingError(v.clone()))?
                .split('-')
                .next()
                .ok_or_else(|| ParsingError::VersionParsingError(v.clone()))?
                .to_string();
            if cmd_name == "GPSRM" {
                cmd_name = String::from("GPS")
            } else if cmd_name == "RHM3" {
                cmd_name = String::from("RHM")
            }
            def.name = cmd_name;
            def.simulatable = v.contains("(on STM)") || v.contains("(on QSM)");
            debug!("Version: {v}");
            debug!("CMD Name: {}", self.get_definition()?.name);
        }
        Ok(())
    }

    /// Discovers the definition (metadata) for a telemetry item.
    ///
    /// For each telemetry item it gets thee name, format, and sometimes length and simulatability.
    async fn discover_telemetry_definition(
        &mut self,
        telemetry_type: TelemetryType,
        idx: usize,
    ) -> Result<SupMCUTelemetryDefinition, SupMCUError> {
        // replace non-alphanumeric substrings with _ and make everything lowercase
        fn normalize(name: String) -> String {
            let re = Regex::new(r"[^a-zA-Z0-9]+").unwrap();
            let mut s = re.replace_all(&name, "_").to_lowercase();
            if s.ends_with('_') {
                s = s[..s.len() - 1].to_owned()
            }
            s
        }

        debug!("Discovering {telemetry_type} telemetry item {idx}");

        let mut def = SupMCUTelemetryDefinition {
            idx,
            telemetry_type,
            ..Default::default()
        };

        trace!("Requesting telemetry name");
        self.send_command(self.create_tlm_command(&def)? + ",NAME")?;
        self.i2c_delay_async().await;

        trace!("Parsing telemetry name");
        let name_resp = self
            .read_telemetry_response_safe_async(
                &discovery::PremadeTelemetryDefs::Name.into(),
            )
            .await?;
        if let SupMCUValue::Str(name) = &name_resp.data[0] {
            def.name = normalize(name.to_string());
        }

        trace!("Requesting telemetry format");
        self.send_command(self.create_tlm_command(&def)? + ",FORMAT")?;
        self.i2c_delay_async().await;

        trace!("Parsing telemetry format");
        let format_resp = self
            .read_telemetry_response_safe_async(
                &discovery::PremadeTelemetryDefs::Format.into(),
            )
            .await?;
        if let SupMCUValue::Str(format) = &format_resp.data[0] {
            def.format = SupMCUFormat::new(format);
        }

        if def.format.get_byte_length().is_none() {
            trace!("Format includes a string. Requesting telemetry length");
            self.send_command(self.create_tlm_command(&def)? + ",LENGTH")?;
            self.i2c_delay_async().await;

            trace!("Parsing telemetry length");
            let length_resp = self
                .read_telemetry_response_safe_async(
                    &discovery::PremadeTelemetryDefs::Length.into(),
                )
                .await?;
            if let SupMCUValue::U16(length) = length_resp.data[0] {
                def.length = Some(length.into());
            }
        }

        if self.get_definition()?.simulatable {
            trace!("Checking whether telemetry item is simulatable");
            self.send_command(self.create_tlm_command(&def)? + ",SIMULATABLE")?;
            self.i2c_delay_async().await;

            trace!("Parsing simulatability");
            let simulatable_resp = self
                .read_telemetry_response_safe_async(
                    &discovery::PremadeTelemetryDefs::Simulatable.into(),
                )
                .await?;
            if let SupMCUValue::U16(simulatable) = simulatable_resp.data[0] {
                if simulatable == 1 {
                    trace!("Telemetry item is simulatable. Requesting default values.");
                    let defaults = self.get_telemetry_by_def_async(&def).await?;
                    def.default_sim_value = Some(defaults.data);
                } else {
                    trace!("Telemetry item is not simulatable.");
                }
            }
        }
        Ok(def)
    }

    async fn discover_all_telemetry(&mut self) -> Result<(), SupMCUError> {
        debug!(
            "Discovering SupMCU telemetry definitions for {}",
            self.get_definition()?.name
        );
        let vals = self
            .get_telemetry_by_def_async(
                &discovery::PremadeTelemetryDefs::TlmAmount.into(),
            )
            .await?
            .data;
        if let SupMCUValue::U16(supmcu_amount) = vals[0] {
            for i in 0..supmcu_amount {
                let def = self
                    .discover_telemetry_definition(TelemetryType::SupMCU, i as usize)
                    .await?;
                self.get_definition_mut()?.telemetry.push(def);
            }
        }
        debug!(
            "Discovering module telemetry definitions for {}",
            self.get_definition()?.name
        );
        if let SupMCUValue::U16(module_amount) = vals[1] {
            for i in 0..module_amount {
                let def = self
                    .discover_telemetry_definition(TelemetryType::Module, i as usize)
                    .await?;
                self.get_definition_mut()?.telemetry.push(def);
            }
        }
        Ok(())
    }

    async fn discover_commands(&mut self) -> Result<(), SupMCUError> {
        debug!("Discovering commands for {}", self.get_definition()?.name);
        let val = self
            .get_telemetry_by_def_async(
                &discovery::PremadeTelemetryDefs::CmdAmount.into(),
            )
            .await?
            .data;
        if let SupMCUValue::U16(commands_amount) = val[0] {
            for i in 0..commands_amount {
                self.send_command(format!("SUP:COM? {i}"))?;
                self.i2c_delay_async().await;
                if let SupMCUValue::Str(name) = &self
                    .read_telemetry_response_safe_async(
                        &discovery::PremadeTelemetryDefs::CmdName.into(),
                    )
                    .await?
                    .data[0]
                {
                    self.get_definition_mut()?.commands.push(SupMCUCommand {
                        name: name.to_string(),
                        idx: i,
                    })
                }
            }
        }
        Ok(())
    }

    /// Discovers the module definition from the I2C bus.
    async fn discover(&mut self) -> Result<(), SupMCUError> {
        if self.definition.is_none() {
            self.definition = Some(SupMCUModuleDefinition {
                address: self.address,
                ..Default::default()
            });
        }
        self.discover_cmd_name().await?;
        self.discover_all_telemetry().await?;
        if self.get_definition()?.name != "DCPS" {
            self.discover_commands().await?;
        }
        Ok(())
    }

    /// Returns the module definition as a mutable reference
    pub fn get_definition_mut(
        &mut self,
    ) -> Result<&mut SupMCUModuleDefinition, SupMCUError> {
        self.definition
            .as_mut()
            .ok_or(SupMCUError::MissingDefinitionError)
    }

    /// Returns the module definition as a immutable reference
    pub fn get_definition(&self) -> Result<&SupMCUModuleDefinition, SupMCUError> {
        self.definition
            .as_ref()
            .ok_or(SupMCUError::MissingDefinitionError)
    }

    /// Sets the module definition
    pub fn set_definition(&mut self, def: SupMCUModuleDefinition) {
        self.address = def.address;
        self.definition = Some(def);
    }

    /// Check if the module fits a particular definition, will match if addr OR cmd_name match
    pub fn matches(&self, other: &SupMCUModuleDefinition) -> bool {
        match self.get_definition() {
            Ok(def) => other.address == def.address || other.name == def.name,
            Err(_) => false,
        }
    }

    /// Retries a failed telemetry request, increasing the response delay each time.
    ///
    /// A NonReadyError may still be returned if the max retries is exceeded.
    async fn retry_nonready_async(
        &mut self,
        def: &SupMCUTelemetryDefinition,
        resp: Result<SupMCUTelemetry, SupMCUError>,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        if self.max_retries.is_none() {
            return resp;
        }
        let mut retries = 0;
        loop {
            self.send_command(self.last_cmd.clone())?;
            time::sleep(time::Duration::from_secs_f64(
                self.response_delay() as f64 + RETRY_TIME_INCREMENT * retries as f64,
            ))
            .await;
            let resp = self.read_telemetry_response(def);
            if let Err(SupMCUError::NonReadyError(..)) = resp {
                debug!("{} sent a non-ready response.", self.get_definition()?.name);
                retries += 1;
                if retries > self.max_retries.unwrap() {
                    debug!(
                        "Max retries exceeded, returning `SupMCUError::NonReadyError`"
                    );
                    break resp;
                }
                debug!("Retrying...");
                continue;
            } else {
                break resp;
            }
        }
    }

    fn retry_nonready(
        &mut self,
        def: &SupMCUTelemetryDefinition,
        resp: Result<SupMCUTelemetry, SupMCUError>,
    ) -> Result<SupMCUTelemetry, SupMCUError> {
        if self.max_retries.is_none() {
            return resp;
        }
        let mut retries = 0;
        loop {
            self.send_command(self.last_cmd.clone())?;
            thread::sleep(time::Duration::from_secs_f64(
                self.response_delay() as f64 + RETRY_TIME_INCREMENT * retries as f64,
            ));
            let resp = self.read_telemetry_response(def);
            if let Err(SupMCUError::NonReadyError(..)) = resp {
                debug!("{} sent a non-ready response.", self.get_definition()?.name);
                retries += 1;
                if retries > self.max_retries.unwrap() {
                    debug!(
                        "Max retries exceeded, returning `SupMCUError::NonReadyError`"
                    );
                    break resp;
                }
                debug!("Retrying...");
                continue;
            } else {
                break resp;
            }
        }
    }

    /// Returns the address
    pub fn get_address(&self) -> u16 {
        self.address
    }
}

impl<T> Debug for SupMCUModule<T>
where
    T: I2CDevice + Send + Sync,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupMCUModule")
            .field("address", &self.address)
            .field("response_delay", &self.response_delay())
            .field("max_retries", &self.max_retries)
            .field("last_cmd", &self.last_cmd)
            .finish()
    }
}

impl SupMCUModule<LinuxI2CDevice> {
    /// Creates a new SupMCUModule
    pub fn new(
        device: &str,
        address: u16,
        max_retries: Option<u8>,
    ) -> Result<Self, SupMCUError> {
        let dev = LinuxI2CDevice::new(device, address).map_err(|error| {
            SupMCUError::I2CDevError {
                device: String::from(device),
                address,
                error,
            }
        })?;
        Ok(SupMCUModule {
            i2c_dev: Box::new(dev),
            last_cmd: "".into(),
            definition: None,
            max_retries,
            address,
        })
    }

    /// Creates a new SupMCUModule from a SupMCUModuleDefinition
    pub fn new_from_def(
        device: &str,
        max_retries: Option<u8>,
        def: SupMCUModuleDefinition,
    ) -> Result<Self, SupMCUError> {
        let address = def.address;
        let dev = LinuxI2CDevice::new(device, def.address).map_err(|error| {
            SupMCUError::I2CDevError {
                device: String::from(device),
                address,
                error,
            }
        })?;
        Ok(SupMCUModule {
            i2c_dev: Box::new(dev),
            definition: Some(def),
            last_cmd: "".into(),
            max_retries,
            address,
        })
    }
}

/**
A struct to represent an I2C bus of SupMCU modules

This basically just holds a vec of [`SupMCUModule`]s and an async runtime.
The async runtime is used to run async functions like [`SupMCUModule.get_telemetry_by_def_async`](SupMCUModule#memthod.get_telemetry_by_def_async)
from withing a sync context.  This allows you to take advantage of the speedups
that come from accessing modules in parallel without having to deal with an entire
async application.

```no_run
# use supmcu_rs::SupMCUError;
use supmcu_rs::supmcu::{
    SupMCUMaster,
    parsing::*
};
use std::{
    time::Duration,
    path::Path
};

// Initialize master from definition file
let mut master = SupMCUMaster::new("/dev/i2c-1", None)?;
master.load_def_file(Path::new("definition.json"))?;

// Get the first telemetry item  (version string) from each module
let versions = master
    .for_each(|module| module.get_telemetry_async(TelemetryType::SupMCU, 0))
    .into_iter()
    .collect::<Result<Vec<SupMCUTelemetry>, SupMCUError>>()?;

for version in versions {
    // Prints the version string from each module in the definition.json file
    println!("{}", &version.data[0]);
}
# Ok::<(), SupMCUError>(())
```
**/

/// A SupMCUMaster is used to communicate with SupMCU modules over an I2C bus 
pub struct SupMCUMaster<I: I2CDevice + Send + Sync> {
    /// The [`SupMCUModule`]s available to control
    pub modules: Vec<SupMCUModule<I>>,
    def_file: Option<PathBuf>,
    rt: runtime::Runtime,
}

impl<I> SupMCUMaster<I>
where
    I: I2CDevice + Send + Sync,
{

    /// Discover the definitions for each stored module
    pub fn discover_modules(&mut self) -> Result<(), SupMCUError> {
        log::info!(
            "Discovering modules: {:?}",
            self.modules
                .iter()
                .map(|m| format!("{:#04X}", m.address))
                .collect::<Vec<String>>()
        );
        self.for_each(|module: &mut SupMCUModule<I>| module.discover())
            .into_iter()
            // Consolidating the vec of results into one result
            .collect::<Result<Vec<()>, SupMCUError>>()?;
        Ok(())
    }

    /// Discover an individual module's definition
    pub fn discover_module(
        &mut self,
        module: &SupMCUModuleDefinition,
    ) -> Result<(), SupMCUError> {
        for m in self.modules.iter_mut() {
            if m.matches(module) {
                return self.rt.block_on(async { m.discover().await });
            }
        }
        Err(SupMCUError::ModuleNotFound(
            module.name.clone(),
            module.address,
        ))
    }

    /// Get module definitions of this SupMCUMaster
    pub fn get_definitions(&self) -> Result<Vec<SupMCUModuleDefinition>, SupMCUError> {
        self.modules
            .iter()
            .map(|module| Ok(module.get_definition()?.clone()))
            .collect::<Result<Vec<SupMCUModuleDefinition>, SupMCUError>>()
    }

    /// Getting all the telemetry for each stored module
    pub fn get_all_telemetry(
        &mut self,
    ) -> Vec<Vec<Result<SupMCUTelemetry, SupMCUError>>> {
        self.for_each(|module| async { module.get_all_telemetry_async().await.unwrap() })
    }

    /// Runs a closure for a specific module
    pub fn with_module<F: FnOnce(&SupMCUModule<I>) -> O, O: Send + 'static>(
        &self,
        module: &SupMCUModuleDefinition,
        f: F,
    ) -> Result<O, SupMCUError> {
        self.modules
            .iter()
            .find(|m| m.matches(module))
            .map(f)
            .ok_or(SupMCUError::ModuleNotFound(
                module.name.clone(),
                module.address,
            ))
    }

    /// Runs a closure for a specific module, mutable
    pub fn with_module_mut<F: FnOnce(&mut SupMCUModule<I>) -> O, O: Send + 'static>(
        &mut self,
        module: &SupMCUModuleDefinition,
        f: F,
    ) -> Result<O, SupMCUError> {
        self.modules
            .iter_mut()
            .find(|m| m.matches(module))
            .map(f)
            .ok_or(SupMCUError::ModuleNotFound(
                module.name.clone(),
                module.address,
            ))
    }

    /// Sends a command to a module
    pub fn send_command(
        &mut self,
        module: &SupMCUModuleDefinition,
        command: &str,
    ) -> Result<(), SupMCUError> {
        let module_command = |module: &mut SupMCUModule<I>| module.send_command(command);
        self.with_module_mut(module, module_command)?
    }

    /// Updates a module's response delay
    pub fn response_delay(
        &mut self,
        module: &SupMCUModuleDefinition,
        delay: f32,
    ) -> Result<(), SupMCUError> {
        self.with_module_mut(module, |m| -> Result<(), SupMCUError> {
            m.definition
                .as_mut()
                .ok_or(SupMCUError::MissingDefinitionError)?
                .response_delay = delay;
            Ok(())
        })??;
        if let Some(file) = &self.def_file {
            self.save_def_file(file)?;
        }
        Ok(())
    }

    /// Runs an async function for each module and returns their results in a Vec
    pub fn for_each<'a, F, T, O>(&'a mut self, f: F) -> Vec<O>
    where
        F: Fn(&'a mut SupMCUModule<I>) -> T,
        T: Future<Output = O> + Send,
        O: Send + 'static,
    {
        // Wait for the entire async block to finish
        self.rt.block_on(async {
            // We need a scope so that self doesn't have to be moved
            let (_, outputs) = TokioScope::scope_and_block(|s| {
                for module in self.modules.iter_mut() {
                    // Spawn the provided function within the scope
                    s.spawn(f(module));
                }
            });
            // Unwrap the Result<O, JoinError>
            outputs.into_iter().map(|t| t.unwrap()).collect::<Vec<O>>()
        })
    }

    /// Load a SupMCU master from a definition file instead of discovering modules.
    pub fn load_def_file(&mut self, file: &Path) -> Result<(), SupMCUError> {
        let defs: Vec<SupMCUModuleDefinition> = serde_json::from_reader(File::open(file)?)?;
        for (def, module) in defs.into_iter().zip(self.modules.iter_mut()) {
            module.set_definition(def);
        }
        self.def_file = Some(file.to_path_buf());
        Ok(())
    }

    /// Save the modules definitions to a definition file
    pub fn save_def_file<P: AsRef<Path>>(&self, file: P) -> Result<(), SupMCUError> {
        let file = File::create(&file)?;
        serde_json::to_writer(file, &self.get_definitions()?).unwrap();
        Ok(())
    }
}

impl SupMCUMaster<LinuxI2CDevice> {
    /// Uses single byte reads to determine what addresses on the bus are populated.
    ///
    /// Checks addresses between 0x03 and 0x77, inclusive.a
    pub fn scan_bus(
        device: &str,
        blacklist: Option<Vec<u16>>,
    ) -> Result<Vec<u16>, SupMCUError> {
        debug!("scanning I2C bus");
        let address = 0x03;
        let mut dev = LinuxI2CDevice::new(device, address).map_err(|error| {
            SupMCUError::I2CDevError {
                device: String::from(device),
                address,
                error,
            }
        })?;
        let mut addresses = vec![];

        for i in 0x03..0x78 {
            trace!("checking address 0x{i:x}");
            if dev.set_slave_address(i).is_err() {
                error!("failed to set address 0x{i:x}");
                continue;
            }
            if dev.smbus_read_byte().is_ok() {
                debug!("found valid address 0x{i:x}");
                if let Some(blacklist) = &blacklist {
                    if let Err(_idx) = blacklist.binary_search(&i) {
                        addresses.push(i);
                    } else {
                        debug!("skipping blacklisted address 0x{i:x}");
                    }
                } else {
                    addresses.push(i);
                }
            }
        }
        Ok(addresses)
    }

    fn new_ext<S: AsRef<str>>(
        device: S,
        max_retries: Option<u8>,
        addresses: Option<Vec<u16>>,
        blacklist: Option<Vec<u16>>,
    ) -> Result<Self, SupMCUError> {
        let device = device.as_ref();
        let addresses = if let Some(addrs) = addresses {
            addrs
        } else {
            SupMCUMaster::scan_bus(device, blacklist)?
        };
        Ok(SupMCUMaster {
            modules: addresses
                .into_iter()
                .map(|addr| SupMCUModule::new(device, addr, max_retries))
                .collect::<Result<Vec<SupMCUModule<LinuxI2CDevice>>, SupMCUError>>()?,
            def_file: None,
            rt: runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
        })
    }

    /// Initialize a SupMCUMaster with empty SupMCUModules, usually followed by discovery.
    pub fn new<S: AsRef<str>>(
        device: S,
        blacklist: Option<Vec<u16>>,
    ) -> Result<Self, SupMCUError> {
        SupMCUMaster::new_ext(device, Some(DEFAULT_RETRIES), None, blacklist)
    }

    /// Initialize a SupMCUMaster, specifying addresses of modules to interact with
    pub fn new_with_addrs<S: AsRef<str>>(
        device: S,
        addresses: Vec<u16>,
    ) -> Result<Self, SupMCUError> {
        SupMCUMaster::new_ext(device, Some(DEFAULT_RETRIES), Some(addresses), None)
    }

    /// Initialize a SupMCUMaster with modules definitions that have been saved to disk
    pub fn new_from_file<S: AsRef<str>, P: AsRef<Path>>(
            device: S,
            file: P,
        ) -> Result<Self, SupMCUError> {
        let def_file = Some(PathBuf::from(file.as_ref()));
        let defs: Vec<SupMCUModuleDefinition> = serde_json::from_reader(File::open(file)?)?;
        let modules = defs
            .into_iter()
            .map(|d| SupMCUModule::new_from_def(device.as_ref(), None, d).unwrap())
            .collect();
        Ok(SupMCUMaster {
            modules,
            def_file,
            rt: runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,

        })
    }

    /// Initialize a SupMCUMaster without allowing any attempts to retry telemetry requests
    /// that return non-ready responses.
    pub fn new_no_retries<S: AsRef<str>>(device: S) -> Result<Self, SupMCUError> {
        SupMCUMaster::new_ext(device, None, None, None)
    }
}

#[cfg(test)]
mod test {

    use i2c::TestI2CDevice;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;

    use super::*;

    impl SupMCUModule<TestI2CDevice> {
        pub fn new_test(
            rng: SmallRng,
            def: SupMCUModuleDefinition,
            nonreadys: bool,
            max_retries: Option<u8>,
        ) -> Result<Self, SupMCUError> {
            Ok(SupMCUModule {
                i2c_dev: Box::new(TestI2CDevice::new(rng, def, nonreadys)),
                last_cmd: "".into(),
                definition: None,
                max_retries,
                address: 0,
            })
        }

        pub fn update_def(&mut self) {
            self.i2c_dev.definition = self.definition.clone().unwrap();
        }
    }

    impl SupMCUMaster<TestI2CDevice> {
        pub fn new_test(
            rng: SmallRng,
            nonreadys: bool,
            max_retries: Option<u8>,
        ) -> Result<Self, SupMCUError> {
            let defs: Vec<SupMCUModuleDefinition> =
                serde_json::from_reader(File::open(Path::new("test-definition.json"))?)?;

            Ok(SupMCUMaster {
                modules: defs
                    .into_iter()
                    .map(|def| {
                        SupMCUModule::new_test(rng.clone(), def, nonreadys, max_retries)
                    })
                    .collect::<Result<Vec<SupMCUModule<TestI2CDevice>>, SupMCUError>>()?,
                def_file: None,
                rt: runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()?,
            })
        }
    }

    #[test]
    fn discover_module() {
        let rng = SmallRng::from_entropy();

        SupMCUMaster::new_test(rng, true, Some(5))
            .unwrap()
            .discover_modules()
            .unwrap();
    }

    /// This test should panic, but there is a small chance that it won't (causing the test to fail) because the
    /// module returns non-ready responses randomly. Try to have larger modules in the `test_definition.json` file,
    /// to decrease the chance of this happening.  
    #[test]
    #[should_panic]
    fn nonready_no_retry() {
        let rng = SmallRng::from_entropy();

        SupMCUMaster::new_test(rng, true, None)
            .unwrap()
            .discover_modules()
            .unwrap();
    }

    #[test]
    fn get_telemetry_values() {
        // Telemetry values are generated from this rng
        let rng = SmallRng::from_entropy();
        let mut master = SupMCUMaster::new_test(rng.clone(), false, Some(5)).unwrap();
        master
            .load_def_file(Path::new("test-definition.json"))
            .unwrap();
        for module in master.modules.iter_mut() {
            // rng needs to be cloned so that each module starts with a "fresh"/unused rng initialized from the same seed
            let mut local_rng = rng.clone();
            for tel_def in module
                .get_definition_mut()
                .unwrap()
                .telemetry
                .clone()
                .iter_mut()
            {
                // Skip telemetry items that have special purposes
                if tel_def.telemetry_type == TelemetryType::SupMCU
                    && (tel_def.idx == 0 || tel_def.idx == 14 || tel_def.idx == 17 || tel_def.idx ==19)
                {
                    continue;
                }
                assert_eq!(
                    // Because both functions are using the exact same rng, the numbers generated should be the same
                    module.get_telemetry_by_def(tel_def).unwrap().data,
                    tel_def.format.random_data(&mut local_rng)
                );
            }
        }
    }

    /// tests saving and loading of a bus definition
    #[test]
    fn save_load_defs() {
        let tmp_path = "test-definition.tmp";
        let rng = SmallRng::from_entropy();
        let mut master = SupMCUMaster::new_test(rng.clone(), false, Some(5)).unwrap();
        master
            .load_def_file(Path::new("test-definition.json"))
            .unwrap();
        master.save_def_file(Path::new(tmp_path)).unwrap();
        let mut reload_master = SupMCUMaster::new_test(rng, false, Some(5)).unwrap();
        match reload_master.load_def_file(Path::new(tmp_path)) {
            Ok(m) => m,
            Err(e) => {
                std::fs::remove_file(tmp_path).unwrap();
                panic!("{}", e);
            }
        };
        std::fs::remove_file(tmp_path).unwrap();
        assert_eq!(
            master.get_definitions().unwrap(),
            reload_master.get_definitions().unwrap(),
        );
    }
}
