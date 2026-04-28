//! Cross-channel physical bus test (feature = "bus-test").
//!
//! Verifies that a frame transmitted on FDCAN1 is received by FDCAN2, and
//! vice-versa, over the physical CAN bus wired between the two Waveshare
//! SN65HVD230 modules.
//!
//! # Test sequence
//!
//! 1. Wait 100 ms for both `can_task` instances to enter their run loops.
//! 2. Send a known frame (ID `0x7E1`, 8-byte payload) via `USB_TO_CAN`
//!    → FDCAN1 transmits → FDCAN2 should receive it → appears in
//!    `BUS_TEST_MONITOR` with `channel = 1`.
//! 3. Send a known frame (ID `0x7E2`) via `USB_TO_CAN2`
//!    → FDCAN2 transmits → FDCAN1 should receive it → appears in
//!    `BUS_TEST_MONITOR` with `channel = 0`.
//! 4. Both checks must complete within 500 ms; result is logged via defmt.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};
use kcan_protocol::frame::{FrameType, KCanFrame};

use defmt::*;

// Test frame identifiers — chosen outside normal traffic range.
const ID_1TO2: u32 = 0x7E1;
const ID_2TO1: u32 = 0x7E2;
const TEST_DATA: [u8; 8] = [0xBA, 0x5E, 0xCA, 0xFE, 0x01, 0x02, 0x03, 0x04];
const TIMEOUT: Duration = Duration::from_millis(500);

#[embassy_executor::task]
pub async fn bus_test_task(
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can2: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    monitor: &'static Channel<CriticalSectionRawMutex, KCanFrame, 8>,
) {
    // Allow both can_task instances to enter their main loops.
    Timer::after(Duration::from_millis(100)).await;
    info!("BUS TEST: starting cross-channel physical TX/RX verification");

    // ── Test 1: FDCAN1 → FDCAN2 ─────────────────────────────────────────────
    let mut frame = KCanFrame::new_tx(ID_1TO2, 0, 8, &TEST_DATA, 0);
    frame.channel = 0;
    usb_to_can.send(frame).await;

    let r1 = with_timeout(TIMEOUT, async {
        loop {
            let f = monitor.receive().await;
            // Accept only Data frames (not TxEcho) arriving on channel 1.
            if f.frame_type == FrameType::Data as u8
                && f.channel == 1
                && f.can_id == ID_1TO2
                && f.data[..8] == TEST_DATA
            {
                return true;
            }
        }
    })
    .await;

    match r1 {
        Ok(_) => info!("BUS TEST 1 (FDCAN1→FDCAN2): PASS  [ID={:#05x}]", ID_1TO2),
        Err(_) => error!("BUS TEST 1 (FDCAN1→FDCAN2): FAIL  [timeout — check wiring/termination]"),
    }

    // ── Test 2: FDCAN2 → FDCAN1 ─────────────────────────────────────────────
    let mut frame2 = KCanFrame::new_tx(ID_2TO1, 0, 8, &TEST_DATA, 1);
    frame2.channel = 1;
    usb_to_can2.send(frame2).await;

    let r2 = with_timeout(TIMEOUT, async {
        loop {
            let f = monitor.receive().await;
            if f.frame_type == FrameType::Data as u8
                && f.channel == 0
                && f.can_id == ID_2TO1
                && f.data[..8] == TEST_DATA
            {
                return true;
            }
        }
    })
    .await;

    match r2 {
        Ok(_) => info!("BUS TEST 2 (FDCAN2→FDCAN1): PASS  [ID={:#05x}]", ID_2TO1),
        Err(_) => error!("BUS TEST 2 (FDCAN2→FDCAN1): FAIL  [timeout — check wiring/termination]"),
    }

    // ── Summary ──────────────────────────────────────────────────────────────
    if r1.is_ok() && r2.is_ok() {
        info!("BUS TEST: ALL PASS — physical CAN bus verified on both channels");
    } else {
        error!("BUS TEST: FAILED — verify CANH/CANL wiring and 120 Ω termination");
    }
}
