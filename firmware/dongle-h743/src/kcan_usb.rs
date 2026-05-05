//! KCAN USB class — Bulk IN/OUT endpoints for KCAN frame transport.
//!
//! - Bulk IN  0x81: device→host (KCanFrames: RX data + TX echoes + status)
//! - Bulk OUT 0x02: host→device (KCanFrames to transmit on the bus)
//!
//! EP0 vendor requests (GET_INFO, SET_BITTIMING, SET_MODE ...) are not yet
//! handled in firmware v1; the bus runs at the default 250 kbps until future
//! EP0 handler implementation.

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
    /// chunks.  At High-Speed MPS=512; an 80-byte KCAN frame fits in a single
    /// packet (short packet), so the host knows the transfer is complete.
    pub async fn write_packet(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        const MPS: usize = 512;
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
    /// At High-Speed MPS=512; an 80-byte KCAN frame arrives in a single
    /// short packet so the loop exits immediately after one read.
    pub async fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        const MPS: usize = 512;
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
        // High-speed bulk endpoints, 512-byte max packet size.
        // The h743 connects via USB OTG HS (ULPI) at 480 Mbps; USB spec requires
        // bulk endpoints to declare wMaxPacketSize=512 at High-Speed.
        // Windows rejects HS devices that advertise 64-byte (Full-Speed) MPS.
        // iface and alt are scoped here so their borrows are released before func is dropped.
        let (rx, tx) = {
            let mut iface = func.interface();
            let mut alt = iface.alt_setting(0xFF, 0xFF, 0xFF, None);
            let rx = alt.endpoint_bulk_out(None, 512);
            let tx = alt.endpoint_bulk_in(None, 512);
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
