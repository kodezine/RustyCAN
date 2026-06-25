//! PEAK PCAN-USB adapter — thin wrapper around `host-can`.
//!
//! Translates the `host_can::adapter::Adapter` trait to the unified
//! [`crate::adapters::CanAdapter`] trait.  Hardware timestamps are not
//! available from PEAK on macOS (`hardware_timestamp_us = None`).

use std::time::Duration;

use host_can::frame::CanFrame;

use super::{AdapterError, CanAdapter, ReceivedFrame};

pub struct PeakAdapter {
    inner: Box<dyn host_can::adapter::Adapter>,
}

impl PeakAdapter {
    pub fn new(inner: Box<dyn host_can::adapter::Adapter>) -> Self {
        Self { inner }
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
                    // Any other error — including "Unable to receive message"
                    // (ReadFailed), which host-can returns when CAN_Read yields
                    // any non-OK, non-QRCVEMPTY status such as
                    // PCAN_ERROR_DISCONNECT on physical USB removal — signals
                    // that the adapter is gone.  Return Disconnected so the
                    // session enters the reconnect loop instead of silently
                    // continuing with stale "Connected" state.
                    Err(AdapterError::Disconnected)
                }
            }
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        self.inner
            .send(frame)
            .map_err(|e| AdapterError::Io(e.to_string()))
    }

    fn name(&self) -> &str {
        "PEAK PCAN-USB"
    }
}
