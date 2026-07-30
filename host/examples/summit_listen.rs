//! Regression smoke test for the Summit adapter after the PEAK->Summit rename.
//!
//! Opens the Summit adapter (channel 1) at 250 kbit/s in listen-only mode and
//! prints received frames for ~6 s.  Requires the vendor driver library.
//!
//!     cargo run --example summit_listen

use std::time::{Duration, Instant};

use embedded_can::{Frame, Id};
use rustycan::adapters::{open_adapter, AdapterKind};

fn main() {
    let baud = 250_000;
    println!("Opening Summit adapter (channel 1) @ {baud} bps, listen-only...");
    let mut adapter = match open_adapter(&AdapterKind::Summit, "1", baud, true) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    println!("Opened: {}\nListening for 6 s...", adapter.name());

    let start = Instant::now();
    let mut rx = 0u32;
    while start.elapsed() < Duration::from_secs(6) {
        match adapter.recv(Duration::from_millis(500)) {
            Ok(f) => {
                rx += 1;
                let id = match f.frame.id() {
                    Id::Standard(s) => s.as_raw() as u32,
                    Id::Extended(e) => e.as_raw(),
                };
                if rx <= 30 {
                    println!(
                        "RX {rx:>3}  id=0x{id:03X}  dlc={}  {:02X?}",
                        f.frame.dlc(),
                        f.frame.data()
                    );
                }
            }
            Err(e) => {
                let s = e.to_string();
                if !s.to_lowercase().contains("timeout") {
                    eprintln!("recv error: {s}");
                }
            }
        }
    }
    println!("Done. Received {rx} frames.");
}
