//! PEAK PCAN-USB adapter — thin wrapper around `host-can`.
//!
//! Translates the `host_can::adapter::Adapter` trait to the unified
//! [`crate::adapters::CanAdapter`] trait.  Hardware timestamps are not
//! available from PEAK on macOS (`hardware_timestamp_us = None`).

use std::time::{Duration, Instant};

use host_can::frame::CanFrame;

use super::{probe_adapter_kind, AdapterError, AdapterKind, CanAdapter, ReceivedFrame};

/// Minimum interval between USB-presence probes triggered by recv errors.
///
/// A transient CAN bus error (BUSHEAVY / BUSLIGHT / BUSOFF / OVERRUN) surfaces
/// from libPCBUSB as the *same* "Unable to receive message" status that a real
/// USB removal produces.  We disambiguate by checking whether the PEAK vendor
/// ID is still enumerated, but that check spawns `ioreg` on macOS, so we
/// debounce it to avoid a subprocess storm during an error burst.
const PRESENCE_PROBE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Maximum number of resends after a transient "Unable to send message" TX
/// failure while the hardware is still enumerated on USB.
const SEND_RETRY_MAX: u32 = 8;

/// Backoff between send retries.  A CAN controller that entered bus-off needs
/// to observe 128 runs of 11 recessive bits before rejoining (≈5.6 ms at
/// 250 kbps on an otherwise idle bus), so a short sleep gives libPCBUSB time to
/// auto-recover before the next `CAN_Write`.
const SEND_RETRY_BACKOFF: Duration = Duration::from_millis(20);

pub struct PeakAdapter {
    inner: Box<dyn host_can::adapter::Adapter>,
    /// Cached `(instant, present)` result of the last USB-presence probe.
    last_probe: Option<(Instant, bool)>,
}

impl PeakAdapter {
    pub fn new(inner: Box<dyn host_can::adapter::Adapter>) -> Self {
        Self {
            inner,
            last_probe: None,
        }
    }

    /// Returns `true` while a PEAK adapter is still enumerated on USB.
    ///
    /// Debounced to at most one real probe per [`PRESENCE_PROBE_DEBOUNCE`]; the
    /// cached answer is reused for calls in between so an error burst cannot
    /// spawn a flood of `ioreg` subprocesses.
    fn hardware_present(&mut self) -> bool {
        if let Some((when, present)) = self.last_probe {
            if when.elapsed() < PRESENCE_PROBE_DEBOUNCE {
                return present;
            }
        }
        let present = probe_adapter_kind(&AdapterKind::Peak, "", 0);
        self.last_probe = Some((Instant::now(), present));
        present
    }
}

impl CanAdapter for PeakAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        match self.inner.recv(Some(timeout)) {
            Ok(frame) => Ok(ReceivedFrame {
                frame,
                hardware_timestamp_ns: None,
                channel: 0,
                is_tx_echo: false,
            }),
            Err(e) => {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                // host-can returns "The read operation timed out" (ReadTimeout)
                // when PCAN_ERROR_QRCVEMPTY persists until the timeout window
                // expires.  Treat that as a clean no-frame timeout.
                if lower.contains("timed out") || lower.contains("timeout") {
                    Err(AdapterError::Timeout)
                } else {
                    // host-can flattens *every* non-OK, non-QRCVEMPTY CAN_Read
                    // status to the same "Unable to receive message"
                    // (ReadFailed) error.  That covers both a genuine USB
                    // removal AND transient CAN bus conditions that are routine
                    // on a busy bus — PCAN_ERROR_BUSLIGHT / BUSHEAVY / BUSOFF /
                    // OVERRUN / QOVERRUN.  Treating a transient bus error as a
                    // disconnect would tear the adapter down and re-open it,
                    // dropping frames for the duration of every glitch.
                    //
                    // Disambiguate by checking whether the PEAK hardware is
                    // still on the USB bus: if it is, this is a recoverable bus
                    // error — report a no-frame Timeout so the recv loop keeps
                    // running.  Only when the device has actually left the USB
                    // tree do we report Disconnected and let the session
                    // reconnect.
                    if self.hardware_present() {
                        Err(AdapterError::Timeout)
                    } else {
                        Err(AdapterError::Disconnected)
                    }
                }
            }
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        let mut attempt = 0u32;
        loop {
            match self.inner.send(frame) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // libPCBUSB reports the same "Unable to send message"
                    // status for a transient TX failure — the controller went
                    // bus-off / error-passive because the target reset its CAN
                    // peripheral during a firmware state transition (e.g. the
                    // bootloader jumping to the freshly flashed application) —
                    // and for a genuine USB removal.  Disambiguate exactly as
                    // the recv path does: if the PEAK is gone from USB, report
                    // Disconnected; otherwise treat it as a recoverable bus-off
                    // and resend.  A failed CAN_Write never queued the frame,
                    // so resending cannot duplicate traffic on the bus.
                    if !self.hardware_present() {
                        return Err(AdapterError::Disconnected);
                    }
                    attempt += 1;
                    if attempt > SEND_RETRY_MAX {
                        return Err(AdapterError::Io(e.to_string()));
                    }
                    std::thread::sleep(SEND_RETRY_BACKOFF);
                }
            }
        }
    }

    fn name(&self) -> &str {
        "PEAK PCAN-USB"
    }
}
