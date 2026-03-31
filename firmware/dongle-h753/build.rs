// build.rs — copies memory.x into the linker search path.
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    // CARGO_MANIFEST_DIR is the dongle-h753/ package directory;
    // memory.x lives one level up in the firmware/ workspace root.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_x = manifest_dir.join("../memory.x");
    fs::copy(&memory_x, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed={}", memory_x.display());
    println!("cargo:rerun-if-changed=build.rs");
}
