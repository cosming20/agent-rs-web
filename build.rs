// Compile agent.proto (vendored via the proto/agent-rs-proto git
// submodule) into generated Rust modules available as `crate::pb`.
//
// Client-only: the web layer never hosts the gRPC service itself.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = "proto/agent-rs-proto/proto";
    let proto_file = format!("{}/agent.proto", proto_root);

    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[&proto_file], &[proto_root])?;

    println!("cargo:rerun-if-changed={}", proto_file);
    Ok(())
}
