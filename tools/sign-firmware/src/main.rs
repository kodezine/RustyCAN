//! KCAN firmware signing tool.
//!
//! # Usage
//!
//! ```text
//! # Generate a new Ed25519 keypair (one-time, store private key in CI secret)
//! sign-firmware --generate
//!
//! # Sign a raw firmware binary (appends 64-byte signature)
//! sign-firmware --key <hex-private-key> firmware.bin
//!   → produces firmware.bin.signed
//!
//! # Verify a signed binary
//! sign-firmware --verify firmware.bin.signed
//! ```
//!
//! The signed image format is simply:
//! ```text
//! [ raw binary payload (N bytes) ][ Ed25519 signature (64 bytes) ]
//! ```
//! The signature is computed over `SHA-512(payload)`, matching embassy-boot's
//! `verify_and_mark_updated` which pre-hashes the firmware with SHA-512 before
//! calling `verify()`.  The bootloader and host both verify by taking the last
//! 64 bytes as the signature and hashing the preceding bytes with SHA-512.

use std::path::PathBuf;
use std::process;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha512};

// ---------------------------------------------------------------------------
// Minimal CLI parsing (no clap dependency — keep the tool lightweight)
// ---------------------------------------------------------------------------

enum Cmd {
    Generate,
    Sign { key_hex: String, input: PathBuf },
    Verify { input: PathBuf },
}

fn parse_args() -> Cmd {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--generate" => Cmd::Generate,
        [flag, key, input] if flag == "--key" => Cmd::Sign {
            key_hex: key.clone(),
            input: PathBuf::from(input),
        },
        [flag, input] if flag == "--verify" => Cmd::Verify {
            input: PathBuf::from(input),
        },
        _ => {
            eprintln!(
                "Usage:\n  \
                 sign-firmware --generate\n  \
                 sign-firmware --key <hex-private-key> <firmware.bin>\n  \
                 sign-firmware --verify <firmware.bin.signed>"
            );
            process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_generate() {
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    let sk_hex = hex_encode(signing_key.to_bytes().as_ref());
    let vk_hex = hex_encode(verifying_key.as_bytes());

    println!("=== KCAN firmware signing keypair ===");
    println!();
    println!("PRIVATE KEY (store in GitHub Actions Secret KCAN_SIGNING_KEY):");
    println!("{sk_hex}");
    println!();
    println!("PUBLIC KEY (commit to firmware/signing-pubkey.bin as raw bytes):");
    println!("{vk_hex}");
    println!();
    println!("Writing firmware/signing-pubkey.bin …");

    let pubkey_path = std::path::Path::new("firmware/signing-pubkey.bin");
    if let Some(parent) = pubkey_path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_default();
    }
    std::fs::write(pubkey_path, verifying_key.as_bytes())
        .unwrap_or_else(|e| eprintln!("Warning: could not write signing-pubkey.bin: {e}"));

    println!("Done. Commit firmware/signing-pubkey.bin — it is the public key only.");
}

fn cmd_sign(key_hex: &str, input: &PathBuf) {
    // Decode private key.
    let key_bytes = hex_decode(key_hex).unwrap_or_else(|e| {
        eprintln!("Invalid key hex: {e}");
        process::exit(1);
    });
    if key_bytes.len() != 32 {
        eprintln!(
            "Private key must be exactly 32 bytes (got {})",
            key_bytes.len()
        );
        process::exit(1);
    }
    let key_arr: [u8; 32] = key_bytes.try_into().unwrap();
    let signing_key = SigningKey::from_bytes(&key_arr);

    // Read firmware binary.
    let payload = std::fs::read(input).unwrap_or_else(|e| {
        eprintln!("Cannot read {}: {e}", input.display());
        process::exit(1);
    });

    // Hash the payload with SHA-512 then sign the hash.
    // embassy-boot's verify_and_mark_updated pre-hashes the firmware with SHA-512
    // before calling verify(), so we must sign the hash — not the raw bytes.
    let hash = Sha512::digest(&payload);
    let signature: Signature = signing_key.sign(hash.as_slice());

    // Write payload + signature.
    let mut output_path = input.as_os_str().to_owned();
    output_path.push(".signed");
    let output_path = PathBuf::from(output_path);

    let mut out = payload.clone();
    out.extend_from_slice(signature.to_bytes().as_ref());
    std::fs::write(&output_path, &out).unwrap_or_else(|e| {
        eprintln!("Cannot write {}: {e}", output_path.display());
        process::exit(1);
    });

    println!(
        "OK — signed {} bytes → {} ({} bytes total)",
        payload.len(),
        output_path.display(),
        out.len()
    );
}

fn cmd_verify(input: &PathBuf) {
    let data = std::fs::read(input).unwrap_or_else(|e| {
        eprintln!("Cannot read {}: {e}", input.display());
        process::exit(1);
    });

    if data.len() < 64 {
        eprintln!("File too short to contain a signature (need ≥ 64 bytes)");
        process::exit(1);
    }

    let (payload, sig_bytes) = data.split_at(data.len() - 64);
    let sig_arr: [u8; 64] = sig_bytes.try_into().unwrap();
    let signature = Signature::from_bytes(&sig_arr);

    // Load public key from firmware/signing-pubkey.bin.
    let pubkey_bytes = std::fs::read("firmware/signing-pubkey.bin").unwrap_or_else(|e| {
        eprintln!("Cannot read firmware/signing-pubkey.bin: {e}");
        process::exit(1);
    });
    if pubkey_bytes.len() != 32 {
        eprintln!(
            "signing-pubkey.bin must be 32 bytes (got {})",
            pubkey_bytes.len()
        );
        process::exit(1);
    }
    let pk_arr: [u8; 32] = pubkey_bytes.try_into().unwrap();
    let verifying_key = VerifyingKey::from_bytes(&pk_arr).unwrap_or_else(|e| {
        eprintln!("Invalid public key: {e}");
        process::exit(1);
    });

    let hash = Sha512::digest(payload);
    verifying_key
        .verify(hash.as_slice(), &signature)
        .unwrap_or_else(|_| {
            eprintln!("FAIL — signature invalid or payload tampered");
            process::exit(1);
        });

    println!("OK — signature valid, payload {} bytes", payload.len());
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| format!("invalid hex at position {i}: {e}"))
        })
        .collect()
}

// ---------------------------------------------------------------------------

fn main() {
    match parse_args() {
        Cmd::Generate => cmd_generate(),
        Cmd::Sign { key_hex, input } => cmd_sign(&key_hex, &input),
        Cmd::Verify { input } => cmd_verify(&input),
    }
}
