//! Build script that compiles the gRPC protobuf definitions via `tonic-prost-build`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/pcx.proto")?;
    Ok(())
}
