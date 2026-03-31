//! Phase 3 encryption layer — interface stub.
//!
//! The implementation is deferred to the STM32H563 dongle.  This module
//! defines the trait boundary so both firmware and host can compile against
//! it today without any functional crypto code.
//!
//! When Phase 3 arrives:
//! - Firmware: `SaesEncryptionLayer` using STM32H563 hardware SAES/PKA/RNG.
//! - Host:     `SoftwareEncryptionLayer` using `aes-gcm` + `x25519-dalek`.
//! - Both implement `EncryptionLayer`; the `KCanAdapter` holds
//!   `Option<Box<dyn EncryptionLayer>>` — `None` in Phase 1/2.
//!
//! The encrypted frame replaces [`crate::frame::KCanFrame`] on the USB bulk
//! endpoints once a session has been established.

/// Error type for crypto operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// Key exchange has not been completed.
    SessionNotEstablished,
    /// AES-GCM authentication tag verification failed (tampered ciphertext).
    AuthenticationFailed,
    /// Replay protection: sequence number already seen.
    ReplayDetected,
    /// Internal hardware error (firmware side only).
    HardwareFault,
}

/// An encrypted KCAN frame on the USB bulk endpoints.
///
/// Fixed size so the USB transfer length is constant and predictable.
///
/// Layout (108 bytes):
/// | Offset | Size | Field        |
/// |--------|------|--------------|
/// | 0      | 80   | ciphertext   |
/// | 80     | 16   | AES-GCM tag  |
/// | 96     | 4    | sequence no  |
/// | 100    | 8    | reserved     |
pub const ENCRYPTED_FRAME_SIZE: usize = 108;

#[derive(Clone, Copy, Debug)]
pub struct EncryptedKCanFrame {
    pub ciphertext: [u8; 80],
    pub tag: [u8; 16],
    pub seq: u32,
    _reserved: [u8; 8],
}

/// Encryption layer interface.
///
/// Sits between the FDCAN FIFO and the USB Bulk IN write (firmware),
/// and between the USB Bulk IN read and `session.rs` (host).
pub trait EncryptionLayer {
    /// Complete the ECDH handshake using the remote party's public key.
    ///
    /// After this call, [`is_active`][Self::is_active] returns `true`.
    fn establish_session(&mut self, remote_pubkey: &[u8; 32]) -> Result<[u8; 32], CryptoError>;

    /// Encrypt one KCAN frame for transmission.
    fn encrypt_frame(&mut self, frame: &[u8; 80]) -> Result<EncryptedKCanFrame, CryptoError>;

    /// Decrypt one received encrypted frame.
    fn decrypt_frame(&mut self, enc: &EncryptedKCanFrame) -> Result<[u8; 80], CryptoError>;

    /// True once `establish_session` has completed successfully.
    fn is_active(&self) -> bool;
}
