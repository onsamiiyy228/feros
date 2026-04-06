//! AES-256-GCM encryption engine for credential secrets at rest.
//!
//! Each `encrypt()` call generates a random 12-byte nonce. The master key
//! is loaded from the `ENCRYPTION_KEY` environment variable (base64-encoded,
//! 32 bytes decoded).

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use rand::RngCore;
use thiserror::Error;

const NONCE_LEN: usize = 12;

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("encryption failed")]
    EncryptFailed,
    #[error("decryption failed")]
    DecryptFailed,
    #[error("invalid key length (expected 32 bytes, got {0})")]
    InvalidKeyLength(usize),
    #[error("invalid base64 encoding")]
    InvalidBase64,
    #[error("invalid JSON")]
    InvalidJson,
}

/// AES-256-GCM encryption engine.
///
/// Single master key derived from environment variable.
/// Each `encrypt()` call generates a random 12-byte nonce to ensure
/// ciphertext uniqueness even for identical plaintexts.
pub struct EncryptionEngine {
    cipher: Aes256Gcm,
}

impl EncryptionEngine {
    /// Create from a raw 32-byte key.
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new_from_slice(key).expect("32-byte key is always valid"),
        }
    }

    /// Create from a base64-encoded key string (e.g. from env var).
    ///
    /// # Errors
    ///
    /// Returns `InvalidBase64` if the string is not valid base64, or
    /// `InvalidKeyLength` if the decoded key is not exactly 32 bytes.
    pub fn from_base64(key_b64: &str) -> Result<Self, EncryptionError> {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(key_b64)
            .map_err(|_| EncryptionError::InvalidBase64)?;
        if key_bytes.len() != 32 {
            return Err(EncryptionError::InvalidKeyLength(key_bytes.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);
        let engine = Self::new(&key);
        // Zeroize local copy
        key.iter_mut().for_each(|b| *b = 0);
        Ok(engine)
    }

    /// Encrypt plaintext bytes → `(ciphertext_b64, nonce_b64)`.
    ///
    /// Both values are base64-encoded for direct storage in a TEXT column.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(String, String), EncryptionError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| EncryptionError::EncryptFailed)?;

        let ct_b64 = base64::engine::general_purpose::STANDARD.encode(&ciphertext);
        let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);

        Ok((ct_b64, nonce_b64))
    }

    /// Decrypt `(ciphertext_b64, nonce_b64)` → plaintext bytes.
    pub fn decrypt(
        &self,
        ciphertext_b64: &str,
        nonce_b64: &str,
    ) -> Result<Vec<u8>, EncryptionError> {
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(ciphertext_b64)
            .map_err(|_| EncryptionError::InvalidBase64)?;
        let nonce_bytes = base64::engine::general_purpose::STANDARD
            .decode(nonce_b64)
            .map_err(|_| EncryptionError::InvalidBase64)?;

        if nonce_bytes.len() != NONCE_LEN {
            return Err(EncryptionError::DecryptFailed);
        }

        let mut nonce_arr = [0u8; NONCE_LEN];
        nonce_arr.copy_from_slice(&nonce_bytes);
        let nonce = Nonce::from(nonce_arr);
        self.cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| EncryptionError::DecryptFailed)
    }

    /// Encrypt a JSON value → `(ciphertext_b64, nonce_b64)`.
    pub fn encrypt_json(
        &self,
        data: &serde_json::Value,
    ) -> Result<(String, String), EncryptionError> {
        let plaintext = serde_json::to_vec(data).map_err(|_| EncryptionError::InvalidJson)?;
        self.encrypt(&plaintext)
    }

    /// Decrypt `(ciphertext_b64, nonce_b64)` → JSON value.
    pub fn decrypt_json(
        &self,
        ciphertext_b64: &str,
        nonce_b64: &str,
    ) -> Result<serde_json::Value, EncryptionError> {
        let plaintext = self.decrypt(ciphertext_b64, nonce_b64)?;
        serde_json::from_slice(&plaintext).map_err(|_| EncryptionError::InvalidJson)
    }
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        key
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let engine = EncryptionEngine::new(&test_key());
        let plaintext = b"hello, secrets!";

        let (ct, nonce) = engine.encrypt(plaintext).unwrap();
        let decrypted = engine.decrypt(&ct, &nonce).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_json_roundtrip() {
        let engine = EncryptionEngine::new(&test_key());
        let data = serde_json::json!({
            "access_token": "sk-live-xxxxx",
            "refresh_token": "rt-yyyyy",
            "expires_in": 3600
        });

        let (ct, nonce) = engine.encrypt_json(&data).unwrap();
        let decrypted = engine.decrypt_json(&ct, &nonce).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let engine1 = EncryptionEngine::new(&test_key());
        let mut other_key = test_key();
        other_key[0] = 0xFF;
        let engine2 = EncryptionEngine::new(&other_key);

        let (ct, nonce) = engine1.encrypt(b"secret data").unwrap();
        let result = engine2.decrypt(&ct, &nonce);

        assert!(result.is_err());
    }

    #[test]
    fn nonce_uniqueness() {
        let engine = EncryptionEngine::new(&test_key());
        let plaintext = b"same data";

        let (ct1, nonce1) = engine.encrypt(plaintext).unwrap();
        let (ct2, nonce2) = engine.encrypt(plaintext).unwrap();

        // Same plaintext must produce different ciphertexts (different nonces)
        assert_ne!(ct1, ct2);
        assert_ne!(nonce1, nonce2);
    }

    #[test]
    fn from_base64_valid() {
        let key = test_key();
        let b64 = base64::engine::general_purpose::STANDARD.encode(key);
        let engine = EncryptionEngine::from_base64(&b64).unwrap();

        let (ct, nonce) = engine.encrypt(b"test").unwrap();
        let decrypted = engine.decrypt(&ct, &nonce).unwrap();
        assert_eq!(decrypted, b"test");
    }

    #[test]
    fn from_base64_wrong_length() {
        let result = EncryptionEngine::from_base64("dG9vIHNob3J0"); // "too short"
        assert!(matches!(result, Err(EncryptionError::InvalidKeyLength(_))));
    }

    #[test]
    fn from_base64_invalid_encoding() {
        let result = EncryptionEngine::from_base64("not!!!valid===base64");
        assert!(matches!(result, Err(EncryptionError::InvalidBase64)));
    }

    #[test]
    fn empty_plaintext() {
        let engine = EncryptionEngine::new(&test_key());
        let (ct, nonce) = engine.encrypt(b"").unwrap();
        let decrypted = engine.decrypt(&ct, &nonce).unwrap();
        assert!(decrypted.is_empty());
    }
}
