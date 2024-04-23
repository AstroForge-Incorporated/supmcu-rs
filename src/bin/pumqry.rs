/*!
# PumQry

This utility is an evolution of the previous PumQry utility that was a part of [PuTDIG-CLI](https://github.com/PumpkinSpace/PuTDIG-CLI).
It is significantly faster for two main reasons: it is written in rust, and the unerlying library
can discover and get telemetry for different modules in parallel.

There are two subcommands, `pumqry query` and `pumqry discover`.  Query is for loading a definition
file and getting specific telemetry values from a module.  Discver is for discovering a definition
from a specific module or I2C bus.

## Examples
Querying the data of a SupMCU telemetry item called "Firmware version" from a module at addres 0x52.
```bash
$ pumqry -p /dev/i2c-1 query -d def.json -m 0x52 -v "Firmware version" -s supmcu
```

Discovering definitions for all the modules on the I2C bus and saving them to a file, formatted to be human readable.
```bash
$ pumqry -p /dev/i2c-1 discover -dq -f def.json
```

Discovering a definition for a single module at address 0x52.
```bash
$ pumqry -p /dev/i2c-1 discover -f def.json 0x52
```


```bash
$ pumqry --help
pumpkijn_supmcu-rs 0.1.0
Jack Hughes <jack.hughes@pumpkininc.com>

USAGE:
    pumqry [OPTIONS] --path <DEVICE> <SUBCOMMAND>

OPTIONS:
    -h, --help                         Print help information
    -p, --path <DEVICE>                Path for I2C device, e.g. /dev/i2c-1
    -t, --device-type <DEVICE_TYPE>    Type of I2C device at the specified port. DEPRECATED - Only
                                       kubos/linux type is currently supported [default: linux]
                                       [possible values: i2c-driver, aardvark, linux, kubos]
    -V, --version                      Print version information

SUBCOMMANDS:
    discover    Discover the telemetry/commands and query data from any Pumpkin SupMCU modules
                    on a particular I2C bus
    help        Print this message or the help of the given subcommand(s)
    query       Query individual telemetry valus from any Pumpkin SupMCU module with a premade
                    definition file
```

```bash
$ pumqry query --help
pumqry-query
Query individual telemetry valus from any Pumpkin SupMCU module with a premade definition file

Example: pumqry -p /dev/i2c-1 query -d def.json -m 0x52 -v "Firmware version" -s supmcu

USAGE:
    pumqry --path <DEVICE> query --definition <DEFINITION> --module <MODULE> --value <VALUE> --telemetry-type <TELEMETRY_TYPE>

OPTIONS:
    -d, --definition <DEFINITION>
            The definition file to load

    -h, --help
            Print help information

    -m, --module <MODULE>
            Them module name or I2C address to pull telemetry from

    -s, --telemetry-type <TELEMETRY_TYPE>
            The type of telemetry to pull, either SupMCU or Module

            [possible values: supmcu, module]

    -v, --value <VALUE>
            Value to pull out of the module
```


```bash
$ pumqry discover --help
pumqry-discover
Discover the telemetry/commands and query data from any Pumpkin SupMCU modules on a particular I2C
bus.

Example: pumqry -p /dev/i2c-1 discover -dq -f def.json

USAGE:
    pumqry --path <DEVICE> discover [OPTIONS] [I2C ADDRESSES]...

ARGS:
    <I2C ADDRESSES>...


OPTIONS:
    -d, --pretty
            Format the JSON output

    -f, --file <FILE>
            The file to save JSON data to

    -h, --help
            Print help information

    -l, --list
            List all of the available i2c addresses without getting telemetry data

    -q, --quiet
            Runs without outputing anything to stdout
```
*/

use clap::{Args, Parser, Subcommand};
use flexi_logger::Logger;
use std::path::PathBuf;
use supmcu_rs::supmcu::{parsing, SupMCUMaster};
use log::debug;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct PumQry {
    #[clap(subcommand)]
    command: Commands,
    /// Path for I2C device, e.g. /dev/i2c-1
    #[clap(short, long, parse(from_os_str), value_name = "DEVICE")]
    path: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Discover(DiscoveryArgs),
    Query(QueryArgs),
}

/// Discover the telemetry/commands and query data from any Pumpkin SupMCU modules on a particular I2C bus.
///
/// Example: pumqry -p /dev/i2c-1 discover -dq -f def.json
#[derive(Args, Debug)]
struct DiscoveryArgs {
    /// The file to save JSON data to.
    #[clap(short, long, parse(from_os_str), value_name = "FILE")]
    file: Option<PathBuf>,
    /// Runs without outputing anything to stdout.
    #[clap(short, long)]
    quiet: bool,
    /// Format the JSON output.
    #[clap(short = 'd', long)]
    pretty: bool,
    /// List all of the available i2c addresses without getting telemetry data.
    #[clap(short, long)]
    list: bool,
    /// I2C address(es) to ignore
    #[clap(short, long, value_parser = parse_hex, value_name = "I2C ADDRESS TO IGNORE")]
    blacklist: Vec<u16>,
    /// I2C address(es) of module(s) to read from
    #[clap(value_parser = parse_hex, value_name = "I2C ADDRESSES")]
    addrs: Vec<u16>,
}

/// An enum of the two different ways to specify a module
#[derive(Clone, Debug, PartialEq)]
enum ModuleOption {
    Name(String),
    Address(u16),
}

/// An enum of the two different ways to specify a telemetry item
#[derive(Clone, Debug, PartialEq)]
enum TelemetryOption {
    Name(String),
    Index(usize),
}

/// Query individual telemetry valus from any Pumpkin SupMCU module with a premade definition file
///
/// Example: pumqry -p /dev/i2c-1 query -d def.json -m 0x52 -v "Firmware version" -s supmcu
#[derive(Args, Debug)]
struct QueryArgs {
    /// The definition file to load.
    #[clap(short, long)]
    definition: PathBuf,

    /// Them module name or I2C address to pull telemetry from
    #[clap(short, long, value_parser = parse_module)]
    module: ModuleOption,

    /// Value to pull out of the module.
    #[clap(short, long, value_parser = parse_tlm)]
    value: TelemetryOption,

    /// The type of telemetry to pull, either SupMCU or Module
    #[clap(short = 's', long, value_enum)]
    telemetry_type: parsing::TelemetryType,
}

fn parse_module(s: &str) -> Result<ModuleOption, String> {
    let s = s.to_string();
    if let Ok(i) = parse_hex(&s) {
        Ok(ModuleOption::Address(i))
    } else {
        Ok(ModuleOption::Name(s))
    }
}

fn parse_tlm(s: &str) -> Result<TelemetryOption, String> {
    let s = s.to_string();
    if let Ok(i) = s.parse::<usize>() {
        Ok(TelemetryOption::Index(i))
    } else {
        Ok(TelemetryOption::Name(s))
    }
}

fn parse_hex(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim_start_matches("0x"), 16)
        .map_err(|_| "Error parsing hex address".to_string())
}

fn discover(path: PathBuf, args: DiscoveryArgs) -> Result<(), anyhow::Error> {
    let device = path.to_str().unwrap();

    if args.list {
        let addrs = SupMCUMaster::scan_bus(device, None).unwrap();
        for addr in addrs {
            print!("0x{addr:x} ");
        }
        println!();
        return Ok(());
    }

    let mut master = if args.addrs.is_empty() {
        SupMCUMaster::new(device, Some(args.blacklist))
    } else {
        SupMCUMaster::new_with_addrs(device, args.addrs)
    }
    .unwrap();
    master.discover_modules().unwrap();

    if let Some(ref f) = args.file {
        master.save_def_file(f)?;
    }

    if !(args.file.is_some() && args.quiet) {
        if args.pretty {
            println!(
                "{}",
                serde_json::to_string_pretty(&master.get_definitions()?)?
            );
        } else {
            println!("{}", serde_json::to_string(&master.get_definitions()?)?);
        }
    }
    Ok(())
}

fn query(path: PathBuf, args: QueryArgs) -> Result<(), anyhow::Error> {
    let mut master = SupMCUMaster::new(path.to_str().unwrap(), None).unwrap();
    master.load_def_file(&args.definition).unwrap();
    let tlm = if let Some(module) = match &args.module {
        ModuleOption::Name(name) => master
            .modules
            .iter_mut()
            .find(|module| &module.get_definition().unwrap().name == name),
        ModuleOption::Address(addr) => master
            .modules
            .iter_mut()
            .find(|module| &module.get_address() == addr),
    } {
        let mod_def = module.get_definition().unwrap().clone();
        match args.value {
            TelemetryOption::Name(name) => {
                if let Some(tlm_def) =
                    mod_def.telemetry.iter().find(|def| def.name == name)
                {
                    module.get_telemetry_by_def(tlm_def).unwrap()
                } else {
                    panic!(
                        "Couldn't find telemetry item `{}` in {}",
                        name, mod_def.name
                    );
                }
            }
            TelemetryOption::Index(idx) => module
                .get_telemetry(args.telemetry_type, idx)
                .expect("Telemetry item not found"),
        }
    } else {
        let msg = match &args.module {
            ModuleOption::Name(name) => format!("name `{}`", name),
            ModuleOption::Address(addr) => format!("address `{}`", addr),
        };
        panic!("Cannot find module with {}", msg);
    };
    println!("{:?}", tlm.data);
    Ok(())
}

fn main() -> Result<(), anyhow::Error> {
    let args = PumQry::parse();
    Logger::try_with_str("info")?.start()?;
    debug!("{:?}", args);

    match args.command {
        Commands::Discover(discovery_args) => discover(args.path, discovery_args),
        Commands::Query(query_args) => query(args.path, query_args),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_hex_test() {
        assert_eq!(parse_hex("0x2a").unwrap(), 42);
    }

    #[test]
    fn parse_tlm_test() {
        assert_eq!(parse_tlm("5").unwrap(), TelemetryOption::Index(5));
        assert_eq!(
            parse_tlm("important number").unwrap(),
            TelemetryOption::Name("important number".into())
        );
    }

    #[test]
    fn parse_module_test() {
        assert_eq!(parse_module("0x2a").unwrap(), ModuleOption::Address(42));
        assert_eq!(
            parse_module("cool module").unwrap(),
            ModuleOption::Name("cool module".into())
        );
    }
}
