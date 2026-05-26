//! KCAN EP0 control request types.
//!
//! All requests use `bmRequestType = 0x40` (vendor, device, host→device)
//! for writes and `0xC0` (vendor, device, device→host) for reads.
//!
//! # Request codes (`bRequest`)
//!
//! | Code   | Name                   | Dir            | Payload type        |
//! |--------|------------------------|----------------|---------------------|
//! | `0x01` | `GET_INFO`             | Device→Host    | [`KCanDeviceInfo`]  |
//! | `0x02` | `SET_BITTIMING`        | Host→Device    | [`KCanBitTiming`]   |
//! | `0x03` | `SET_FD_BITTIMING`     | Host→Device    | [`KCanBitTiming`]   |
//! | `0x04` | `SET_MODE`             | Host→Device    | [`KCanMode`]        |
//! | `0x05` | `GET_STATUS`           | Device→Host    | [`KCanStatus`]      |
//! | `0x06` | `GET_BT_CONST`         | Device→Host    | [`KCanBtConst`]     |
//! | `0x10` | `CRYPTO_HELLO`         | Both           | 32-byte public key  |
//! | `0x11` | `GET_IDENTITY`         | Device→Host    | variable cert blob  |

/// USB vendor request codes for KCAN EP0 control transfers.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestCode {
    GetInfo = 0x01,
    SetBitTiming = 0x02,
    SetFdBitTiming = 0x03,
    SetMode = 0x04,
    GetStatus = 0x05,
    GetBtConst = 0x06,
    /// Phase 3: initiate ECDH key exchange.
    CryptoHello = 0x10,
    /// Phase 3: retrieve device identity certificate.
    GetIdentity = 0x11,
}

// ─── Device info (GET_INFO response) ─────────────────────────────────────────

/// 12-byte response to `GET_INFO`.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct KCanDeviceInfo {
    /// Major version of the KCAN firmware.
    pub fw_major: u8,
    /// Minor version of the KCAN firmware.
    pub fw_minor: u8,
    /// Patch version of the KCAN firmware.
    pub fw_patch: u8,
    /// Number of CAN channels (always 1 for Phase 1 dongle).
    pub channels: u8,
    /// KCAN protocol version this firmware speaks (currently 1).
    pub protocol_version: u8,
    _reserved: [u8; 3],
    /// Lower 32 bits of the STM32 96-bit unique device ID.
    pub uid_lo: u32,
}

impl KCanDeviceInfo {
    pub fn new(fw_major: u8, fw_minor: u8, fw_patch: u8, uid_lo: u32) -> Self {
        Self {
            fw_major,
            fw_minor,
            fw_patch,
            channels: 1,
            protocol_version: 1,
            _reserved: [0; 3],
            uid_lo,
        }
    }

    pub fn to_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = self.fw_major;
        b[1] = self.fw_minor;
        b[2] = self.fw_patch;
        b[3] = self.channels;
        b[4] = self.protocol_version;
        // b[5..8] reserved zeros
        b[8..12].copy_from_slice(&self.uid_lo.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 12]) -> Self {
        Self {
            fw_major: b[0],
            fw_minor: b[1],
            fw_patch: b[2],
            channels: b[3],
            protocol_version: b[4],
            _reserved: [0; 3],
            uid_lo: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        }
    }
}

// ─── Bit timing (SET_BITTIMING / SET_FD_BITTIMING) ───────────────────────────

/// 16-byte bit-timing parameters.
///
/// The host computes these from the user-selected bitrate using the
/// constraints returned by `GET_BT_CONST`.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KCanBitTiming {
    /// Baud-rate prescaler.
    pub brp: u32,
    /// Time segment 1 (propagation + phase1), in time quanta.
    pub tseg1: u32,
    /// Time segment 2 (phase2), in time quanta.
    pub tseg2: u32,
    /// Synchronisation jump width, in time quanta.
    pub sjw: u32,
}

impl KCanBitTiming {
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.brp.to_le_bytes());
        b[4..8].copy_from_slice(&self.tseg1.to_le_bytes());
        b[8..12].copy_from_slice(&self.tseg2.to_le_bytes());
        b[12..16].copy_from_slice(&self.sjw.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 16]) -> Self {
        Self {
            brp: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            tseg1: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            tseg2: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            sjw: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        }
    }

    /// Compute bit timing for a given bitrate given the FDCAN kernel clock.
    ///
    /// Uses fixed TSEG1=13, TSEG2=2, SJW=1 (appropriate for CANopen).
    /// BRP is derived as `clock_hz / (bitrate * (1 + TSEG1 + TSEG2))`.
    ///
    /// Returns `None` if the bitrate is not achievable with integer BRP.
    pub fn for_bitrate(clock_hz: u32, bitrate: u32) -> Option<Self> {
        let tq_per_bit: u32 = 16; // 1 + tseg1 + tseg2 = 1 + 13 + 2
        let brp = clock_hz / (bitrate * tq_per_bit);
        if brp == 0 || brp > 512 {
            return None;
        }
        // Verify the bitrate is exact.
        let actual = clock_hz / (brp * tq_per_bit);
        if actual != bitrate {
            return None;
        }
        Some(Self {
            brp,
            tseg1: 13,
            tseg2: 2,
            sjw: 1,
        })
    }

    /// Compute CAN FD **data-phase** bit timing for a given data bitrate.
    ///
    /// Uses fixed TSEG1=11, TSEG2=4, SJW=4 (16 TQ per bit).  These values
    /// satisfy the FDCAN data-phase hardware limits (TSEG1 ≤ 31, TSEG2 ≤ 15,
    /// SJW ≤ 15) and the data-phase BRP cap of 31.
    ///
    /// Returns `None` if the bitrate is not achievable (non-integer BRP, BRP=0,
    /// or BRP > 31).
    pub fn for_fd_data_bitrate(clock_hz: u32, data_bitrate: u32) -> Option<Self> {
        let tq_per_bit: u32 = 16; // 1 + tseg1 + tseg2 = 1 + 11 + 4
        let brp = clock_hz / (data_bitrate * tq_per_bit);
        if brp == 0 || brp > 31 {
            return None;
        }
        let actual = clock_hz / (brp * tq_per_bit);
        if actual != data_bitrate {
            return None;
        }
        Some(Self {
            brp,
            tseg1: 11,
            tseg2: 4,
            sjw: 4,
        })
    }
}

// ─── Mode (SET_MODE) ─────────────────────────────────────────────────────────

/// Bitfield flags for [`KCanMode`].
pub struct KCanModeFlags;

impl KCanModeFlags {
    /// Take the bus on-line. Must be set together with the mode bits.
    pub const BUS_ON: u8 = 1 << 0;
    /// Listen-only: no ACK bits / no TX frames generated by the dongle.
    pub const LISTEN_ONLY: u8 = 1 << 1;
    /// Internal loopback: TX frames loop back to RX without hitting the bus.
    pub const LOOPBACK: u8 = 1 << 2;
    /// Take the bus off-line (release). Clears BUS_ON.
    pub const BUS_OFF: u8 = 1 << 3;
}

/// Bitfield flags for the `fd_flags` byte of [`KCanMode`] (byte 1).
pub struct KCanModeFdFlags;

impl KCanModeFdFlags {
    /// Enable CAN FD mode with bit-rate switching (BRS).  If clear, the
    /// firmware operates in classic CAN mode regardless of any previously
    /// configured FD data-phase timing.
    pub const FD_ENABLED: u8 = 1 << 0;
    /// Use non-ISO CAN FD framing (Bosch original, CCCR.NISO=1).
    /// When clear, ISO 11898-1:2015 framing is used.
    pub const NON_ISO: u8 = 1 << 1;
}

/// 4-byte payload sent with `SET_MODE`.
///
/// Wire layout:
/// ```text
/// byte 0  flags    (KCanModeFlags)
/// byte 1  fd_flags (KCanModeFdFlags) — zero on classic-CAN hosts, ignored by classic firmware
/// byte 2  reserved
/// byte 3  reserved
/// ```
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct KCanMode {
    pub flags: u8,
    /// CAN FD flags. Older firmware treats this as reserved (zero) and ignores it.
    pub fd_flags: u8,
    _reserved: [u8; 2],
}

impl KCanMode {
    /// Classic CAN bus-on.  `fd_flags` is zero (backward compatible).
    pub fn bus_on(listen_only: bool, loopback: bool) -> Self {
        let mut flags = KCanModeFlags::BUS_ON;
        if listen_only {
            flags |= KCanModeFlags::LISTEN_ONLY;
        }
        if loopback {
            flags |= KCanModeFlags::LOOPBACK;
        }
        Self {
            flags,
            fd_flags: 0,
            _reserved: [0; 2],
        }
    }

    /// CAN FD bus-on with explicit FD flags.
    pub fn bus_on_fd(listen_only: bool, loopback: bool, fd_flags: u8) -> Self {
        let mut flags = KCanModeFlags::BUS_ON;
        if listen_only {
            flags |= KCanModeFlags::LISTEN_ONLY;
        }
        if loopback {
            flags |= KCanModeFlags::LOOPBACK;
        }
        Self {
            flags,
            fd_flags,
            _reserved: [0; 2],
        }
    }

    pub fn bus_off() -> Self {
        Self {
            flags: KCanModeFlags::BUS_OFF,
            fd_flags: 0,
            _reserved: [0; 2],
        }
    }

    pub fn to_bytes(&self) -> [u8; 4] {
        [self.flags, self.fd_flags, 0, 0]
    }

    pub fn from_bytes(b: &[u8; 4]) -> Self {
        Self {
            flags: b[0],
            fd_flags: b[1],
            _reserved: [0; 2],
        }
    }
}

// ─── Status (GET_STATUS response) ────────────────────────────────────────────

/// Bus state returned in `GET_STATUS`.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusState {
    Off = 0,
    Active = 1,
    Warning = 2,
    Passive = 3,
    BusOff = 4,
}

/// 12-byte response to `GET_STATUS`.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct KCanStatus {
    pub rx_errors: u8,
    pub tx_errors: u8,
    pub bus_state: u8,
    _reserved: u8,
    /// Current TIM2 timestamp counter value (µs since bus-on).
    pub current_timestamp_us: u32,
    /// Total frames received since bus-on.
    pub rx_count: u32,
}

impl KCanStatus {
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = self.rx_errors;
        b[1] = self.tx_errors;
        b[2] = self.bus_state;
        b[4..8].copy_from_slice(&self.current_timestamp_us.to_le_bytes());
        b[8..12].copy_from_slice(&self.rx_count.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 12]) -> Self {
        Self {
            rx_errors: b[0],
            tx_errors: b[1],
            bus_state: b[2],
            _reserved: 0,
            current_timestamp_us: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            rx_count: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        }
    }
}

// ─── Bit-timing constraints (GET_BT_CONST response) ──────────────────────────

/// 32-byte response to `GET_BT_CONST`.
///
/// The host uses these constraints to compute a valid [`KCanBitTiming`]
/// for any user-selected bitrate.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct KCanBtConst {
    /// FDCAN kernel clock frequency in Hz (e.g. 64_000_000).
    pub clock_hz: u32,
    pub brp_min: u32,
    pub brp_max: u32,
    pub tseg1_min: u32,
    pub tseg1_max: u32,
    pub tseg2_min: u32,
    pub tseg2_max: u32,
    pub sjw_max: u32,
}

impl KCanBtConst {
    /// Constants for STM32H743XI with 32 MHz FDCAN kernel clock (PLL2Q = 320 MHz / 10).
    pub const H743_32MHZ: Self = Self {
        clock_hz: 32_000_000,
        brp_min: 1,
        brp_max: 512,
        tseg1_min: 2,
        tseg1_max: 256,
        tseg2_min: 2,
        tseg2_max: 128,
        sjw_max: 128,
    };

    /// Constants for STM32H753ZI with 32 MHz FDCAN kernel clock (PLL2Q = 320 MHz / 10).
    ///
    /// # Note
    /// The name `H753_64MHZ` is a misnomer — the actual clock is 32 MHz.
    /// Use [`KCanBtConst::H743_32MHZ`] or [`KCanBtConst::H753_32MHZ`] in new code.
    #[deprecated(note = "Use H753_32MHZ — the clock is 32 MHz, not 64 MHz")]
    pub const H753_64MHZ: Self = Self {
        clock_hz: 32_000_000,
        brp_min: 1,
        brp_max: 512,
        tseg1_min: 2,
        tseg1_max: 256,
        tseg2_min: 2,
        tseg2_max: 128,
        sjw_max: 128,
    };

    pub fn to_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&self.clock_hz.to_le_bytes());
        b[4..8].copy_from_slice(&self.brp_min.to_le_bytes());
        b[8..12].copy_from_slice(&self.brp_max.to_le_bytes());
        b[12..16].copy_from_slice(&self.tseg1_min.to_le_bytes());
        b[16..20].copy_from_slice(&self.tseg1_max.to_le_bytes());
        b[20..24].copy_from_slice(&self.tseg2_min.to_le_bytes());
        b[24..28].copy_from_slice(&self.tseg2_max.to_le_bytes());
        b[28..32].copy_from_slice(&self.sjw_max.to_le_bytes());
        b
    }

    /// Constants for STM32H753ZI with 32 MHz FDCAN kernel clock (PLL2Q = 320 MHz / 10).
    pub const H753_32MHZ: Self = Self {
        clock_hz: 32_000_000,
        brp_min: 1,
        brp_max: 512,
        tseg1_min: 2,
        tseg1_max: 256,
        tseg2_min: 2,
        tseg2_max: 128,
        sjw_max: 128,
    };

    pub fn from_bytes(b: &[u8; 32]) -> Self {
        Self {
            clock_hz: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            brp_min: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            brp_max: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            tseg1_min: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            tseg1_max: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
            tseg2_min: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            tseg2_max: u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
            sjw_max: u32::from_le_bytes([b[28], b[29], b[30], b[31]]),
        }
    }
}

// ─── FD configuration (firmware-internal, EP0 → can_task) ────────────────────

/// CAN FD configuration assembled by the EP0 handler from `SET_BITTIMING`,
/// `SET_FD_BITTIMING`, and `SET_MODE` and forwarded to `can_task` via a signal.
///
/// Not a wire format — no `repr(C)`.  Firmware only.
#[derive(Clone, Copy, Debug)]
pub struct KCanFdConfig {
    /// Nominal (arbitration) baud rate in bits/s.
    pub nominal_baud: u32,
    /// Data-phase bit timing, or `None` for classic CAN mode.
    pub fd_timing: Option<KCanBitTiming>,
    /// `true` = ISO 11898-1:2015 (CCCR.NISO=0); `false` = non-ISO (CCCR.NISO=1).
    pub iso: bool,
    /// Bus mode flags forwarded from `KCanModeFlags` (listen-only, loopback).
    pub mode_flags: u8,
}
