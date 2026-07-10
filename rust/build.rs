use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let source_path = manifest_dir.join("../draw_protocol.vo");
    println!("cargo:rerun-if-changed={}", source_path.display());
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let values = parse_constants(&source);
    validate_constants(&values);
    let generated = generate_rust(&values);
    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"))
        .join("draw_protocol_generated.rs");
    fs::write(output, generated).expect("failed to write generated draw protocol");
}

fn validate_constants(values: &BTreeMap<String, u32>) {
    for name in [
        "drawStreamMagic0",
        "drawStreamMagic1",
        "drawStreamMagic2",
        "drawStreamMagic3",
        "drawStreamVersion",
        "drawStreamFlags",
        "DrawStreamHeaderSize",
        "drawStreamPayloadLengthOffset",
    ] {
        value(values, name);
    }
    for index in 0..4 {
        let name = format!("drawStreamMagic{index}");
        assert!(
            value(values, &name) <= u8::MAX as u32,
            "{name} must fit in one byte"
        );
    }
    assert!(
        value(values, "drawStreamVersion") <= u16::MAX as u32,
        "drawStreamVersion must fit in u16"
    );
    assert!(
        value(values, "drawStreamFlags") <= u16::MAX as u32,
        "drawStreamFlags must fit in u16"
    );
    let header_size = value(values, "DrawStreamHeaderSize");
    let payload_offset = value(values, "drawStreamPayloadLengthOffset");
    assert_eq!(
        header_size, 12,
        "draw stream header layout changed without generator support"
    );
    assert!(
        payload_offset.saturating_add(4) <= header_size,
        "payload length field must fit inside the draw stream header"
    );
    let mut opcodes = BTreeSet::new();
    for (name, opcode) in values.iter().filter(|(name, _)| name.starts_with("op")) {
        assert!(name.len() > 2, "opcode name must include an enum variant");
        assert!(
            *opcode <= u8::MAX as u32,
            "opcode {name} must fit in one byte"
        );
        assert!(
            opcodes.insert(*opcode),
            "duplicate draw opcode 0x{opcode:02X}"
        );
    }
    assert!(
        !opcodes.is_empty(),
        "draw protocol must define at least one opcode"
    );
}

fn parse_constants(source: &str) -> BTreeMap<String, u32> {
    let mut values = BTreeMap::new();
    for raw_line in source.lines() {
        let line = raw_line.trim();
        let Some((name, raw_value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let raw_value = raw_value.trim();
        if !(name.starts_with("op")
            || name.starts_with("drawStream")
            || name == "DrawStreamHeaderSize")
        {
            continue;
        }
        let value = if let Some(hex) = raw_value.strip_prefix("0x") {
            u32::from_str_radix(hex, 16)
        } else {
            raw_value.parse()
        }
        .unwrap_or_else(|_| panic!("invalid draw protocol value for {name}: {raw_value}"));
        values.insert(name.to_string(), value);
    }
    values
}

fn value(values: &BTreeMap<String, u32>, name: &str) -> u32 {
    *values
        .get(name)
        .unwrap_or_else(|| panic!("draw protocol is missing {name}"))
}

fn generate_rust(values: &BTreeMap<String, u32>) -> String {
    let mut output = String::from(
        "// @generated from ../draw_protocol.vo by rust/build.rs\n\
         pub const DRAW_STREAM_MAGIC: [u8; 4] = [",
    );
    for index in 0..4 {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&format!(
            "0x{:02X}",
            value(values, &format!("drawStreamMagic{index}"))
        ));
    }
    output.push_str("];\n");
    output.push_str(&format!(
        "pub const DRAW_STREAM_VERSION: u16 = {};\n",
        value(values, "drawStreamVersion")
    ));
    output.push_str(&format!(
        "pub const DRAW_STREAM_FLAGS: u16 = {};\n",
        value(values, "drawStreamFlags")
    ));
    output.push_str(&format!(
        "pub const DRAW_STREAM_HEADER_SIZE: usize = {};\n\n",
        value(values, "DrawStreamHeaderSize")
    ));
    output
        .push_str("#[repr(u8)]\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Opcode {\n");
    for (name, opcode) in values.iter().filter(|(name, _)| name.starts_with("op")) {
        output.push_str(&format!("    {} = 0x{opcode:02X},\n", &name[2..]));
    }
    output.push_str("}\n\nimpl Opcode {\n    pub fn from_u8(value: u8) -> Option<Self> {\n        match value {\n");
    for (name, opcode) in values.iter().filter(|(name, _)| name.starts_with("op")) {
        output.push_str(&format!(
            "            0x{opcode:02X} => Some(Self::{}),\n",
            &name[2..]
        ));
    }
    output.push_str("            _ => None,\n        }\n    }\n}\n");
    output
}
