//! Compiles the vendored gRPC contract schemas (`proto/`) into tonic/prost types.
//!
//! The `proto/` directory is a verbatim copy from the service repository (see
//! `proto/README.md` for the pinned source commit and file hashes); this build
//! script only compiles it. The generated code is included by `src/grpc.rs`.

use std::{env, path::PathBuf};

/// Vendored contract schemas, compiled in a stable order.
const PROTO_FILES: &[&str] = &["embed/v1/embed.proto", "google/rpc/error_details.proto"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    // No canonicalize(): the UNC prefix Windows produces breaks vendored protoc paths.
    let proto_root = manifest.join("proto");
    for file in PROTO_FILES {
        println!("cargo:rerun-if-changed={}", proto_root.join(file).display());
    }

    // Edition 2024 makes set_var unsafe: mutate the build environment before the
    // single codegen read; there are no concurrent readers to race with.
    unsafe {
        env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }

    let files: Vec<PathBuf> = PROTO_FILES.iter().map(|f| proto_root.join(f)).collect();
    tonic_prost_build::configure().compile_protos(&files, &[proto_root])?;
    Ok(())
}
