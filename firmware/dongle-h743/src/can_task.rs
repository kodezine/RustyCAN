//! FDCAN1 task — receives CAN frames and forwards TX requests.
//!
//! Single-channel variant for STM32H743I-EVAL (MB1246 Rev E).
//! FDCAN1 is connected to the on-board TJA1044 transceiver via CN3 DB9.
//!
//! # CANTx fix
//!
//! The previous implementation used `select(rx.read(), usb_to_can.receive())`
//! which serialised RX and TX: whichever future resolved first consumed the
//! entire iteration.  On a busy CAN bus `rx.read()` was always ready first,
//! starving USB-initiated TX frames until the 32-entry `USB_TO_CAN` channel
//! filled and new TX requests were silently dropped.
//!
//! The fix splits the FDCAN peripheral into its independent `CanTx` / `CanRx`
//! halves (already separate types from `Can::split()`) and drives them with
//! `embassy_futures::join::join`.  Both loops run concurrently in the same
//! embassy task: while the TX loop blocks on `usb_to_can.receive()`, the RX
//! loop can freely drain the FDCAN FIFO, and vice-versa.

use core::sync::atomic::Ordering;
use embassy_stm32::can::config::{DataBitTiming, FrameTransmissionConfig};
use embassy_stm32::can::frame::{FdFrame, Frame, Header};
use embassy_stm32::can::CanConfigurator;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Instant};
use embedded_can::{ExtendedId, Id, StandardId};

use kcan_protocol::frame::{FrameFlags, KCanFrame};

use defmt::*;

#[embassy_executor::task]
pub async fn can_task(
    can_cfg: CanConfigurator<'static>,
    can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    // ── Wait for host to configure baud rate and mode ─────────────────────────
    // The EP0 handler (Phase 3) signals CAN_CONFIG on SET_MODE(BUS_ON).
    // Until Phase 3 lands, main() pre-signals a 250 kbps classic-CAN default.
    let cfg = crate::usb_task::CAN_CONFIG.wait().await;

    let mut configurator = can_cfg;
    configurator.set_bitrate(cfg.nominal_baud);

    // Apply FD data-phase configuration if requested.
    if let Some(fd_bt) = cfg.fd_timing {
        use core::num::{NonZeroU16, NonZeroU8};
        let data_timing = DataBitTiming {
            transceiver_delay_compensation: false,
            prescaler: NonZeroU16::new(fd_bt.brp as u16).unwrap_or(NonZeroU16::MIN),
            seg1: NonZeroU8::new(fd_bt.tseg1 as u8).unwrap_or(NonZeroU8::MIN),
            seg2: NonZeroU8::new(fd_bt.tseg2 as u8).unwrap_or(NonZeroU8::MIN),
            sync_jump_width: NonZeroU8::new(fd_bt.sjw as u8).unwrap_or(NonZeroU8::MIN),
        };
        let fdcan_cfg = configurator
            .config()
            .set_data_bit_timing(data_timing)
            .set_frame_transmit(FrameTransmissionConfig::AllowFdCanAndBRS)
            .set_non_iso_mode(!cfg.iso);
        configurator.set_config(fdcan_cfg);
    }

    #[cfg(not(feature = "loopback"))]
    let can = {
        let c = configurator.into_normal_mode();
        info!("FDCAN1: normal mode, {} kbps", cfg.nominal_baud / 1_000);
        lcd_terminal::boot_log!(
            crate::display_task::LOG_CHANNEL,
            "FDCAN1 ready (TJA1044 CN3 DB9)",
            lcd_terminal::BootStatus::Ok
        );
        c
    };
    #[cfg(feature = "loopback")]
    let can = {
        let c = configurator.into_internal_loopback_mode();
        info!(
            "FDCAN1: INTERNAL LOOPBACK mode, {} kbps",
            cfg.nominal_baud / 1_000
        );
        lcd_terminal::boot_log!(
            crate::display_task::LOG_CHANNEL,
            "FDCAN1 ready (LOOPBACK mode)",
            lcd_terminal::BootStatus::Ok
        );
        c
    };

    let (mut tx, mut rx, _) = can.split();

    // ── Phase 2 loopback self-test ────────────────────────────────────────────
    // Sends a known test frame and expects it back immediately via internal
    // loopback to verify clock/timing/frame-path without external hardware.
    #[cfg(feature = "loopback")]
    {
        use embassy_time::{with_timeout, Duration};
        const TEST_ID: u16 = 0x123;
        const TEST_DATA: [u8; 8] = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        if let Ok(test_frame) = Frame::new_standard(TEST_ID, &TEST_DATA) {
            tx.write(&test_frame).await;
            match with_timeout(Duration::from_millis(100), rx.read()).await {
                Ok(Ok(envelope)) => {
                    let (rx_frame, _) = envelope.parts();
                    if rx_frame.data() == &TEST_DATA {
                        info!(
                            "FDCAN self-test: PASS [ID={:#05x}, loopback RX matched TX]",
                            TEST_ID
                        );
                    } else {
                        error!("FDCAN self-test: FAIL [data mismatch]");
                    }
                }
                Ok(Err(e)) => error!("FDCAN self-test: FAIL [RX error: {:?}]", e),
                Err(_) => error!("FDCAN self-test: FAIL [timeout 100 ms — check 32 MHz PLL2Q]"),
            }
        }
    }

    let mut rx_seq: u16 = 0;
    let mut echo_seq: u16 = 0;
    let fd_mode = cfg.fd_timing.is_some();

    // Run CAN RX and TX paths as independent concurrent loops.
    // CanRx and CanTx are separate types returned by Can::split() — using them
    // in separate join branches has no shared mutable state.
    //
    // With the old select() approach, rx.read() could continuously "win" on a
    // busy bus and starve USB-initiated TX frames.  join() lets both make
    // progress independently: when the TX loop blocks on usb_to_can.receive(),
    // the RX loop can drain the FDCAN FIFO, and vice-versa.
    use embassy_futures::join::join;
    join(
        // ── RX: FDCAN1 → USB Bulk IN ─────────────────────────────────────────
        async {
            loop {
                // In FD mode, read_fd() handles both classic and FD frames.
                // In classic mode, read() is used (no FD flags in Header).
                let result = if fd_mode {
                    rx.read_fd().await.map(|e| {
                        let (frame, ts) = e.parts();
                        let h = frame.header();
                        let ts_100ns = (ts.as_nanos() / 100) as u32;
                        let kf = fd_frame_to_kcan(h, frame.data(), ts_100ns, rx_seq);
                        (kf, *h.id())
                    })
                } else {
                    rx.read().await.map(|e| {
                        let (frame, ts) = e.parts();
                        let h = frame.header();
                        let ts_100ns = (ts.as_nanos() / 100) as u32;
                        let kf = classic_to_kcan(&frame, ts_100ns, rx_seq);
                        (kf, *h.id())
                    })
                };

                match result {
                    Ok((mut kf, id)) => {
                        if let Id::Standard(sid) = id {
                            let raw = sid.as_raw() as usize;
                            crate::display_task::SEEN_IDS[raw >> 5]
                                .fetch_or(1u32 << (raw & 31), Ordering::Relaxed);
                        }
                        crate::display_task::RX_FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);
                        trace!(
                            "FDCAN RX [ID={:#010x}, DLC={}, FD={}]",
                            kf.can_id,
                            kf.dlc,
                            kf.flags & FrameFlags::FD != 0
                        );
                        kf.channel = 0;
                        rx_seq = rx_seq.wrapping_add(1);
                        #[cfg(not(feature = "periodic-echo"))]
                        if can_to_usb.try_send(kf).is_err() {
                            trace!("can_to_usb channel full — RX frame dropped");
                        }
                        #[cfg(feature = "periodic-echo")]
                        let _ = can_to_usb.try_send(kf);
                    }
                    Err(e) => {
                        warn!("FDCAN RX error: {:?}", e);
                    }
                }
            }
        },
        // ── TX: USB Bulk OUT → FDCAN1 ────────────────────────────────────────
        async {
            loop {
                let kf = usb_to_can.receive().await;
                // Drop TX frames silently when in listen-only (passive) mode.
                if crate::usb_task::LISTEN_ONLY.load(core::sync::atomic::Ordering::Relaxed) {
                    trace!("FDCAN TX suppressed (listen-only mode)");
                    continue;
                }
                let is_fd = kf.flags & FrameFlags::FD != 0;
                trace!(
                    "FDCAN TX [ID={:#010x}, DLC={}, FD={}]",
                    kf.can_id,
                    kf.dlc,
                    is_fd
                );
                let tx_ok = if is_fd && fd_mode {
                    match kcan_to_fd_frame(&kf) {
                        Some(frame) => {
                            with_timeout(Duration::from_millis(200), tx.write_fd(&frame))
                                .await
                                .is_ok()
                        }
                        None => {
                            warn!(
                                "FDCAN1 TX: bad FD frame — can_id={:#010x} dlc={}",
                                kf.can_id, kf.dlc
                            );
                            false
                        }
                    }
                } else {
                    match kcan_to_frame(&kf) {
                        Some(frame) => with_timeout(Duration::from_millis(200), tx.write(&frame))
                            .await
                            .is_ok(),
                        None => {
                            warn!(
                                "FDCAN1 TX: bad classic frame — can_id={:#010x} dlc={}",
                                kf.can_id, kf.dlc
                            );
                            false
                        }
                    }
                };
                if !tx_ok {
                    warn!("FDCAN1 TX timeout or error — possible Bus-Off, skipping frame");
                    continue;
                }
                let ts_100ns = (Instant::now().as_nanos() / 100) as u32;
                let echo = KCanFrame::new_tx_echo(
                    kf.can_id,
                    kf.flags,
                    kf.dlc,
                    &kf.data[..kf.dlc as usize],
                    ts_100ns,
                    echo_seq,
                );
                echo_seq = echo_seq.wrapping_add(1);
                let _ = can_to_usb.try_send(echo);
            }
        },
    )
    .await;
}

// ─── Frame conversion helpers ─────────────────────────────────────────────────

/// Convert a classic CAN `Frame` to a `KCanFrame`.
fn classic_to_kcan(frame: &Frame, timestamp_100ns: u32, seq: u16) -> KCanFrame {
    let header = frame.header();
    let (id_val, mut flags) = match header.id() {
        Id::Standard(id) => (id.as_raw() as u32, 0u8),
        Id::Extended(id) => (id.as_raw(), FrameFlags::EFF),
    };
    if header.rtr() {
        flags |= FrameFlags::RTR;
    }
    let data = frame.data();
    let dlc = header.len();
    KCanFrame::new_data(id_val, flags, dlc, data, timestamp_100ns, seq)
}

/// Convert an FD-mode receive header + data to a `KCanFrame`.
/// Works for both classic CAN frames and CAN FD frames received in FD mode.
fn fd_frame_to_kcan(header: &Header, data: &[u8], timestamp_100ns: u32, seq: u16) -> KCanFrame {
    let (id_val, mut flags) = match header.id() {
        Id::Standard(id) => (id.as_raw() as u32, 0u8),
        Id::Extended(id) => (id.as_raw(), FrameFlags::EFF),
    };
    if header.rtr() {
        flags |= FrameFlags::RTR;
    }
    if header.fdcan() {
        flags |= FrameFlags::FD;
    }
    if header.bit_rate_switching() {
        flags |= FrameFlags::BRS;
    }
    let dlc = header.len();
    KCanFrame::new_data(id_val, flags, dlc, data, timestamp_100ns, seq)
}

/// Build a classic CAN `Frame` from a `KCanFrame`.
fn kcan_to_frame(kf: &KCanFrame) -> Option<Frame> {
    let dlc = kf.dlc as usize;
    let is_eff = kf.flags & FrameFlags::EFF != 0;
    let is_rtr = kf.flags & FrameFlags::RTR != 0;

    if is_rtr {
        if is_eff {
            Frame::new_remote(ExtendedId::new(kf.can_id & 0x1FFF_FFFF).unwrap(), dlc).ok()
        } else {
            Frame::new_remote(StandardId::new(kf.can_id as u16 & 0x7FF).unwrap(), dlc).ok()
        }
    } else {
        let data = &kf.data[..dlc.min(8)];
        if is_eff {
            Frame::new_extended(kf.can_id, data).ok()
        } else {
            Frame::new_standard(kf.can_id as u16, data).ok()
        }
    }
}

/// Build a CAN FD `FdFrame` from a `KCanFrame`.
fn kcan_to_fd_frame(kf: &KCanFrame) -> Option<FdFrame> {
    let dlc = kf.dlc as usize;
    let is_eff = kf.flags & FrameFlags::EFF != 0;
    let brs = kf.flags & FrameFlags::BRS != 0;
    let data = &kf.data[..dlc.min(64)];

    let id: embedded_can::Id = if is_eff {
        ExtendedId::new(kf.can_id & 0x1FFF_FFFF)?.into()
    } else {
        StandardId::new(kf.can_id as u16 & 0x7FF)?.into()
    };
    let header = Header::new_fd(id, dlc as u8, false, brs);
    FdFrame::new(header, data).ok()
}
