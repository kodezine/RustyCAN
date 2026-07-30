//! USB descriptor dump for Apex USB-CAN reverse-engineering
//! (issue #103, Phase 0).
//!
//! Plug the module into the Mac, then run from the `host/` directory:
//!
//! ```sh
//! cargo run --example apex_probe            # list all devices, deep-dump Apex (VID 0x0878)
//! cargo run --example apex_probe 0878:4000  # also deep-dump a specific VID:PID (hex)
//! ```
//!
//! The deep dump reports the interface number and the bulk IN/OUT endpoint
//! addresses + max packet sizes needed to implement the userspace driver
//! (`host/src/adapters/apex/mod.rs`).  Paste the output on issue #103.

use nusb::{DeviceInfo, MaybeFuture};

/// USB vendor ID of the Apex USB-CAN hardware (confirm from the dump).
const APEX_VID: u16 = 0x0878;

fn main() {
    let target = std::env::args().nth(1).and_then(parse_vidpid);

    let devices: Vec<DeviceInfo> = match nusb::list_devices().wait() {
        Ok(iter) => iter.collect(),
        Err(e) => {
            eprintln!("USB enumeration failed: {e}");
            std::process::exit(1);
        }
    };

    println!("== USB devices ({}) ==", devices.len());
    let mut apex_seen = false;
    for d in &devices {
        let is_apex = d.vendor_id() == APEX_VID;
        let is_target = target == Some((d.vendor_id(), d.product_id()));
        apex_seen |= is_apex;

        println!(
            "{:04x}:{:04x}  class={:02x}  {}{}{}{}",
            d.vendor_id(),
            d.product_id(),
            d.class(),
            d.manufacturer_string().unwrap_or("?"),
            d.product_string()
                .map(|s| format!(" / {s}"))
                .unwrap_or_default(),
            d.serial_number()
                .map(|s| format!(" / sn={s}"))
                .unwrap_or_default(),
            if is_apex { "   <== APEX" } else { "" },
        );

        if is_apex || is_target {
            dump_device(d);
        }
    }

    if !apex_seen && target.is_none() {
        println!("\nNo Apex device (VID 0x{APEX_VID:04x}) found.");
        println!(
            "If your module reports a different VID, re-run with the pair shown \
             above, e.g.: cargo run --example apex_probe VID:PID"
        );
    }
}

fn parse_vidpid(s: String) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    let hex = |x: &str| u16::from_str_radix(x.trim().trim_start_matches("0x"), 16).ok();
    Some((hex(v)?, hex(p)?))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump_device(info: &DeviceInfo) {
    println!(
        "   -- descriptors for {:04x}:{:04x} --",
        info.vendor_id(),
        info.product_id()
    );
    println!(
        "   bcdUSB={:04x}  bcdDevice(fw)={:04x}  class={:02x}/{:02x}/{:02x}",
        info.usb_version(),
        info.device_version(),
        info.class(),
        info.subclass(),
        info.protocol()
    );
    let device = match info.open().wait() {
        Ok(d) => d,
        Err(e) => {
            println!("   (cannot open device: {e})");
            return;
        }
    };

    // Dump every cached configuration descriptor.  `active_configuration()` can
    // fail on macOS for an unclaimed vendor-specific device (the OS leaves it
    // unconfigured and reports a bogus active value), but the configuration
    // descriptors themselves are still cached and enumerable.
    let active = device.active_configuration().ok();
    let mut any = false;
    let mut max_endpoints = 0usize;
    for config in device.configurations() {
        any = true;
        let is_active = active
            .as_ref()
            .is_some_and(|a| a.configuration_value() == config.configuration_value());
        println!(
            "   configuration value={} #interfaces={}{}",
            config.configuration_value(),
            config.num_interfaces(),
            if is_active { "  (active)" } else { "" }
        );
        println!("     raw: {}", hex(config.as_bytes()));
        for iface in config.interfaces() {
            println!("     interface {}", iface.interface_number());
            for alt in iface.alt_settings() {
                println!(
                    "       alt {}  class={:02x} subclass={:02x} protocol={:02x}  #endpoints={}",
                    alt.alternate_setting(),
                    alt.class(),
                    alt.subclass(),
                    alt.protocol(),
                    alt.num_endpoints()
                );
                max_endpoints = max_endpoints.max(alt.num_endpoints() as usize);
                for ep in alt.endpoints() {
                    println!(
                        "         ep 0x{:02x}  {:?}  {:?}  max_packet={}  interval={}",
                        ep.address(),
                        ep.direction(),
                        ep.transfer_type(),
                        ep.max_packet_size(),
                        ep.interval()
                    );
                }
            }
        }
    }
    if !any {
        println!("   (no configuration descriptors cached by the OS)");
    }

    // Classify by observable enumeration (PID + endpoint count):
    //   running/application mode exposes the full multi-endpoint CAN interface
    //   (PID 0x1101/0x1181 on this hardware); bootloader mode exposes only the
    //   firmware-loader endpoint and must be flashed before CAN traffic works.
    let pid = info.product_id();
    let running = matches!(pid, 0x1101 | 0x1181) || max_endpoints >= 5;
    if running {
        println!("   MODE: running / application ({max_endpoints} endpoints) — ready for CAN");
    } else {
        println!(
            "   MODE: bootloader ({max_endpoints} endpoint) — firmware upload required before CAN use"
        );
    }
}
