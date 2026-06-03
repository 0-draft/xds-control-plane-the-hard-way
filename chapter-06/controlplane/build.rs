// Generate the OpenRcaService *client* and the OrcaLoadReport message. The
// control plane is the out-of-band ORCA client here: it streams load reports
// from each backend (Envoy itself does not consume ORCA), so we build the
// client side only.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .include_file("orca.rs")
        .compile_protos(&["proto/orca.proto"], &["proto"])?;
    Ok(())
}
