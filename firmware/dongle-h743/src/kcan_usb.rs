//! KCAN USB class — Bulk IN/OUT endpoints for KCAN frame transport.
//!
//! - Bulk IN  0x81: device→host (KCanFrames: RX data + TX echoes + status)
//! - Bulk OUT 0x02: host→device (KCanFrames to transmit on the bus)
//!
//! ## USB MPS feature-gating
//!
//! The `usb-hs` Cargo feature selects the bulk endpoint max packet size:
//!
//! | Feature | MPS | Use case |
//! |---------|-----|----------|
//! | `usb-hs` (default) | 512 bytes | OTG HS via ULPI (h743) — Windows requires 512 on HS |
//! | *(off)* | 64 bytes | OTG FS fallback — h753 Nucleo or FS-only hosts |
//!
//! The host side (`kcan.rs`) already handles both transparently: its
//! `BULK_IN_BUF=512` is a multiple of both 64 and 512.

use embassy_usb::driver::{Driver as UsbDriver, EndpointError, EndpointIn, EndpointOut};
use embassy_usb::Builder;

/// Bulk IN endpoint wrapper (device → host).
pub struct KCanSender<'d, D: UsbDriver<'d>> {
    ep: D::EndpointIn,
}

/// Bulk OUT endpoint wrapper (host → device).
pub struct KCanReceiver<'d, D: UsbDriver<'d>> {
    ep: D::EndpointOut,
}

impl<'d, D: UsbDriver<'d>> KCanSender<'d, D> {
    /// Write `data` to the Bulk IN endpoint, splitting into max-packet-size
    /// chunks.  The MPS is 512 bytes when the `usb-hs` feature is enabled
    /// (default on h743 via OTG HS ULPI) or 64 bytes otherwise.
    ///
    /// An 80-byte KCAN frame is always shorter than MPS=512 (one short packet)
    /// and spans two packets at MPS=64 (64 + 16 bytes).
    pub async fn write_packet(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        #[cfg(feature = "usb-hs")]
        const MPS: usize = 512;
        #[cfg(not(feature = "usb-hs"))]
        const MPS: usize = 64;
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + MPS).min(data.len());
            self.ep.write(&data[offset..end]).await?;
            offset = end;
        }
        // Send a zero-length packet if the payload is an exact multiple of MPS,
        // so the host can distinguish "transfer done" from "more data coming".
        if data.len().is_multiple_of(MPS) {
            self.ep.write(&[]).await?;
        }
        Ok(())
    }
}

impl<'d, D: UsbDriver<'d>> KCanReceiver<'d, D> {
    /// Read one KCAN frame from the Bulk OUT endpoint into `buf`.
    ///
    /// Loops until a short packet (< MPS) is received or `buf` is full.
    /// At MPS=512 an 80-byte frame arrives in one short packet (single loop
    /// iteration); at MPS=64 it arrives in two packets (64 + 16).
    pub async fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        #[cfg(feature = "usb-hs")]
        const MPS: usize = 512;
        #[cfg(not(feature = "usb-hs"))]
        const MPS: usize = 64;
        let mut total = 0;
        loop {
            let n = self.ep.read(&mut buf[total..]).await?;
            total += n;
            // A short packet (n < MPS) signals the end of the USB transfer.
            // Stop early also if the buffer is full.
            if n < MPS || total >= buf.len() {
                break;
            }
        }
        Ok(total)
    }
}

/// KCAN USB class — owns the bulk IN and OUT endpoints.
pub struct KCanUsbClass<'d, D: UsbDriver<'d>> {
    sender: KCanSender<'d, D>,
    receiver: KCanReceiver<'d, D>,
}

impl<'d, D: UsbDriver<'d>> KCanUsbClass<'d, D> {
    pub fn new(builder: &mut Builder<'d, D>) -> Self {
        // Vendor-specific class, subclass and protocol (0xFF each).
        let mut func = builder.function(0xFF, 0xFF, 0xFF);
        // Bulk endpoint MPS: 512 bytes with `usb-hs` (default on h743 — OTG HS
        // via ULPI; Windows rejects HS devices advertising 64-byte FS MPS),
        // 64 bytes without (FS fallback).
        #[cfg(feature = "usb-hs")]
        const MPS: u16 = 512;
        #[cfg(not(feature = "usb-hs"))]
        const MPS: u16 = 64;
        let (rx, tx) = {
            let mut iface = func.interface();
            let mut alt = iface.alt_setting(0xFF, 0xFF, 0xFF, None);
            let rx = alt.endpoint_bulk_out(None, MPS);
            let tx = alt.endpoint_bulk_in(None, MPS);
            (rx, tx)
        };
        drop(func);
        Self {
            sender: KCanSender { ep: tx },
            receiver: KCanReceiver { ep: rx },
        }
    }

    /// Split into independent sender and receiver for concurrent use.
    pub fn split(self) -> (KCanSender<'d, D>, KCanReceiver<'d, D>) {
        (self.sender, self.receiver)
    }
}
