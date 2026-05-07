use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Write embassy-boot partition symbols as a standalone linker script.
    // This is added via -T (linker arg) so it is always included regardless of
    // which memory.x the workspace picks up.
    //
    // Partition layout (2 MB STM32H743XI / H753ZI):
    //   BOOTLOADER  : 0x08000000  128 KB  (Bank1 sector 0)  — bootloader code
    //   ACTIVE      : 0x08020000  896 KB  (Bank1 sectors 1–7)
    //   STATE       : 0x08100000  128 KB  (Bank2 sector 0)
    //   DFU         : 0x08120000  896 KB  (Bank2 sectors 1–7)
    //
    // Symbols are offsets from the start of FLASH (0x08000000).
    fs::write(
        out.join("bootloader-partitions.x"),
        b"__bootloader_active_start = 0x00020000;\n\
          __bootloader_active_end   = 0x00100000;\n\
          __bootloader_state_start  = 0x00100000;\n\
          __bootloader_state_end    = 0x00120000;\n\
          __bootloader_dfu_start    = 0x00120000;\n\
          __bootloader_dfu_end      = 0x00200000;\n",
    )
    .expect("cannot write bootloader-partitions.x");

    println!(
        "cargo:rustc-link-arg=-T{}",
        out.join("bootloader-partitions.x").display()
    );

    println!("cargo:rerun-if-changed=build.rs");
}
