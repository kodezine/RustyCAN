//! KCAN protocol — shared wire types.
//!
//! This crate is `#![no_std]` so it compiles for both:
//! - the STM32H753ZI / STM32H563 Embassy firmware (bare-metal)
//! - the RustyCAN host application (std, via the `std` feature)
//!
//! The canonical source of truth for the protocol is this crate.
//! Both the firmware and the host `KCanAdapter` import it directly,
//! so a change to the frame layout here is a compile error everywhere.

#![no_std]
#![cfg_attr(not(feature = "std"), allow(unused_imports))]

pub mod control;
pub mod encrypted;
pub mod frame;

pub use control::{
    KCanBitTiming, KCanBtConst, KCanDeviceInfo, KCanMode, KCanModeFlags, KCanStatus, RequestCode,
};
pub use frame::{FrameFlags, FrameType, KCanFrame, KCAN_FRAME_SIZE, KCAN_MAGIC, KCAN_VERSION};
