//! Signs release files for SLAM's self-update (ADR-0185).
//!
//! `sign-release sign FILE...` writes `FILE.sig` next to each file: an ed25519 signature of
//! the SHA-256 digest of the file, as 128 hex characters. The key is the base64 of the raw
//! 32-byte seed in the environment variable `SLAM_UPDATE_KEY`.
//!
//! `sign-release public` prints the matching public key, base64 and hex, from the same
//! variable.

use std::fmt::Write as _;
use std::process::ExitCode;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const KEY_VARIABLE: &str = "SLAM_UPDATE_KEY";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The signing key of the base64 text of a raw 32-byte seed.
fn key_from(text: &str) -> Result<SigningKey, String> {
    let seed = STANDARD
        .decode(text.trim())
        .map_err(|e| format!("{KEY_VARIABLE} is not base64: {e}"))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| format!("{KEY_VARIABLE} must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The signature text of `data`.
fn sign_text(key: &SigningKey, data: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(data).into();
    hex(&key.sign(&digest).to_bytes())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or("usage: sign-release sign FILE... | public")?;
    let text = std::env::var(KEY_VARIABLE).map_err(|_| format!("{KEY_VARIABLE} is not set"))?;
    let key = key_from(&text)?;
    match command.as_str() {
        "public" => {
            let public = key.verifying_key().to_bytes();
            println!(
                "base64: {}\nhex:    {}",
                STANDARD.encode(public),
                hex(&public)
            );
            Ok(())
        }
        "sign" => {
            let files: Vec<String> = args.collect();
            if files.is_empty() {
                return Err("no files to sign".to_owned());
            }
            for file in files {
                let data = std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?;
                let signature = sign_text(&key, &data);
                std::fs::write(format!("{file}.sig"), format!("{signature}\n"))
                    .map_err(|e| format!("{file}.sig: {e}"))?;
                println!("signed {file}");
            }
            Ok(())
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sign-release: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_in_base64_gives_a_signature_the_public_key_verifies() {
        let seed = STANDARD.encode([5u8; 32]);
        let key = key_from(&format!("{seed}\n")).unwrap();
        let text = sign_text(&key, b"release file");
        assert_eq!(text.len(), 128);
        let bytes: Vec<u8> = (0..64)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let signature = ed25519_dalek::Signature::from_bytes(&bytes.try_into().unwrap());
        let digest: [u8; 32] = Sha256::digest(b"release file").into();
        assert!(
            key.verifying_key()
                .verify_strict(&digest, &signature)
                .is_ok()
        );
    }

    #[test]
    fn a_bad_seed_is_refused() {
        assert!(key_from("not base64!").is_err());
        assert!(key_from(&STANDARD.encode([1u8; 31])).is_err());
    }
}
