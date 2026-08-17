use std::{fs, path::Path, sync::Arc};

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use thiserror::Error;
use zeroize::Zeroizing;

const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
const FORMAT_VERSION: u8 = 1;

#[derive(Clone)]
pub struct SecretCipher {
    key: Arc<Zeroizing<[u8; KEY_LENGTH]>>,
}

impl SecretCipher {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SecretError> {
        let bytes = Zeroizing::new(fs::read(path).map_err(SecretError::Read)?);
        Self::new(bytes.as_slice())
    }

    pub fn new(key: &[u8]) -> Result<Self, SecretError> {
        let key: [u8; KEY_LENGTH] = key.try_into().map_err(|_| SecretError::InvalidKeyLength)?;
        Ok(Self {
            key: Arc::new(Zeroizing::new(key)),
        })
    }

    pub fn encrypt(&self, plaintext: &[u8], context: &[u8]) -> Result<String, SecretError> {
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref().as_ref())
            .map_err(|_| SecretError::InvalidKeyLength)?;
        let mut nonce_bytes = [0_u8; NONCE_LENGTH];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad: context,
                },
            )
            .map_err(|_| SecretError::Encrypt)?;

        let mut envelope = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
        envelope.push(FORMAT_VERSION);
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(envelope))
    }

    pub fn decrypt(
        &self,
        envelope: &str,
        context: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        let envelope = URL_SAFE_NO_PAD
            .decode(envelope.as_bytes())
            .map_err(|_| SecretError::InvalidEnvelope)?;
        if envelope.len() <= 1 + NONCE_LENGTH || envelope[0] != FORMAT_VERSION {
            return Err(SecretError::InvalidEnvelope);
        }

        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref().as_ref())
            .map_err(|_| SecretError::InvalidKeyLength)?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&envelope[1..=NONCE_LENGTH]),
                Payload {
                    msg: &envelope[1 + NONCE_LENGTH..],
                    aad: context,
                },
            )
            .map_err(|_| SecretError::Decrypt)?;
        Ok(Zeroizing::new(plaintext))
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("could not read master encryption key: {0}")]
    Read(std::io::Error),
    #[error("master encryption key must contain exactly 32 bytes")]
    InvalidKeyLength,
    #[error("secret encryption failed")]
    Encrypt,
    #[error("secret envelope is invalid")]
    InvalidEnvelope,
    #[error("secret authentication or decryption failed")]
    Decrypt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn round_trip_requires_the_same_context() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher.encrypt(b"very secret", b"user:1:totp").unwrap();

        assert_ne!(encrypted, "very secret");
        assert_eq!(
            cipher
                .decrypt(&encrypted, b"user:1:totp")
                .unwrap()
                .as_slice(),
            b"very secret"
        );
        assert!(cipher.decrypt(&encrypted, b"user:2:totp").is_err());
    }

    #[test]
    fn wrong_key_decrypt_fails() {
        let cipher_a = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let cipher_b = SecretCipher::new(&[21_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher_a.encrypt(b"secret data", b"context").unwrap();

        assert!(cipher_b.decrypt(&encrypted, b"context").is_err());
    }

    #[test]
    fn same_plaintext_produces_different_ciphertext() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let enc1 = cipher.encrypt(b"identical", b"ctx").unwrap();
        let enc2 = cipher.encrypt(b"identical", b"ctx").unwrap();

        assert_ne!(enc1, enc2, "random nonce ensures ciphertext differs");
    }

    #[test]
    fn plaintext_never_appears_in_ciphertext() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let plaintext = b"unforgettable-plaintext-token";
        let encrypted = cipher.encrypt(plaintext, b"context").unwrap();

        assert!(!encrypted.contains("unforgettable-plaintext-token"));

        // Decode and verify plaintext bytes are absent from the envelope
        let decoded = URL_SAFE_NO_PAD.decode(encrypted.as_bytes()).unwrap();
        let plaintext_bytes = plaintext.to_vec();
        assert!(
            !decoded
                .windows(plaintext_bytes.len())
                .any(|window| window == plaintext_bytes),
            "plaintext must not appear in decoded envelope"
        );
    }

    #[test]
    fn ciphertext_tampering_detected() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher.encrypt(b"secret", b"context").unwrap();

        let mut decoded = URL_SAFE_NO_PAD.decode(encrypted.as_bytes()).unwrap();
        // Envelope: version(1) + nonce(12) + ciphertext+tag. Flip a byte in ciphertext.
        decoded[15] ^= 0xFF;
        let tampered = URL_SAFE_NO_PAD.encode(&decoded);

        assert!(cipher.decrypt(&tampered, b"context").is_err());
    }

    #[test]
    fn nonce_tampering_detected() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher.encrypt(b"secret", b"context").unwrap();

        let mut decoded = URL_SAFE_NO_PAD.decode(encrypted.as_bytes()).unwrap();
        // Flip a byte in the nonce region (bytes 1..13)
        decoded[5] ^= 0xFF;
        let tampered = URL_SAFE_NO_PAD.encode(&decoded);

        assert!(cipher.decrypt(&tampered, b"context").is_err());
    }

    #[test]
    fn version_tampering_detected() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher.encrypt(b"secret", b"context").unwrap();

        let mut decoded = URL_SAFE_NO_PAD.decode(encrypted.as_bytes()).unwrap();
        decoded[0] ^= 0xFF; // Corrupt the version byte
        let tampered = URL_SAFE_NO_PAD.encode(&decoded);

        assert!(cipher.decrypt(&tampered, b"context").is_err());
    }

    #[test]
    fn aad_context_tampering_detected() {
        let cipher = SecretCipher::new(&[7_u8; KEY_LENGTH]).unwrap();
        let encrypted = cipher.encrypt(b"secret", b"owner:1:record").unwrap();

        // Same ciphertext, different AAD — tag verification must fail
        assert!(cipher.decrypt(&encrypted, b"owner:2:record").is_err());
    }

    #[test]
    fn load_round_trips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("master.key");
        let key_bytes: [u8; 32] =
            core::array::from_fn(|i| u8::try_from(i).unwrap_or(0).wrapping_mul(3));
        std::fs::write(&key_path, key_bytes).unwrap();

        let cipher = SecretCipher::load(&key_path).unwrap();
        let encrypted = cipher
            .encrypt(b"file-based-secret", b"file:context")
            .unwrap();
        let decrypted = cipher.decrypt(&encrypted, b"file:context").unwrap();

        assert_eq!(decrypted.as_slice(), b"file-based-secret");
    }

    #[test]
    fn load_rejects_wrong_length_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("bad.key");
        std::fs::write(&key_path, [0_u8; 16]).unwrap();

        let result = SecretCipher::load(&key_path);
        assert!(matches!(result, Err(SecretError::InvalidKeyLength)));
    }
}
