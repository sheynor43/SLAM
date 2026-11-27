//! Checking a downloaded file against its signature (ADR-0185): the signature is ed25519
//! over the SHA-256 digest of the file, written as 128 hex characters in `<file>.sig`.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// A key of all zeros: the project owner has not set the real one (see
/// [`super::UPDATE_PUBLIC_KEY`]).
pub const PLACEHOLDER_KEY: [u8; 32] = [0; 32];

/// Whether `key` is a real key and not the placeholder.
pub fn key_is_set(key: &[u8; 32]) -> bool {
    *key != PLACEHOLDER_KEY
}

/// Why a file was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    /// The key is the placeholder or not a curve point.
    NoKey,
    /// The `.sig` text is not 128 hex characters.
    BadFormat,
    /// The signature does not match the file.
    Mismatch,
}

/// SHA-256 of the file at `path`, read in blocks.
pub fn digest_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hasher.update(&block[..n]);
    }
    Ok(hasher.finalize().into())
}

/// The 64 signature bytes of the text of a `.sig` file.
pub fn parse_signature(text: &str) -> Result<[u8; 64], Reject> {
    let hex = text.trim();
    if hex.len() != 128 || !hex.is_ascii() {
        return Err(Reject::BadFormat);
    }
    let mut out = [0u8; 64];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| Reject::BadFormat)?;
    }
    Ok(out)
}

/// Whether `sig_text` is `key`'s signature of `digest`.
pub fn verify(key: &[u8; 32], digest: &[u8; 32], sig_text: &str) -> Result<(), Reject> {
    if !key_is_set(key) {
        return Err(Reject::NoKey);
    }
    let key = VerifyingKey::from_bytes(key).map_err(|_| Reject::NoKey)?;
    let signature = Signature::from_bytes(&parse_signature(sig_text)?);
    key.verify_strict(digest, &signature)
        .map_err(|_| Reject::Mismatch)
}

#[cfg(test)]
pub(crate) mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    /// A fixed test key pair: the signing key and the public key.
    pub fn test_key() -> (SigningKey, [u8; 32]) {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = signing.verifying_key().to_bytes();
        (signing, public)
    }

    pub fn sign_hex(signing: &SigningKey, digest: &[u8; 32]) -> String {
        use std::fmt::Write as _;
        signing
            .sign(digest)
            .to_bytes()
            .iter()
            .fold(String::new(), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            })
    }

    #[test]
    fn a_good_signature_verifies() {
        let (signing, public) = test_key();
        let digest: [u8; 32] = Sha256::digest(b"file").into();
        let sig = sign_hex(&signing, &digest);
        assert_eq!(verify(&public, &digest, &format!("{sig}\n")), Ok(()));
    }

    #[test]
    fn another_file_or_key_is_rejected() {
        let (signing, public) = test_key();
        let digest: [u8; 32] = Sha256::digest(b"file").into();
        let sig = sign_hex(&signing, &digest);
        let other: [u8; 32] = Sha256::digest(b"other").into();
        assert_eq!(verify(&public, &other, &sig), Err(Reject::Mismatch));
        let stranger = SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .to_bytes();
        assert_eq!(verify(&stranger, &digest, &sig), Err(Reject::Mismatch));
    }

    #[test]
    fn the_placeholder_key_never_verifies() {
        let (signing, _) = test_key();
        let digest: [u8; 32] = Sha256::digest(b"file").into();
        let sig = sign_hex(&signing, &digest);
        assert_eq!(verify(&PLACEHOLDER_KEY, &digest, &sig), Err(Reject::NoKey));
        assert!(!key_is_set(&PLACEHOLDER_KEY));
    }

    #[test]
    fn malformed_signature_text_is_rejected() {
        let (_, public) = test_key();
        let digest = [0u8; 32];
        for text in ["", "abcd", &"zz".repeat(64), &"é".repeat(64)] {
            assert_eq!(
                verify(&public, &digest, text),
                Err(Reject::BadFormat),
                "{text}"
            );
        }
    }

    #[test]
    fn the_file_digest_is_sha256() {
        let path = std::env::temp_dir().join(format!("slam-verify-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = digest_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let expected: [u8; 32] = Sha256::digest(b"abc").into();
        assert_eq!(digest, expected);
    }
}
