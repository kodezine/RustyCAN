use std::process::Command;

fn main() {
    // Re-run this script if HEAD or any tag changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let version = git_describe().unwrap_or_else(|| {
        // Fallback to Cargo.toml version when git is unavailable.
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into())
    });

    println!("cargo:rustc-env=RUSTYCAN_VERSION={version}");

    // On macOS, embed /usr/local/lib as an LC_RPATH entry in the binary.
    // This ensures dlopen("libPCBUSB.dylib") resolves correctly regardless of
    // how the app is launched (terminal, Finder, .app bundle) — even when
    // DYLD_LIBRARY_PATH / DYLD_FALLBACK_LIBRARY_PATH are stripped by the OS.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-rpath");
        println!("cargo:rustc-link-arg=/usr/local/lib");
    }
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;

    if output.status.success() {
        let s = String::from_utf8(output.stdout).ok()?;
        Some(s.trim().to_string())
    } else {
        None
    }
}
