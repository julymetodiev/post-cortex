// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The messages, clients and the helper's server of the protos the
//! bindings' test runs: the greet example's and the relay fixture's.

/// The greet example's protos, outside this package: cargo runs this again
/// on their change only when it is told to.
const GREET: &str = "../../examples/greet/proto";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    println!("cargo:rerun-if-changed={GREET}");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("relay_descriptor.bin"))
        .compile_protos(&["proto/relay/v1/relay.proto"], &["proto"])?;
    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(
            &[format!("{GREET}/greet/v1/greet.proto")],
            &[GREET.to_string()],
        )?;
    Ok(())
}
