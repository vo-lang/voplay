use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let schema_path = PathBuf::from("../../../protocol/voplay.schema.toml");
    println!("cargo:rerun-if-changed={}", schema_path.display());
    let source = fs::read_to_string(&schema_path).expect("read Voplay protocol schema");
    let schema = vo_schema_compiler::compile_framework_schema(&source, "voplay.engine")
        .expect("compile Voplay protocol schema");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(output.join("generated.rs"), schema.render_rust())
        .expect("write generated Voplay protocol Rust definitions");
}
