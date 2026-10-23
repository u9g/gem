use std::{env, path::PathBuf};

fn main() {
    let descriptors = PathBuf::from(env::var("OUT_DIR").unwrap()).join("descriptors.bin");

    prost_build::Config::new()
        .file_descriptor_set_path(&descriptors)
        .compile_protos(&["../protocol/gem.proto"], &["../protocol"])
        .unwrap();

    // pbjson emits the canonical proto3 JSON mapping, which the frontend's protobuf-es decodes.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(descriptors).unwrap())
        .unwrap()
        .build(&[".gem"])
        .unwrap();
}
