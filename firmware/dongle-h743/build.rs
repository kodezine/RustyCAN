// build.rs — copies memory.x into the linker search path and emits
// embassy-boot partition symbols required by FirmwareUpdaterConfig.
use std::env;
use std::fs;
use std::path::PathBuf;

/// Extract (major, minor, patch) from the nearest git tag matching `v*`.
/// The tag must be exactly `vX.Y.Z`; any describe suffix (commits, dirty) is
/// ignored so the version shown on the LCD is always the release triple.
/// Falls back to Cargo.toml when git is unavailable or no tag exists.
fn version_from_git() -> Option<(u8, u8, u8)> {
    // --abbrev=0 returns the tag name only, no commit hash suffix.
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
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // ── Firmware version constants ──────────────────────────────────────────
    // Prefer the nearest git tag (vX.Y.Z → exact release triple);
    // fall back to Cargo.toml when git is unavailable or no tag exists.
    let cargo_ver = env::var("CARGO_PKG_VERSION").unwrap();
    let mut cargo_parts = cargo_ver.splitn(3, '.');
    let cargo_maj: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let cargo_min: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let cargo_pat: u8 = cargo_parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let (fw_maj, fw_min, fw_pat) = version_from_git().unwrap_or((cargo_maj, cargo_min, cargo_pat));
    fs::write(
        out.join("version_consts.rs"),
        format!(
            "/// Firmware major version.\n\
             pub const FW_MAJ: u8 = {fw_maj};\n\
             /// Firmware minor version.\n\
             pub const FW_MIN: u8 = {fw_min};\n\
             /// Firmware patch version.\n\
             pub const FW_PAT: u8 = {fw_pat};\n"
        ),
    )
    .expect("cannot write version_consts.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // ── Fixed-field version display string ────────────────────────────────
    // Format: v003.005.001 — always 3 digits per component, leading zeros.
    // Used on the LCD header row alongside the BL version.
    println!(
        "cargo:rustc-env=FW_VERSION_DISPLAY=v{:03}.{:03}.{:03}",
        fw_maj, fw_min, fw_pat
    );

    // Use the per-crate memory.x (FLASH at 0x08020000, 896 KB) so the app
    // links at the correct address for the embassy-boot bootloader.
    let memory_x_src = manifest_dir.join("memory.x");
    fs::copy(&memory_x_src, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // Partition symbols for embassy-boot: FirmwareUpdaterConfig::from_linkerfile_blocking
    // uses these to locate the STATE and DFU flash regions at runtime.
    let partitions = out.join("bootloader-partitions.x");
    fs::write(
        &partitions,
        "/* embassy-boot partition symbols \u{2014} offsets from 0x08000000 */\n\
         __bootloader_active_start = 0x00020000;\n\
         __bootloader_active_end   = 0x00100000;\n\
         __bootloader_state_start  = 0x00100000;\n\
         __bootloader_state_end    = 0x00120000;\n\
         __bootloader_dfu_start    = 0x00120000;\n\
         __bootloader_dfu_end      = 0x00200000;\n",
    )
    .expect("cannot write bootloader-partitions.x");
    println!("cargo:rustc-link-arg=-T{}", partitions.display());

    println!("cargo:rerun-if-changed={}", memory_x_src.display());
    println!("cargo:rerun-if-changed=build.rs");
}
