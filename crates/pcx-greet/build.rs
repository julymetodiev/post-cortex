//! Build script: the module's package, `example.greet.v1`, and its
//! descriptors, which its manifest carries (with the module API's protos
//! it imports, for its settings' option).

/// The directories of the protos it compiles: its own and the module
/// API's, which lies outside this package, so cargo runs this again on its
/// change only when it is told to (found 2026-11-14: greet carried the
/// module API of 2026-11-01 and its tools read no other module's messages).
const PROTOS: [&str; 2] = ["proto", "../post-cortex-proto/proto"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for dir in PROTOS {
        println!("cargo:rerun-if-changed={dir}");
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("greet_descriptor.bin"))
        .compile_protos(&["proto/greet.proto"], &PROTOS)?;
    Ok(())
}
