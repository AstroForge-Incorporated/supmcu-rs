use byteorder::{WriteBytesExt, LE};
use std::{fs::File, io::Cursor, path::Path};
use supmcu_rs::supmcu::parsing::*;

#[test]
fn create_all_data_types() {
    assert_eq!(DataType::Str, DataType::try_from('S').unwrap());
    assert_eq!(DataType::Char, DataType::try_from('c').unwrap());
    assert_eq!(DataType::UINT8, DataType::try_from('u').unwrap());
    assert_eq!(DataType::INT8, DataType::try_from('t').unwrap());
    assert_eq!(DataType::UINT16, DataType::try_from('s').unwrap());
    assert_eq!(DataType::INT16, DataType::try_from('n').unwrap());
    assert_eq!(DataType::UINT32, DataType::try_from('i').unwrap());
    assert_eq!(DataType::INT32, DataType::try_from('d').unwrap());
    assert_eq!(DataType::UINT64, DataType::try_from('l').unwrap());
    assert_eq!(DataType::INT64, DataType::try_from('k').unwrap());
    assert_eq!(DataType::Float, DataType::try_from('f').unwrap());
    assert_eq!(DataType::Double, DataType::try_from('F').unwrap());
    assert_eq!(DataType::Hex8, DataType::try_from('x').unwrap());
    assert_eq!(DataType::Hex8, DataType::try_from('X').unwrap());
    assert_eq!(DataType::Hex16, DataType::try_from('z').unwrap());
    assert_eq!(DataType::Hex16, DataType::try_from('Z').unwrap());
}

#[test]
#[should_panic]
fn create_invalid_data_type() {
    DataType::try_from('h').unwrap();
}

#[test]
fn getting_format_string() {
    assert_eq!("fun", SupMCUFormat::new("f,u. o\\n").get_format_str());
}

#[test]
fn format_length() {
    // char + int8 + uint16 + int32 + double + hex16
    //   1  +   1  +    2   +   4   +    8   +   2   = 18
    assert_eq!(18, SupMCUFormat::new("ctsdFz").get_byte_length().unwrap());
}

#[test]
#[should_panic]
fn invalid_format_length() {
    SupMCUFormat::new("lfS").get_byte_length().unwrap();
}

#[test]
fn parse_numbers() {
    let mut wtr = vec![];
    wtr.write_i16::<LE>(-1234).unwrap();
    wtr.write_u8(55).unwrap();
    wtr.write_f32::<LE>(std::f32::consts::PI).unwrap();
    wtr.write_u8(0x32).unwrap();

    let output = vec![
        SupMCUValue::I16(-1234),
        SupMCUValue::U8(55),
        SupMCUValue::Float(std::f32::consts::PI),
        SupMCUValue::Hex8(0x32),
    ];
    assert_eq!(
        output,
        SupMCUFormat::new("nufx")
            .parse_data(&mut Cursor::new(&wtr))
            .unwrap()
    );
}

#[test]
fn parse_string() {
    let s = String::from("Hello World!");
    let mut data = s.clone().into_bytes();
    data.push(0); // Adding null terminator

    let mut data2 = vec![];

    #[allow(clippy::char_lit_as_u8)]
    data2.write_u8('j' as u8).unwrap();

    data.append(&mut data2);
    let output = vec![SupMCUValue::Str(s), SupMCUValue::Char('j')];
    assert_eq!(
        output,
        SupMCUFormat::new("Sc")
            .parse_data(&mut Cursor::new(&data))
            .unwrap()
    );
}

#[test]
fn value_to_string() {
    assert_eq!("123456", SupMCUValue::U32(123456).to_string());
    assert_eq!("1.23456", SupMCUValue::Double(1.23456).to_string());
    assert_eq!("0x12", SupMCUValue::Hex8(0x12).to_string());
    assert_eq!(
        "Hello World!",
        SupMCUValue::Str("Hello World!".into()).to_string()
    );
    assert_eq!("j", SupMCUValue::Char('j').to_string());
}

#[test]
fn load_definition() {
    let _defs: Vec<SupMCUModuleDefinition> =
        serde_json::from_reader(File::open(Path::new("test-definition.json")).unwrap())
            .unwrap();
}
