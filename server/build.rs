use std::env;

fn main() {
    // libwebrtc's Objective-C categories are dropped unless the binary links with -ObjC.
    if env::var("CARGO_CFG_TARGET_OS").unwrap() == "macos" {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
