//! App self-update helpers (SOUP025).
//!
//! Queries the GitHub Releases API for a newer RustyCAN version and, depending
//! on the platform, either performs an in-place binary replacement (macOS
//! Apple Silicon) or opens the release page in the system browser
//! (Windows / Linux / Intel Mac).
//!
//! All public functions are intentionally infallible from the caller's
//! perspective: network failures, parse errors, and platform mismatches are
//! converted to `None` / logged and do not prevent normal operation.

use std::path::{Path, PathBuf};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Information about an available RustyCAN update.
#[derive(Debug, Clone)]
pub struct AppUpdateRelease {
    /// Parsed version triple from the GitHub `tag_name` field.
    pub version: (u8, u8, u8),
    /// Direct download URL for the platform-specific release asset.
    ///
    /// Empty when no downloadable asset exists for the current platform
    /// (Windows, Linux, Intel Mac) — those platforms use `release_url` instead.
    pub download_url: String,
    /// File name of the release asset (e.g. `rustycan-v0.3.0-aarch64-apple-darwin.dmg`).
    pub asset_name: String,
    /// HTML URL of the GitHub release page (`https://github.com/…/releases/tag/vX.Y.Z`).
    pub release_url: String,
}

impl AppUpdateRelease {
    /// True when in-place download + replace is supported on this platform.
    ///
    /// Only macOS Apple Silicon builds carry a `download_url`; all other
    /// platforms (Windows, Linux, Intel Mac) are link-only.
    pub fn can_download(&self) -> bool {
        !self.download_url.is_empty()
    }

    /// Human-readable version string, e.g. `"v0.3.0"`.
    pub fn version_string(&self) -> String {
        let (ma, mi, pa) = self.version;
        format!("v{ma}.{mi}.{pa}")
    }
}

/// Progress messages sent from a background download thread to the UI.
pub enum DownloadMsg {
    Progress(f32),
    Done(std::path::PathBuf),
    Err(String),
}

// ─── Version parsing ──────────────────────────────────────────────────────────

/// Parse a SemVer Git tag such as `"v1.2.3"` or `"v1.2.3-5-gabcdef"` into a
/// `(major, minor, patch)` triple.  The leading `v` and any `git describe`
/// suffix are stripped before parsing.
///
/// Returns `None` when the input cannot be interpreted as `MAJOR.MINOR.PATCH`.
pub fn parse_semver_tag(tag: &str) -> Option<(u8, u8, u8)> {
    let s = tag.trim().trim_start_matches('v');
    // Ignore any "-N-gHASH" describe suffix — keep only the vX.Y.Z base.
    let base = s.split('-').next()?;
    let mut parts = base.splitn(3, '.');
    let maj: u8 = parts.next()?.parse().ok()?;
    let min: u8 = parts.next()?.parse().ok()?;
    let pat: u8 = parts.next()?.parse().ok()?;
    Some((maj, min, pat))
}

// ─── Platform asset resolution ────────────────────────────────────────────────

/// Return the expected release-asset filename for the current platform, or
/// `None` when no downloadable asset is available (Windows, Linux, Intel Mac).
///
/// Asset naming matches `.github/workflows/release.yml`:
/// - macOS aarch64 → `rustycan-{tag}-aarch64-apple-darwin.dmg`
/// - all other targets → `None` (link-only update)
fn platform_asset_name(tag: &str) -> Option<String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some(format!("rustycan-{tag}-aarch64-apple-darwin.dmg"));

    // Windows, Linux, and Intel Mac fall through to link-only mode.
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    None
}

// ─── GitHub release check ─────────────────────────────────────────────────────

/// Query the GitHub Releases API for the latest RustyCAN release.
///
/// Returns `Some(AppUpdateRelease)` when a version **newer** than the current
/// build is available.  Returns `None` on any network error, parse failure, or
/// when the remote version is not newer than the running build.
///
/// Intended to be called from a background thread; never blocks the UI.
pub fn check_for_app_update() -> Option<AppUpdateRelease> {
    let resp = ureq::get("https://api.github.com/repos/kodezine/RustyCAN/releases/latest")
        .set(
            "User-Agent",
            concat!("RustyCAN/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?;

    let body: serde_json::Value = resp.into_json().ok()?;
    let tag = body["tag_name"].as_str()?;
    let remote = parse_semver_tag(tag)?;

    // Compare against current build version; no-op on dev builds where the
    // version cannot be parsed (bundled_firmware_version returns None).
    let current = crate::bundled_firmware_version()?;
    if remote <= current {
        return None;
    }

    let release_url = format!("https://github.com/kodezine/RustyCAN/releases/tag/{}", tag);

    // Walk the assets array for a platform-specific download URL.
    let (download_url, asset_name) = if let Some(expected_name) = platform_asset_name(tag) {
        let url = body["assets"]
            .as_array()
            .and_then(|arr| {
                arr.iter().find(|a| {
                    a["name"]
                        .as_str()
                        .map(|n| n == expected_name)
                        .unwrap_or(false)
                })
            })
            .and_then(|a| a["browser_download_url"].as_str())
            .unwrap_or("")
            .to_string();
        (url, expected_name)
    } else {
        (String::new(), String::new())
    };

    Some(AppUpdateRelease {
        version: remote,
        download_url,
        asset_name,
        release_url,
    })
}

// ─── Download ─────────────────────────────────────────────────────────────────

/// Download a release asset to `<temp_dir>/<asset_name>`, reporting 0.0–1.0
/// progress via `progress`.
///
/// Returns the path to the downloaded file on success.
/// Only used on platforms where `AppUpdateRelease::can_download()` is true.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn download_update(
    release: &AppUpdateRelease,
    progress: impl Fn(f64) + Send,
) -> Result<PathBuf, String> {
    use std::io::Read;

    let dest = std::env::temp_dir().join(&release.asset_name);

    let resp = ureq::get(&release.download_url)
        .set(
            "User-Agent",
            concat!("RustyCAN/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let content_length: Option<u64> = resp.header("Content-Length").and_then(|v| v.parse().ok());

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 65536];
    let mut file =
        std::fs::File::create(&dest).map_err(|e| format!("Cannot create temp file: {e}"))?;
    let mut downloaded: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Read error: {e}"))?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buf[..n])
            .map_err(|e| format!("Write error: {e}"))?;
        downloaded += n as u64;
        if let Some(total) = content_length {
            progress(downloaded as f64 / total as f64);
        }
    }

    progress(1.0);
    Ok(dest)
}

// ─── Apply and restart (macOS aarch64 only) ───────────────────────────────────

/// Mount the downloaded DMG, copy the `.app` bundle over the running one, and
/// re-exec the new binary.  This function does not return on success.
///
/// Only compiled on macOS Apple Silicon; other targets use `open_release_page`
/// instead.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn apply_and_restart(dmg_path: &Path) -> Result<(), String> {
    use std::process::Command;

    let mount_point = std::env::temp_dir().join("rustycan_update_mount");
    let _ = std::fs::create_dir_all(&mount_point);

    // Mount the DMG at a controlled mount point.
    let status = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-quiet",
            "-mountpoint",
            &mount_point.to_string_lossy(),
            &dmg_path.to_string_lossy(),
        ])
        .status()
        .map_err(|e| format!("hdiutil attach failed: {e}"))?;

    if !status.success() {
        return Err(format!(
            "hdiutil attach exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    // Find the .app bundle inside the mounted volume.
    let mounted_app = std::fs::read_dir(&mount_point)
        .map_err(|e| format!("Cannot read mount point: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().map(|x| x == "app").unwrap_or(false))
        .ok_or_else(|| "No .app bundle found in mounted DMG".to_string())?;

    // Locate the running .app bundle from the current executable path.
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot resolve current exe: {e}"))?;
    let app_bundle = find_app_bundle_root(&current_exe)
        .ok_or_else(|| "Running outside a .app bundle — cannot update in-place".to_string())?;
    let install_dir = app_bundle
        .parent()
        .ok_or_else(|| "Cannot determine .app parent directory".to_string())?;

    // Copy the new .app bundle over the existing one.
    let status = Command::new("cp")
        .args([
            "-Rf",
            &mounted_app.to_string_lossy(),
            &install_dir.to_string_lossy(),
        ])
        .status()
        .map_err(|e| format!("cp failed: {e}"))?;

    if !status.success() {
        let _ = Command::new("hdiutil")
            .args(["detach", &mount_point.to_string_lossy()])
            .status();
        return Err(format!(
            "cp exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    // Detach the DMG (best-effort; ignore failure).
    let _ = Command::new("hdiutil")
        .args(["detach", &mount_point.to_string_lossy()])
        .status();

    // Re-exec the new binary with the same arguments.
    let new_exe = app_bundle.join("Contents").join("MacOS").join("rustycan");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = Command::new(&new_exe).args(&args).spawn();

    std::process::exit(0);
}

/// Walk up from `exe_path` until a `.app` bundle root is found.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn find_app_bundle_root(exe_path: &Path) -> Option<PathBuf> {
    let mut current = exe_path.to_path_buf();
    while let Some(parent) = current.parent() {
        if current.extension().map(|e| e == "app").unwrap_or(false) {
            return Some(current);
        }
        current = parent.to_path_buf();
    }
    None
}

// ─── Open release page (all platforms) ───────────────────────────────────────

/// Open the GitHub release page for `release` in the system browser.
///
/// Used as the primary update action on Windows and Linux, and as a fallback
/// "Release notes" link on macOS.
pub fn open_release_page(release: &AppUpdateRelease) {
    let url = &release.release_url;
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

// ─── Unit tests (SOUPTC026) ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::parse_semver_tag;

    #[test]
    fn parse_plain_tag() {
        assert_eq!(parse_semver_tag("v1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn parse_describe_suffix() {
        assert_eq!(parse_semver_tag("v0.2.0-5-gabcdef"), Some((0, 2, 0)));
    }

    #[test]
    fn parse_zero_patch() {
        assert_eq!(parse_semver_tag("v1.0.0"), Some((1, 0, 0)));
    }

    #[test]
    fn parse_no_v_prefix() {
        assert_eq!(parse_semver_tag("2.3.4"), Some((2, 3, 4)));
    }

    #[test]
    fn parse_empty() {
        assert_eq!(parse_semver_tag(""), None);
    }

    #[test]
    fn parse_garbage() {
        assert_eq!(parse_semver_tag("not-a-version"), None);
    }

    #[test]
    fn parse_too_few_parts() {
        assert_eq!(parse_semver_tag("v1.2"), None);
    }
}
