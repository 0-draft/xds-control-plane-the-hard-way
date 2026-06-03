// Generate the OpenRcaService server and the OrcaLoadReport message from the
// vendored protos. tonic-build shells out to protoc, which the Dockerfile
// build stage installs (protobuf-compiler) and which supplies the well-known
// google/protobuf/duration.proto import.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(false)
        .include_file("orca.rs")
        .compile_protos(&["proto/orca.proto"], &["proto"])?;
    Ok(())
}
