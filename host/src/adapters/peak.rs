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
                hardware_timestamp_us: None,
            }),
            Err(e) => {
                let msg = e.to_string();
                // host-can signals timeout via a specific error string.
                if msg.to_lowercase().contains("timeout") {
                    Err(AdapterError::Timeout)
                } else {
                    Err(AdapterError::Io(msg))
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
