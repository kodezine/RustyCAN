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
    usb.run().await;
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
