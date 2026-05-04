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
    /// chunks so the STM32 OTG FS 64-byte FIFO never overflows.
    ///
    /// For an 80-byte KCAN frame this produces:
    ///   packet 1: bytes 0..64  (full-size → host waits for more)
    ///   packet 2: bytes 64..80 (short → host knows transfer is complete)
    pub async fn write_packet(&mut self, data: &[u8]) -> Result<(), EndpointError> {
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
    /// Because MPS=64 and a KCAN frame is 80 bytes, the host splits every
    /// frame into two USB packets: 64 bytes then 16 bytes.  A single
    /// `ep.read()` call returns at most one USB packet (one FIFO entry), so we
    /// must loop until we have received a short packet (< MPS) or the buffer
    /// is full.
    ///
    /// Without this loop, `kcan_io_task` would receive 64 bytes on the first
    /// call (rejected as "unexpected size"), then 16 bytes on the second call
    /// (also rejected), silently dropping every TX frame from the host.
    pub async fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
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
        // Full-speed bulk endpoints, 64-byte max packet size.
        // iface and alt are scoped here so their borrows are released before func is dropped.
        let (rx, tx) = {
            let mut iface = func.interface();
            let mut alt = iface.alt_setting(0xFF, 0xFF, 0xFF, None);
            let rx = alt.endpoint_bulk_out(None, 64);
            let tx = alt.endpoint_bulk_in(None, 64);
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
