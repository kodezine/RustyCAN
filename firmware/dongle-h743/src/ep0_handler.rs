//! KCAN EP0 vendor request handler.
//!
//! Responds to the control sequence the host KCAN adapter issues on every
//! connect before bulk traffic can start:
//!
//! ```text
//! Host → GET_INFO      (0x01)  device responds with firmware version + UID
//! Host → GET_BT_CONST  (0x06)  device responds with clock / BRP / TSEG limits
//! Host → SET_BITTIMING (0x02)  host sends computed BRP/TSEG for chosen bitrate
//! Host → SET_MODE      (0x04)  host sends bus-on / listen-only flags
//! ```
//!
//! Without this handler `KCanAdapter::open()` fails immediately with
//! `"GET_INFO: short response"` and no bulk traffic flows.
//!
//! This handler also drives [`crate::usb_task::USB_CONFIGURED`] on connect /
//! disconnect (replacing the old `UsbStateHandler`), so only one handler needs
//! to be registered with the USB builder.

use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::Handler;

use kcan_protocol::control::{KCanBtConst, KCanDeviceInfo, RequestCode};

use defmt::*;

/// EP0 vendor handler: responds to GET_INFO / GET_BT_CONST and ACKs
/// SET_BITTIMING / SET_MODE.  Also signals USB connection state.
pub struct KCanEp0Handler {
    pub uid_lo: u32,
}

impl Handler for KCanEp0Handler {
    fn configured(&mut self, configured: bool) {
        if configured {
            info!("USB: configured by host");
        } else {
            info!("USB: disconnected / reset");
        }
        crate::usb_task::USB_CONFIGURED.signal(configured);
        crate::usb_task::USB_CONFIGURED_LED.signal(configured);
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        // Only intercept vendor requests directed at the device.
        if req.request_type != RequestType::Vendor || req.recipient != Recipient::Device {
            return None;
        }

        match req.request {
            r if r == RequestCode::GetInfo as u8 => {
                let mut info = KCanDeviceInfo::new(1, 0, 0, self.uid_lo);
                // H753 exposes two FDCAN channels; frames carry a channel field.
                info.channels = 2;
                let bytes = info.to_bytes();
                let len = bytes.len().min(buf.len());
                buf[..len].copy_from_slice(&bytes[..len]);
                info!(
                    "EP0 GET_INFO → protocol_v1, channels=2, uid={:08X}",
                    self.uid_lo
                );
                Some(InResponse::Accepted(&buf[..len]))
            }
            r if r == RequestCode::GetBtConst as u8 => {
                let bytes = KCanBtConst::H753_64MHZ.to_bytes();
                let len = bytes.len().min(buf.len());
                buf[..len].copy_from_slice(&bytes[..len]);
                info!("EP0 GET_BT_CONST → clock=32 MHz, BRP 1-512");
                Some(InResponse::Accepted(&buf[..len]))
            }
            _ => None,
        }
    }

    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        // Only intercept vendor requests directed at the device.
        if req.request_type != RequestType::Vendor || req.recipient != Recipient::Device {
            return None;
        }

        match req.request {
            r if r == RequestCode::SetBitTiming as u8 => {
                // ACK the host's timing parameters; firmware always runs at
                // 250 kbps (configured at init) and the host will compute
                // the same BRP=8 from our 32 MHz clock constant.
                info!("EP0 SET_BITTIMING accepted (250 kbps fixed)");
                Some(OutResponse::Accepted)
            }
            r if r == RequestCode::SetMode as u8 => {
                // ACK the bus-on / listen-only request.  For now the firmware
                // is always in normal mode; listen-only support is deferred.
                info!("EP0 SET_MODE accepted (normal mode)");
                // Signal the IO task that a new host session is starting.
                // SET_MODE is sent by the host on every open() call.  We only
                // fire BULK_RESTART here — USB_CONFIGURED is gated exclusively
                // by the physical configured() callback (SET_CONFIGURATION).
                // Signalling USB_CONFIGURED from SET_MODE caused a deadlock:
                // the outer USB_CONFIGURED.wait() consumed the signal, then
                // BULK_RESTART immediately re-fired it within the same select
                // iteration, leaving USB_CONFIGURED empty after BULK_RESTART
                // handling so the outer loop would block forever.
                crate::usb_task::BULK_RESTART.signal(());
                Some(OutResponse::Accepted)
            }
            _ => None,
        }
    }
}
