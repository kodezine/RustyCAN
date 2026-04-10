use std::path::PathBuf;

use clap::Parser;

/// RustyCAN — CAN-bus monitor and CANopen analyser.
#[derive(Parser)]
#[command(version, about)]
struct CliArgs {
    /// Path to a JSON configuration file.
    ///
    /// When supplied the application skips the connect form and starts the CAN
    /// session automatically using the settings in the file.  The file follows
    /// the same schema that RustyCAN writes to its app-data directory; see the
    /// bundled `config.example.json` for a fully annotated example.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Port for the live HTML dashboard (`http://127.0.0.1:<port>/`).
    ///
    /// Overrides the `http_port` field in the JSON config file when both are
    /// supplied.  Defaults to 7878 when absent from both the CLI and the file.
    #[arg(long, value_name = "PORT")]
    http_port: Option<u16>,
}

fn main() -> Result<(), eframe::Error> {
    let args = CliArgs::parse();

    // Resolve the effective HTTP port:
    //   1. CLI --http-port (highest priority)
    //   2. "http_port" field inside the JSON config file
    //   3. Hard-coded default 7878
    let effective_port: u16 = args.http_port.unwrap_or_else(|| {
        args.config
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| {
                serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .and_then(|v| v.get("http_port").and_then(|x| x.as_u64()))
                    .and_then(|n| u16::try_from(n).ok())
            })
            .unwrap_or(7878)
    });

    rustycan::gui::run(args.config, effective_port)
}
