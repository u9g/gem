use std::{env, path::PathBuf};

fn main() {
    let descriptors = PathBuf::from(env::var("OUT_DIR").unwrap()).join("descriptors.bin");
    // Outside this crate, so Cargo would not otherwise rebuild when it changes.
    println!("cargo:rerun-if-changed=../../../protocol/gem.proto");

    prost_build::Config::new()
        .file_descriptor_set_path(&descriptors)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types")
        .compile_protos(&["../../../protocol/gem.proto"], &["../../../protocol"])
        .unwrap();

    // pbjson emits the canonical proto3 JSON mapping, which the frontend's protobuf-es decodes.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(descriptors).unwrap())
        .unwrap()
        .build(&[".gem"])
        .unwrap();
}
