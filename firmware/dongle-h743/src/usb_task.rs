//! USB tasks.
//!
//! Two tasks:
//! 1. `usb_device_task`  — runs `UsbDevice::run()` forever (handles enumeration,
//!    suspend/resume, EP0 standard requests).
//! 2. `kcan_io_task`     — bridges Bulk IN/OUT to the CAN channels:
//!    - Bulk OUT → `usb_to_can` channel (TX frames from host)
//!    - `can_to_usb` channel → Bulk IN (RX frames + TX echoes to host)
//!
//! The `USB_CONFIGURED` signal gates endpoint I/O: set true when the host
//! sends SET_CONFIGURATION, false on disconnect/reset.

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embassy_usb::driver::EndpointError;
use embassy_usb::UsbDevice;

use kcan_protocol::frame::{KCanFrame, KCAN_FRAME_SIZE};

use defmt::*;

/// Signalled true when USB is configured (host set configuration),
/// false when the device is reset/disconnected.
pub static USB_CONFIGURED: Signal<CriticalSectionRawMutex, bool> = Signal::new();

/// Mirrors USB_CONFIGURED for the status LED task.
/// A separate Signal so that both kcan_io_task and status_task can each
/// have their own exclusive waiter (embassy_sync::Signal supports only one).
pub static USB_CONFIGURED_LED: Signal<CriticalSectionRawMutex, bool> = Signal::new();

/// Fired by the SET_MODE EP0 handler on every host open() call.
/// The kcan_io_task responds by flushing the OTG EP1 TX FIFO via PAC,
/// which clears any stale EPENA=1 state left by a previously dropped
/// write_packet() future, then restarts bulk IO cleanly.
pub static BULK_RESTART: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[embassy_executor::task]
pub async fn usb_device_task(
    mut usb: UsbDevice<
        'static,
        embassy_stm32::usb::Driver<'static, embassy_stm32::peripherals::USB_OTG_FS>,
    >,
) {
    // Hold SDIS=1 for 3 s before releasing SDIS=0 via usb.run()/Bus::init().
    //
    // Why: DSTS.EERR fires when macOS sends SOFs to a device with TRDT=0.
    // If macOS was mid-SOF (from a previous failed enumeration attempt),
    // releasing SDIS=0 immediately sends those SOFs to an uninitialised OTG
    // core (TRDT=0) → 4 consecutive CRC errors → EERR within 5 ms.  Once
    // EERR=1, macOS stops all communication; the device is stuck until reset.
    //
    // Fix: keep SDIS=1 for 3 s so macOS fully de-registers the device in
    // IOKit (clearing port-isolation).  When SDIS=0 is released at 3 s:
    //   1. macOS is fresh — it needs 50–100 ms to detect D+ and start USBRST.
    //   2. OTG core sees ESUSP (no SOF in those 100 ms) but NO CRC errors
    //      (macOS hasn't sent any packets yet) → EERR does NOT fire.
    //   3. macOS sends USBRST → Embassy processes: init_fifo (GRXFSIZ→62),
    //      ENUMDNE handler sets TRDT=6 → GET_DESCRIPTOR succeeds → enumerated.
    //
    // The 3 s SDIS=1 period is provided by simply not calling usb.run() yet:
    // Driver::new_fs() + builder.build() leave SDIS=1 with the PHY idle.
    // No pre-Embassy register manipulation needed; the hardware is quiescent.
    defmt::info!("USB: 3 s SDIS=1 disconnect (IOKit flush)…");
    Timer::after(Duration::from_secs(3)).await;

    let ahb1enr_before = embassy_stm32::pac::RCC.ahb1enr().read().usb_otg_fsen();
    defmt::info!(
        "USB: AHB1ENR.usb_otg_fsen before run() = {} — releasing SDIS=0",
        ahb1enr_before
    );

    usb.run().await;
}

/// Periodically dumps raw OTG register values and decoded key bits.
///
/// After a successful Bus::init() the following should all hold:
///   GCCFG.PWRDWN     = 1  → FS PHY transceiver powered up
///   GUSBCFG.PHYSEL   = 1  → internal FS PHY selected (not ULPI)
///   GOTGCTL.BVALOEN  = 1  → B-valid override enabled (vbus_detection=false)
///   GOTGCTL.BVALOVAL = 1  → B-valid forced high (session active)
///   DCTL.SDIS        = 0  → soft-connected, D+ pulled high
///   CID              = 0x00002300 → Synopsys OTG rev for H743 OTG_FS
#[embassy_executor::task]
pub async fn usb_diag_task() {
    // ── TRDT continuous enforcer (0–3 s window) ─────────────────────────────
    //
    // TRDT fix: Embassy's configure_as_device() does GUSBCFG.write() which
    // zeros TRDT on (a) Bus::init() and (b) ENUMDNE handler.  We poll every
    // 5 ms for 3 s and force TRDT=6 whenever Embassy zeroes it.  TRDT=6 is
    // required for 200 MHz AHB (HCLK); with TRDT=0 IN token responses arrive
    // too early → CRC errors.
    //
    // EERR behaviour: DSTS.EERR fires within 5 ms of SDIS=0 release because
    // macOS needs 50–100 ms to detect D+ and start SOFs, but the OTG core
    // declares ESUSP after only 3 ms of no-SOF.  Per RM0433 Table 1050,
    // EERR is cleared by a USB reset from the host — so the correct action
    // is to keep SDIS=0 and wait: macOS will eventually send USBRST which
    // clears EERR and allows normal enumeration.  We log EERR but take no
    // recovery action (no CSRST, no sys_reset — both were tried and make
    // things worse by breaking Embassy state or cycling infinitely).
    {
        use embassy_stm32::pac::USB_OTG_FS as OTG;

        // ── Immediate snapshot BEFORE first timer tick ────────────────────────
        {
            let dsts = OTG.dsts().read().0;
            let dctl = OTG.dctl().read().0;
            let gintsts = OTG.gintsts().read().0;
            let suspsts = dsts & 1; // bit 0 = SUSPSTS
            let eerr = (dsts >> 3) & 1; // bit 3 = EERR
            let sdis = (dctl >> 1) & 1; // bit 1 = SDIS
            defmt::info!(
                "TRDT [pre-tick]: DSTS={:08x} SUSPSTS={} EERR={} SDIS={} GINTSTS={:08x}",
                dsts,
                suspsts,
                eerr,
                sdis,
                gintsts
            );
        }
        let mut prev_gintsts = OTG.gintsts().read().0;
        let mut eerr_logged = false;

        for tick in 0u16..600 {
            Timer::after(Duration::from_millis(5)).await;

            // ── TRDT enforcer ────────────────────────────────────────────────
            let trdt = OTG.gusbcfg().read().trdt();
            if trdt != 6 {
                OTG.gusbcfg().modify(|w| w.set_trdt(6));
                defmt::warn!("TRDT enforcer [{}ms]: was {} → forced 6", tick * 5, trdt);
            }

            // ── GINTSTS edge detection ────────────────────────────────────────
            {
                let cur_gintsts = OTG.gintsts().read().0;
                let diff = cur_gintsts ^ prev_gintsts;
                if diff != 0 {
                    defmt::info!(
                        "GINTSTS @{}ms: {:08x} → {:08x} (new bits {:08x})",
                        tick * 5,
                        prev_gintsts,
                        cur_gintsts,
                        diff
                    );
                    prev_gintsts = cur_gintsts;
                }
            }

            // ── EERR detector ────────────────────────────────────────────────
            // DSTS.EERR fires within ~5 ms of SDIS=0 release because macOS
            // needs 50–100 ms to detect D+ and send SOFs, but the OTG core
            // declares ESUSP after only 3 ms of no-SOF → EERR.
            //
            // Per RM0433 Table 1050: EERR is cleared by USB reset from host.
            // Correct response: keep SDIS=0 and wait — macOS will eventually
            // send USBRST which clears EERR and allows normal enumeration.
            // No recovery action needed here (CSRST and sys_reset were tried
            // and both make things worse; see session notes).
            let dsts_val = OTG.dsts().read().0;
            let eerr = (dsts_val >> 3) & 1; // bit 3 = EERR
            let suspsts = dsts_val & 1; // bit 0 = SUSPSTS
            if eerr != 0 && !eerr_logged {
                defmt::warn!(
                    "EERR at tick={} ({}ms): DSTS={:08x} SUSPSTS={} — waiting for macOS USBRST",
                    tick,
                    tick * 5,
                    dsts_val,
                    suspsts
                );
                eerr_logged = true;
            }
            if eerr == 0 && eerr_logged {
                defmt::info!(
                    "EERR cleared at tick={} ({}ms) — macOS USBRST received",
                    tick,
                    tick * 5
                );
                eerr_logged = false;
            }

            // Periodic state every 25 ticks (125 ms) — reduced verbosity.
            if tick % 25 == 0 {
                let dad = OTG.dcfg().read().dad();
                let grxfsiz = OTG.grxfsiz().read().rxfd();
                let gintsts = OTG.gintsts().read().0;
                let dsts2 = OTG.dsts().read().0;
                let suspsts2 = dsts2 & 1;
                let eerr2 = (dsts2 >> 3) & 1;
                defmt::debug!(
                    "tick={} DAD={} GRXFSIZ={} GINTSTS={:08x} DSTS={:08x} SUSPSTS={} EERR={}",
                    tick,
                    dad,
                    grxfsiz,
                    gintsts,
                    dsts2,
                    suspsts2,
                    eerr2
                );
            }
            if tick == 120 {
                let cid = OTG.cid().read().0;
                let pwrdwn = OTG.gccfg_v2().read().pwrdwn();
                let physel = OTG.gusbcfg().read().physel();
                let trdt_now = OTG.gusbcfg().read().trdt();
                let bvaloen = OTG.gotgctl().read().bvaloen();
                let bvaloval = OTG.gotgctl().read().bvaloval();
                let sdis = OTG.dctl().read().sdis();
                let doepctl0 = OTG.doepctl(0).read().0;
                let doeptsiz0 = OTG.doeptsiz(0).read().0;
                let diepctl0 = OTG.diepctl(0).read().0;
                let dieptsiz0 = OTG.dieptsiz(0).read().0;
                let grxfsiz = OTG.grxfsiz().read().rxfd();
                let dieptxf0 = OTG.dieptxf0().read().0;
                let dieptxf1 = OTG.dieptxf(0).read().0;
                let gintsts = OTG.gintsts().read().0;
                let gintmsk = OTG.gintmsk().read().0;
                let dsts = OTG.dsts().read().0;
                let dcfg = OTG.dcfg().read().0;
                defmt::info!("=== OTG INIT CHECK (600 ms post-boot) ===");
                defmt::info!(
                    "  CID={:08x} PWRDWN={} PHYSEL={} TRDT={}",
                    cid,
                    pwrdwn,
                    physel,
                    trdt_now
                );
                defmt::info!(
                    "  BVALOEN={} BVALOVAL={} SDIS={} GRXFSIZ={}",
                    bvaloen,
                    bvaloval,
                    sdis,
                    grxfsiz
                );
                defmt::info!("  DOEPCTL0={:08x} DOEPTSIZ0={:08x}", doepctl0, doeptsiz0);
                defmt::info!("  DIEPCTL0={:08x} DIEPTSIZ0={:08x}", diepctl0, dieptsiz0);
                defmt::info!("  DIEPTXF0={:08x} DIEPTXF1={:08x}", dieptxf0, dieptxf1);
                defmt::info!("  GINTSTS={:08x} GINTMSK={:08x}", gintsts, gintmsk);
                defmt::info!("  DSTS={:08x} DCFG={:08x}", dsts, dcfg);
                if !pwrdwn {
                    defmt::warn!("  !! PWRDWN=0: FS PHY is POWERED DOWN");
                }
                if !physel {
                    defmt::warn!("  !! PHYSEL=0: ULPI path selected — wrong");
                }
                if trdt_now != 6 {
                    defmt::warn!("  !! TRDT={} (expect 6)", trdt_now);
                }
                if sdis {
                    defmt::warn!("  !! SDIS=1: device is SOFT-DISCONNECTED");
                }
                let rxflvlm = (gintmsk >> 4) & 1;
                let oepint_msk = (gintmsk >> 19) & 1;
                let iepint_msk = (gintmsk >> 18) & 1;
                if rxflvlm == 0 {
                    defmt::warn!("  !! GINTMSK.RXFLVLM=0 — RX FIFO interrupt masked!");
                }
                if oepint_msk == 0 {
                    defmt::warn!("  !! GINTMSK.OEPINT=0 — OUT endpoint interrupt masked!");
                }
                if iepint_msk == 0 {
                    defmt::warn!("  !! GINTMSK.IEPINT=0 — IN endpoint interrupt masked!");
                }
                let eerr = (dsts >> 3) & 1;
                if eerr != 0 {
                    defmt::warn!("  !! DSTS.EERR=1 — OTG core erratic error!");
                }
            }
        }
        defmt::info!(
            "TRDT enforcer: 3 s window closed, TRDT={}",
            OTG.gusbcfg().read().trdt()
        );
    }

    // ── Enumeration watchdog ──────────────────────────────────────────────────
    // Runs after the 3 s TRDT enforcer window.  Checks every 10 s:
    //
    //   DAD != 0: SET_ADDRESS received — enumeration succeeded, stop.
    //
    //   GRXFSIZ == 62 && DAD == 0:
    //     USBRST was received (init_fifo ran) — the device is connected and
    //     waiting for GET_DESCRIPTOR.  macOS may be in suspend/resume cycle.
    //     DO NOT disconnect (SDIS=1) — this would abort enumeration-in-progress.
    //     Log registers every 10 s and reset after 120 s with no progress.
    //
    //   GRXFSIZ == 1024 && DAD == 0:
    //     No USBRST from host at all.  Wait for physical replug.
    {
        use embassy_stm32::pac::USB_OTG_FS as OTG;

        // No initial wait: start checking immediately after TRDT window closes.
        let mut no_progress_count = 0u8;
        loop {
            // Log current state and wait 10 s before re-checking.
            let dad = OTG.dcfg().read().dad();
            let grxfsiz = OTG.grxfsiz().read().rxfd();
            let dsts = OTG.dsts().read().0;
            let gintsts = OTG.gintsts().read().0;
            let trdt = OTG.gusbcfg().read().trdt();
            defmt::info!(
                "ENUM WATCHDOG: DAD={} GRXFSIZ={} DSTS={:08x} GINTSTS={:08x} TRDT={}",
                dad,
                grxfsiz,
                dsts,
                gintsts,
                trdt
            );

            if dad != 0 {
                defmt::info!(
                    "ENUM WATCHDOG: DAD={} — SET_ADDRESS received, enumeration succeeded",
                    dad
                );
                break;
            }

            if grxfsiz == 62 {
                // USBRST was received and configure_endpoints ran (GRXFSIZ=62).
                // Embassy is armed and waiting for GET_DESCRIPTOR.
                //
                // DO NOT sys_reset() here: each reset re-triggers a failed
                // enumeration attempt, which resets macOS IOKit's 10-minute
                // "problematic device" isolation timer.  The device ends up
                // permanently cached as failed.  Just wait — macOS will retry
                // after its isolation period expires.
                no_progress_count += 1;
                defmt::warn!(
                    "ENUM WATCHDOG: DAD=0, GRXFSIZ=62 — waiting for GET_DESCRIPTOR (count={})",
                    no_progress_count
                );
            } else {
                // GRXFSIZ still 1024: macOS has not sent a bus reset yet.
                // This happens with IOKit port-isolation: macOS's host controller
                // sends SOFs (keepalives) but IOKit doesn't initiate enumeration.
                // Only a physical cable replug fully clears IOKit state.
                // Wait patiently — do NOT sys_reset() on EERR here; EERR will
                // clear when macOS eventually sends USBRST.  After 120 s of
                // no progress, sys_reset() as a signal to the user.
                let eerr = (dsts >> 3) & 1;
                defmt::warn!(
                    "ENUM WATCHDOG: DAD=0, GRXFSIZ=1024 EERR={} count={} — awaiting USBRST (physical replug may be needed)",
                    eerr, no_progress_count
                );
                no_progress_count += 1;
                if no_progress_count >= 12 {
                    // 12 × 10 s = 120 s without progress — give up and reset.
                    defmt::warn!("ENUM WATCHDOG: 120 s without USBRST — sys_reset()");
                    cortex_m::peripheral::SCB::sys_reset();
                }
            }

            // Wait 10 s between watchdog checks.  During this wait, log any
            // GINTSTS transitions and trigger remote wakeup if suspended.
            // After USBRST on a dock/hub, macOS suspends the port within 5ms
            // (hub power-save policy).  GET_DESCRIPTOR requires the port to be
            // active, so we assert DCTL.RWUSIG (resume signalling, 1–15ms) to
            // wake the host — this prompts macOS to resume and start enumeration.
            {
                let mut prev_g = OTG.gintsts().read().0;
                let mut wakeup_sent = false;
                for i in 0u16..100 {
                    // 100 × 100 ms = 10 s
                    Timer::after(Duration::from_millis(100)).await;
                    let cur_g = OTG.gintsts().read().0;
                    // Trigger remote wakeup ~500 ms after suspend (bit 11 = USBSUSP,
                    // bit 10 = ESUSP).  DSTS.SUSPSTS must be 1 first; RWUSIG held
                    // 1–15 ms then cleared.
                    if !wakeup_sent && i == 5 {
                        let dsts_now = OTG.dsts().read().0;
                        let suspsts = dsts_now & 1;
                        if suspsts != 0 || (cur_g & 0xC00) != 0 {
                            defmt::info!("WATCHDOG: asserting DCTL.RWUSIG (remote wakeup) to unsuspend macOS");
                            OTG.dctl().modify(|w| w.set_rwusig(true));
                            Timer::after(Duration::from_millis(5)).await;
                            OTG.dctl().modify(|w| w.set_rwusig(false));
                            wakeup_sent = true;
                        }
                    }
                    if cur_g != prev_g {
                        defmt::info!(
                            "WATCHDOG GINTSTS: {:08x} → {:08x} (diff {:08x})",
                            prev_g,
                            cur_g,
                            cur_g ^ prev_g
                        );
                        prev_g = cur_g;
                    }
                    let d = OTG.dcfg().read().dad();
                    if d != 0 {
                        defmt::info!("WATCHDOG: DAD={} — SET_ADDRESS succeeded!", d);
                        break;
                    }
                }
            }
        }
    }

    loop {
        let (gintsts, gintmsk, dsts, dcfg, gusbcfg, pcgcctl, gotgctl, gccfg) = {
            use embassy_stm32::pac::USB_OTG_FS as OTG;
            (
                OTG.gintsts().read().0,
                OTG.gintmsk().read().0,
                OTG.dsts().read().0,
                OTG.dcfg().read().0,
                OTG.gusbcfg().read().0,
                OTG.pcgcctl().read().0,
                OTG.gotgctl().read().0,
                OTG.gccfg_v2().read().0,
            )
        };
        defmt::info!(
            "OTG: GINTSTS={:08x} GINTMSK={:08x} DSTS={:08x} DCFG={:08x}",
            gintsts,
            gintmsk,
            dsts,
            dcfg
        );
        defmt::info!(
            "OTG: GUSBCFG={:08x} PCGCCTL={:08x} GOTGCTL={:08x} GCCFG={:08x}",
            gusbcfg,
            pcgcctl,
            gotgctl,
            gccfg
        );
        Timer::after(Duration::from_secs(2)).await;
    }
}

#[embassy_executor::task]
pub async fn kcan_io_task(
    class: crate::kcan_usb::KCanUsbClass<
        'static,
        embassy_stm32::usb::Driver<'static, embassy_stm32::peripherals::USB_OTG_FS>,
    >,
    can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can2: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    let (mut sender, mut receiver) = class.split();
    info!("USB: Bulk IN/OUT task started — waiting for host configuration");

    loop {
        // Wait until the host physically configures the device (SET_CONFIGURATION).
        // This is only ever signalled by the configured() callback, NOT by SET_MODE.
        loop {
            if USB_CONFIGURED.wait().await {
                break;
            }
        }
        info!("USB: host configured — starting Bulk IN/OUT");

        // Inner restart loop: handles successive BULK_RESTART events (one per
        // host open() call) without going back through USB_CONFIGURED.wait().
        // Only a USB disconnect (EndpointError::Disabled from run) breaks out
        // of this loop and forces a fresh USB_CONFIGURED.wait().
        'restart: loop {
            // Run IN and OUT concurrently; break out on Disabled (disconnect)
            // or when SET_MODE fires BULK_RESTART (host reconnect without USB reset).
            let run = async {
                use embassy_futures::join::join;
                join(
                    // Bulk IN: can_to_usb → host.
                    async {
                        loop {
                            let frame = can_to_usb.receive().await;
                            let bytes = frame.to_bytes();
                            match sender.write_packet(&bytes).await {
                                Ok(()) => {}
                                Err(EndpointError::Disabled) => {
                                    break;
                                }
                                Err(e) => warn!("USB Bulk IN write error: {:?}", e),
                            }
                        }
                    },
                    // Bulk OUT: host → usb_to_can.
                    async {
                        let mut buf = [0u8; KCAN_FRAME_SIZE];
                        loop {
                            match receiver.read_packet(&mut buf).await {
                                Ok(n) if n == KCAN_FRAME_SIZE => {
                                    let arr: [u8; KCAN_FRAME_SIZE] = buf;
                                    if let Some(frame) = KCanFrame::from_bytes(&arr) {
                                        let ch = match frame.channel {
                                            0 => usb_to_can.try_send(frame),
                                            1 => usb_to_can2.try_send(frame),
                                            c => {
                                                warn!("USB Bulk OUT: unknown channel {}", c);
                                                Ok(())
                                            }
                                        };
                                        if ch.is_err() {
                                            warn!("usb_to_can channel full — TX frame dropped");
                                        }
                                    } else {
                                        warn!("USB Bulk OUT: bad magic/version");
                                    }
                                }
                                Ok(n) => warn!("USB Bulk OUT: unexpected size {}", n),
                                Err(EndpointError::Disabled) => break,
                                Err(e) => warn!("USB Bulk OUT read error: {:?}", e),
                            }
                        }
                    },
                )
                .await
            };

            match select(run, BULK_RESTART.wait()).await {
                Either::First(_) => {
                    info!("USB: disconnected — waiting for reconnect");
                    break 'restart; // Re-enter outer USB_CONFIGURED.wait() loop.
                }
                Either::Second(_) => {
                    // SET_MODE fired: a new host session is starting.  The dropped
                    // write_packet() future may have left EP1 with EPENA=1 and stale
                    // data in the TX FIFO.  The next write() call would then block
                    // forever in Phase-1 waiting for EPENA=0.
                    //
                    // Fix: use the PAC to abort any in-progress IN transfer on EP1,
                    // flush its TX FIFO, and re-enable the endpoint.  This is safe
                    // because kcan_io_task is the only user of EP1 and the write_packet
                    // future was just dropped by select().
                    info!("USB: BULK_RESTART — flushing stale EP1 TX state");
                    let r = embassy_stm32::pac::USB_OTG_FS;
                    // If a transfer is in-progress (EPENA=1), abort it with SNAK+EPDIS.
                    if r.diepctl(1).read().epena() {
                        r.diepctl(1).modify(|w| {
                            w.set_snak(true);
                            w.set_epdis(true);
                        });
                        // Spin until the endpoint disable takes effect (EPENA clears).
                        while r.diepctl(1).read().epena() {}
                    }
                    // Flush the TX FIFO for EP1.
                    r.grstctl().write(|w| {
                        w.set_txfflsh(true);
                        w.set_txfnum(1);
                    });
                    while r.grstctl().read().txfflsh() {}
                    // Increase EP1 TX FIFO from 16 words (64 bytes) to 24 words (96 bytes).
                    //
                    // The OTG FS DTXFSTS counter lags one packet depth behind XFRC:
                    // after the host ACKs the 4-word [64..80] chunk, DTXFSTS shows
                    // FIFO_SIZE - 4 = 12 rather than 16.  Embassy's write() Phase-2
                    // check (size_words=16 > fifo_space=12) then spins on the TXFE
                    // interrupt continuously, starving usb_device_task and causing the
                    // host's transfer_blocking calls to be cancelled indefinitely.
                    //
                    // With FIFO_SIZE=24: DTXFSTS after XFRC = 24-4 = 20 ≥ 16, so the
                    // Phase-2 check succeeds immediately.  Safe here because EP1 FIFO
                    // was just flushed (empty) and words 78-101 are unallocated.
                    let sa = r.dieptxf(0).read().sa();
                    r.dieptxf(0).modify(|w| {
                        w.set_sa(sa);
                        w.set_fd(24);
                    });
                    // Clear SNAK and reset data toggle to DATA0, matching the host's
                    // toggle reset from clear_halt(BULK_IN_EP) in kcan.rs open().
                    r.diepctl(1).modify(|w| {
                        w.set_cnak(true);
                        w.set_sd0pid_sevnfrm(true);
                    });
                    info!("USB: EP1 flushed (FIFO=24w, DATA0) — restarting bulk IO");
                    // Continue 'restart — no USB_CONFIGURED.wait() needed.
                }
            }
        }
    }
}
