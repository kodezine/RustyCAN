//! Live smoke test for the Apex USB-CAN adapter (issue #103).
//!
//! Opens the Apex adapter at 250 kbit/s, prints received frames for ~10 s, and
//! sends one test frame after 2 s.  Exercises the full path: bootloader
//! RECONNECT -> running mode -> bus-on -> RX/TX.
//!
//!     cargo run --example apex_listen

use std::time::{Duration, Instant};

use embedded_can::{Frame, Id, StandardId};
use host_can::frame::CanFrame;
use rustycan::adapters::{open_adapter, AdapterKind};

fn main() {
    let baud = 250_000;
    println!("Opening Apex adapter @ {baud} bps (boots from bootloader if needed)...");
    let mut adapter = match open_adapter(&AdapterKind::Apex { serial: None }, baud, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    println!("Opened: {}\nListening for 10 s...", adapter.name());

    let start = Instant::now();
    let mut rx = 0u32;
    let mut sent = false;
    while start.elapsed() < Duration::from_secs(10) {
        match adapter.recv(Duration::from_millis(500)) {
            Ok(f) => {
                rx += 1;
                let id = match f.frame.id() {
                    Id::Standard(s) => s.as_raw() as u32,
                    Id::Extended(e) => e.as_raw(),
                };
                if rx <= 40 {
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

        if !sent && start.elapsed() > Duration::from_secs(2) {
            if let Some(frame) =
                CanFrame::new(Id::Standard(StandardId::new(0x555).unwrap()), &[0xDE, 0xAD])
            {
                match adapter.send(&frame) {
                    Ok(()) => println!(">> TX 0x555 [DE AD] sent"),
                    Err(e) => eprintln!(">> TX failed: {e}"),
                }
            }
            sent = true;
        }
    }
    println!("Done. Received {rx} frames.");
}
