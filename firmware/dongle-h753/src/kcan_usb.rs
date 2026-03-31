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
    pub async fn write_packet(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        self.ep.write(data).await
    }
}

impl<'d, D: UsbDriver<'d>> KCanReceiver<'d, D> {
    pub async fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        self.ep.read(buf).await
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
