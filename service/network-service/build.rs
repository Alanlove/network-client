//! Build script: compile the frozen IPC contract `shared/proto/network.proto`.
//! protoc is provided by `protoc-bin-vendored`, so no system-wide protoc is
//! required on the developer machine.

use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let proto = PathBuf::from("../../shared/proto/network.proto");
    let include = PathBuf::from("../../shared/proto");

    println!(
        "cargo:rerun-if-changed={}",
        proto.display()
    );

    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap());
    prost_build::compile_protos(&[proto], &[include])
        .expect("prost-build failed to compile network.proto");
    Ok(())
}
