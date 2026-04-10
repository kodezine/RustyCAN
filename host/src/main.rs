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

    /// Start RustyCAN as a full-screen terminal UI instead of opening a GUI window.
    ///
    /// Requires `--config` to be supplied.  The TUI displays live NMT, PDO, and
    /// SDO panels.  Press `n` to send an NMT command, `s` for SDO read, `w` for
    /// SDO write, `L` to toggle the event-log panel, and `q` / Ctrl-C to quit.
    #[arg(long)]
    tui: bool,

    /// Print decoded CAN events as timestamped text lines to stdout and exit
    /// when the adapter disconnects or Ctrl-C is pressed.
    ///
    /// Requires `--config` to be supplied.  No GUI or TUI window is opened;
    /// output can be piped directly to a file or another tool.
    #[arg(long)]
    log_to_stdout: bool,
}

fn main() {
    let args = CliArgs::parse();

    // Validate flags that require --config.
    if args.tui && args.config.is_none() {
        eprintln!("error: --tui requires --config <FILE>");
        std::process::exit(1);
    }
    if args.log_to_stdout && args.config.is_none() {
        eprintln!("error: --log-to-stdout requires --config <FILE>");
        std::process::exit(1);
    }

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

    if args.log_to_stdout {
        // SAFETY: config is Some — validated above.
        let path = args.config.as_deref().unwrap();
        if let Err(e) = rustycan::tui::log_stream::stream(path, effective_port) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else if args.tui {
        let path = args.config.as_deref().unwrap();
        if let Err(e) = rustycan::tui::run_from_config(path, effective_port) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else {
        if let Err(e) = rustycan::gui::run(args.config, effective_port) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
