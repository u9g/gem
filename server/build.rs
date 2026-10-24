use std::{env, path::PathBuf};

fn main() {
    // libwebrtc's Objective-C categories are dropped unless the binary links with -ObjC.
    if env::var("CARGO_CFG_TARGET_OS").unwrap() == "macos" {
        println!("cargo:rustc-link-arg=-ObjC");
    }

    let descriptors = PathBuf::from(env::var("OUT_DIR").unwrap()).join("descriptors.bin");

    prost_build::Config::new()
        .file_descriptor_set_path(&descriptors)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types")
        .compile_protos(&["../protocol/gem.proto"], &["../protocol"])
        .unwrap();

    // pbjson emits the canonical proto3 JSON mapping, which the frontend's protobuf-es decodes.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(descriptors).unwrap())
        .unwrap()
        .build(&[".gem"])
        .unwrap();
}
