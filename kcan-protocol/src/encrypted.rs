//! Encryption layer — types, trait, and software implementation.
//!
//! ## Protocol sketch
//!
//! Both sides hold an X25519 (Curve25519) keypair.  The device's ephemeral
//! public key is encoded in the QR code.  On connect the host sends its own
//! public key; both sides call `establish_session` and independently arrive at
//! the same 32-byte Diffie-Hellman shared secret — without ever transmitting it.
//! That secret is used directly as an AES-256-GCM key (v1 simplification;
//! HKDF domain separation is the obvious v2 improvement).
//!
//! Every subsequent [`KCanFrame`] is replaced by an [`EncryptedKCanFrame`],
//! whose 16-byte GCM tag provides both confidentiality and tamper detection.
//!
//! [`KCanFrame`]: crate::frame::KCanFrame

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

impl EncryptedKCanFrame {
    pub fn to_bytes(&self) -> [u8; ENCRYPTED_FRAME_SIZE] {
        let mut buf = [0u8; ENCRYPTED_FRAME_SIZE];
        buf[..80].copy_from_slice(&self.ciphertext);
        buf[80..96].copy_from_slice(&self.tag);
        buf[96..100].copy_from_slice(&self.seq.to_le_bytes());
        buf
    }

    pub fn from_bytes(buf: &[u8; ENCRYPTED_FRAME_SIZE]) -> Self {
        let mut ciphertext = [0u8; 80];
        let mut tag = [0u8; 16];
        ciphertext.copy_from_slice(&buf[..80]);
        tag.copy_from_slice(&buf[80..96]);
        let seq = u32::from_le_bytes(buf[96..100].try_into().unwrap());
        Self {
            ciphertext,
            tag,
            seq,
            _reserved: [0u8; 8],
        }
    }
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

// ─── Software implementation ──────────────────────────────────────────────────
//
// Compiled only when the `crypto` feature is enabled.  Works on both firmware
// (no_std) and host (std) — all operations are on-stack, no heap required.

#[cfg(feature = "crypto")]
use aes_gcm::{aead::AeadInPlace, Aes256Gcm, KeyInit};
#[cfg(feature = "crypto")]
use x25519_dalek::{PublicKey, StaticSecret};

/// Active session state: AES-256-GCM cipher keyed with the DH shared secret,
/// plus a monotonic TX nonce counter and last-seen RX seq for replay detection.
#[cfg(feature = "crypto")]
struct SessionState {
    cipher: Aes256Gcm,
    /// Next outgoing frame counter (TX nonce source).
    seq: u32,
    /// Last successfully decrypted incoming seq; `None` before first frame.
    last_rx_seq: Option<u32>,
}

/// Software X25519 + AES-256-GCM encryption layer.
///
/// Construct with `new(secret, is_initiator)` or `from_entropy(bytes, is_initiator)`.
/// `is_initiator = true` for the connecting party (host); `false` for the
/// listening party (device/server).  The role determines nonce direction
/// prefixes so TX nonces from one side never collide with TX nonces from the
/// other, even though both derive the same AES key.
#[cfg(feature = "crypto")]
pub struct SoftwareEncryptionLayer {
    /// Retained so we can return our own public key from `establish_session`.
    our_secret: StaticSecret,
    session: Option<SessionState>,
    /// Nonce byte 0 for outgoing frames (0 = server/device, 1 = client/host).
    tx_prefix: u8,
    /// Nonce byte 0 expected for incoming frames.
    rx_prefix: u8,
}

#[cfg(feature = "crypto")]
impl SoftwareEncryptionLayer {
    pub fn new(secret: StaticSecret, is_initiator: bool) -> Self {
        Self {
            our_secret: secret,
            session: None,
            tx_prefix: is_initiator as u8,
            rx_prefix: (!is_initiator) as u8,
        }
    }

    /// Convenience constructor: build from raw TRNG entropy without exposing
    /// the `x25519-dalek` types to the caller.
    pub fn from_entropy(entropy: [u8; 32], is_initiator: bool) -> Self {
        Self::new(StaticSecret::from(entropy), is_initiator)
    }

    /// Returns the local public key so the caller can send it to the remote
    /// party before (or instead of) calling `establish_session`.
    pub fn our_public_key(&self) -> [u8; 32] {
        PublicKey::from(&self.our_secret).to_bytes()
    }
}

#[cfg(feature = "crypto")]
impl EncryptionLayer for SoftwareEncryptionLayer {
    /// Run the ECDH handshake.
    ///
    /// Both sides independently compute `DH(our_secret, their_pubkey)` and
    /// arrive at identical 32 bytes — the shared secret — without it crossing
    /// the wire.  We use it directly as the AES-256 key (32 bytes in, 32 bytes
    /// needed).  Returns our public key so the caller can forward it to the
    /// remote party if needed.
    fn establish_session(&mut self, remote_pubkey: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
        let their_pub = PublicKey::from(*remote_pubkey);
        let shared = self.our_secret.diffie_hellman(&their_pub);
        // new_from_slice fails only if the slice length != 32, which can't happen here.
        let cipher =
            Aes256Gcm::new_from_slice(shared.as_bytes()).map_err(|_| CryptoError::HardwareFault)?;
        self.session = Some(SessionState {
            cipher,
            seq: 0,
            last_rx_seq: None,
        });
        Ok(PublicKey::from(&self.our_secret).to_bytes())
    }

    /// Encrypt one 80-byte KCAN frame in place and attach a 16-byte GCM tag.
    ///
    /// Nonce layout (12 bytes): byte 0 = direction prefix (0x00 device, 0x01 host),
    /// bytes 1–7 = zero, bytes 8–11 = `seq` LE (the low 32 bits).  The session
    /// key changes every connection, so a per-frame counter is safe.
    fn encrypt_frame(&mut self, frame: &[u8; 80]) -> Result<EncryptedKCanFrame, CryptoError> {
        let state = self
            .session
            .as_mut()
            .ok_or(CryptoError::SessionNotEstablished)?;
        // Refuse to wrap: nonce reuse under the same AES key breaks GCM.
        if state.seq == u32::MAX {
            return Err(CryptoError::HardwareFault);
        }
        let seq = state.seq;
        state.seq += 1;

        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[0] = self.tx_prefix;
        nonce_bytes[8..].copy_from_slice(&seq.to_le_bytes());
        let nonce = aes_gcm::aead::Nonce::<Aes256Gcm>::from_slice(&nonce_bytes);

        let mut ciphertext = *frame;
        let tag = state
            .cipher
            .encrypt_in_place_detached(nonce, &[], &mut ciphertext)
            .map_err(|_| CryptoError::HardwareFault)?;

        let mut tag_bytes = [0u8; 16];
        tag_bytes.copy_from_slice(&tag);

        Ok(EncryptedKCanFrame {
            ciphertext,
            tag: tag_bytes,
            seq,
            _reserved: [0u8; 8],
        })
    }

    /// Verify the GCM tag, check monotonic sequence, and decrypt.
    /// Returns `AuthenticationFailed` on tag mismatch or `ReplayDetected`
    /// if the incoming seq is not strictly greater than the last accepted seq.
    fn decrypt_frame(&mut self, enc: &EncryptedKCanFrame) -> Result<[u8; 80], CryptoError> {
        let state = self
            .session
            .as_mut()
            .ok_or(CryptoError::SessionNotEstablished)?;

        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[0] = self.rx_prefix;
        nonce_bytes[8..].copy_from_slice(&enc.seq.to_le_bytes());
        let nonce = aes_gcm::aead::Nonce::<Aes256Gcm>::from_slice(&nonce_bytes);
        // from_slice borrows the tag bytes in-place; no copy needed.
        let tag = aes_gcm::aead::Tag::<Aes256Gcm>::from_slice(&enc.tag);

        let mut plaintext = enc.ciphertext;
        state
            .cipher
            .decrypt_in_place_detached(nonce, &[], &mut plaintext, tag)
            .map_err(|_| CryptoError::AuthenticationFailed)?;

        // Monotonic seq check: reject replays and exact duplicates.
        if let Some(last) = state.last_rx_seq {
            if enc.seq <= last {
                return Err(CryptoError::ReplayDetected);
            }
        }
        state.last_rx_seq = Some(enc.seq);

        Ok(plaintext)
    }

    fn is_active(&self) -> bool {
        self.session.is_some()
    }
}
