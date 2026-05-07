use std::env;
use std::fs;
use std::path::PathBuf;

/// Extract (major, minor, patch) from the nearest git tag matching `v*`.
/// Falls back to Cargo.toml when git is unavailable or no tag exists.
fn version_from_git() -> Option<(u8, u8, u8)> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--match", "v*", "--abbrev=0"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let tag = String::from_utf8(out.stdout).ok()?;
    let tag = tag.trim().trim_start_matches('v');
    let mut p = tag.splitn(3, '.');
    let maj: u8 = p.next()?.parse().ok()?;
    let min: u8 = p.next()?.parse().ok()?;
    let pat: u8 = p.next()?.parse().ok()?;
    Some((maj, min, pat))
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // ── Bootloader version constants ──────────────────────────────────────────
    // Prefer the nearest git tag (vX.Y.Z → exact release triple);
    // fall back to Cargo.toml when git is unavailable or no tag exists.
    let cargo_ver = env::var("CARGO_PKG_VERSION").unwrap();
    let mut cargo_parts = cargo_ver.splitn(3, '.');
    let cargo_maj: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let cargo_min: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let cargo_pat: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let (bl_maj, bl_min, bl_pat) = version_from_git().unwrap_or((cargo_maj, cargo_min, cargo_pat));
    fs::write(
        out.join("version_consts.rs"),
        format!(
            "pub const BL_MAJ: u8 = {bl_maj};\n\
             pub const BL_MIN: u8 = {bl_min};\n\
             pub const BL_PAT: u8 = {bl_pat};\n"
        ),
    )
    .expect("cannot write version_consts.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Copy memory.x to OUT_DIR so cortex-m-rt's link.x finds it via
    // INCLUDE memory.x (OUT_DIR is added to the linker search path below).
    let memory_x = fs::read(manifest.join("memory.x")).expect("cannot read memory.x");
    fs::write(out.join("memory.x"), memory_x).expect("cannot write memory.x");
    println!("cargo:rustc-link-search={}", out.display());

    // The workspace-level firmware/memory.x takes precedence for cortex-m-rt's
    // INCLUDE chain, so embassy-boot's __bootloader_* symbols (which ARE in
    // our memory.x but may be shadowed) are provided via a separate linker
    // script passed explicitly with -T.
    let partitions = out.join("bootloader-partitions.x");
    fs::write(
        &partitions,
        "/* embassy-boot partition symbols — offsets from 0x08000000 */\n\
         __bootloader_active_start = 0x00020000;\n\
         __bootloader_active_end   = 0x000E0000;\n\
         __bootloader_state_start  = 0x00100000;\n\
         __bootloader_state_end    = 0x00120000;\n\
         __bootloader_dfu_start    = 0x00120000;\n\
         __bootloader_dfu_end      = 0x00200000;\n",
    )
    .expect("cannot write bootloader-partitions.x");
    println!("cargo:rustc-link-arg=-T{}", partitions.display());

    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
