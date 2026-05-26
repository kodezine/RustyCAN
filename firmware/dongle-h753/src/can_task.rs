//! FDCAN task — receives CAN frames and forwards TX requests.
//!
//! Shared by FDCAN1 (channel 0) and FDCAN2 (channel 1) via `pool_size = 2`.

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

#[embassy_executor::task(pool_size = 2)]
pub async fn can_task(
    can_cfg: CanConfigurator<'static>,
    channel_id: u8,
    can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    #[cfg(feature = "bus-test")] bus_test_monitor: &'static Channel<
        CriticalSectionRawMutex,
        KCanFrame,
        8,
    >,
) {
    // Wait for the EP0 SET_MODE(BUS_ON) handler to signal configuration.
    let cfg = crate::usb_task::CAN_CONFIG.receive().await;

    let mut configurator = can_cfg;
    configurator.set_bitrate(cfg.nominal_baud);

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
        info!(
            "FDCAN{}: normal mode, {} kbps",
            channel_id + 1,
            cfg.nominal_baud / 1_000
        );
        c
    };
    #[cfg(feature = "loopback")]
    let can = {
        // Only FDCAN1 (channel 0) uses loopback for self-test.
        let c = if channel_id == 0 {
            configurator.into_internal_loopback_mode()
        } else {
            configurator.into_normal_mode()
        };
        info!(
            "FDCAN{}: {} mode, {} kbps",
            channel_id + 1,
            if channel_id == 0 {
                "LOOPBACK"
            } else {
                "normal"
            },
            cfg.nominal_baud / 1_000
        );
        c
    };

    let (mut tx, mut rx, _) = can.split();
    let fd_mode = cfg.fd_timing.is_some();

    // ── Phase 2 loopback self-test (FDCAN1 / channel 0 only) ─────────────────
    #[cfg(feature = "loopback")]
    if channel_id == 0 {
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
                Err(_) => error!("FDCAN self-test: FAIL [timeout 100 ms]"),
            }
        }
    }

    let mut rx_seq: u16 = 0;
    let mut echo_seq: u16 = 0;

    use embassy_futures::join::join;
    join(
        // ── RX: FDCAN → USB Bulk IN ───────────────────────────────────────────
        async {
            loop {
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
                    Ok((mut kf, _id)) => {
                        trace!(
                            "FDCAN{} RX [ID={:#010x}, DLC={}, FD={}]",
                            channel_id + 1,
                            kf.can_id,
                            kf.dlc,
                            kf.flags & FrameFlags::FD != 0
                        );
                        kf.channel = channel_id;
                        rx_seq = rx_seq.wrapping_add(1);
                        #[cfg(feature = "bus-test")]
                        let _ = bus_test_monitor.try_send(kf);
                        #[cfg(not(feature = "periodic-echo"))]
                        if can_to_usb.try_send(kf).is_err() {
                            trace!(
                                "can_to_usb channel full — FDCAN{} RX frame dropped",
                                channel_id + 1
                            );
                        }
                        #[cfg(feature = "periodic-echo")]
                        let _ = can_to_usb.try_send(kf);
                    }
                    Err(e) => {
                        warn!("FDCAN{} RX error: {:?}", channel_id + 1, e);
                    }
                }
            }
        },
        // ── TX: USB Bulk OUT → FDCAN ──────────────────────────────────────────
        async {
            loop {
                let kf = usb_to_can.receive().await;
                if crate::usb_task::LISTEN_ONLY.load(Ordering::Relaxed) {
                    trace!("FDCAN{} TX suppressed (listen-only)", channel_id + 1);
                    continue;
                }
                let is_fd = kf.flags & FrameFlags::FD != 0;
                let tx_ok = if is_fd && fd_mode {
                    match kcan_to_fd_frame(&kf) {
                        Some(frame) => {
                            with_timeout(Duration::from_millis(200), tx.write_fd(&frame))
                                .await
                                .is_ok()
                        }
                        None => {
                            warn!(
                                "FDCAN{} TX: bad FD frame id={:#010x}",
                                channel_id + 1,
                                kf.can_id
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
                                "FDCAN{} TX: bad classic frame id={:#010x}",
                                channel_id + 1,
                                kf.can_id
                            );
                            false
                        }
                    }
                };
                if !tx_ok {
                    warn!(
                        "FDCAN{} TX timeout/error — possible Bus-Off",
                        channel_id + 1
                    );
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

fn classic_to_kcan(frame: &Frame, timestamp_100ns: u32, seq: u16) -> KCanFrame {
    let header = frame.header();
    let (id_val, mut flags) = match header.id() {
        Id::Standard(id) => (id.as_raw() as u32, 0u8),
        Id::Extended(id) => (id.as_raw(), FrameFlags::EFF),
    };
    if header.rtr() {
        flags |= FrameFlags::RTR;
    }
    KCanFrame::new_data(
        id_val,
        flags,
        header.len(),
        frame.data(),
        timestamp_100ns,
        seq,
    )
}

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
    KCanFrame::new_data(id_val, flags, header.len(), data, timestamp_100ns, seq)
}

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
    FdFrame::new(Header::new_fd(id, dlc as u8, false, brs), data).ok()
}
