# Building from Source

## 🚀 Build & Run

```sh
git clone https://github.com/kodezine/RustyCAN
cd RustyCAN
cargo run --release -p rustycan
```

The GUI window opens immediately. No command-line flags are required.

## 🛠️ Development

### Updating the app icon

When editing icon images in `host/assets/RustyCAN.iconset/`, regenerate the `.icns` file and commit both:

```sh
cd host/assets
iconutil -c icns RustyCAN.iconset -o RustyCAN.icns
git add RustyCAN.iconset/ RustyCAN.icns
git commit -m "Update app icon"
```

**Why?** The `.icns` file is a versioned build artifact tracked in the repository. CI builds use the committed version to ensure deterministic, reproducible releases without modifying the working tree during builds (which would add a `-dirty` suffix to version strings from `git describe --dirty`).

The `.iconset` folder contains source PNG images at multiple resolutions (16×16 through 512×512, with @2x variants). macOS's `iconutil` combines these into the installable `.icns` format required by DMG/app bundles.
