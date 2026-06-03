//! update-verify — Verifikasi signature & integritas OTA bundle
//! ============================================================
//!
//! Setiap OTA bundle UOS TV ditandatangani dengan Ed25519.
//! Public key di-embed saat build (disimpan di kernel cmdline/dtb).
//!
//! Alur verifikasi:
//!   1. Verify signature bundle metadata
//!   2. Verify file hash sesuai manifest
//!   3. Check version (no downgrade), timestamp (no expired), device compat

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Signature verification failed: {0}")]
    SignatureInvalid(String),

    #[error("Hash mismatch for '{file}': expected {expected}, got {actual}")]
    HashMismatch {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("Bundle expired (timestamp {timestamp}, max age {max_age_secs}s)")]
    Expired { timestamp: u64, max_age_secs: u64 },

    #[error("Bundle version {version} is older than current {current}")]
    Downgrade { version: String, current: String },

    #[error("Bundle not compatible with device {device}")]
    IncompatibleDevice { device: String },

    #[error("Invalid signature encoding: {0}")]
    InvalidSignature(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Metadata bundle OTA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMetadata {
    pub version: String,
    pub device: String,
    pub timestamp: u64,
    pub channel: String,
    #[serde(default)]
    pub critical: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub caibx_url: Option<String>,
    pub rootfs_sha256: String,
    pub rootfs_size: u64,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Signed envelope: data + signature (hex).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedBundle {
    /// Metadata JSON (sebagai Value agar tidak butuh Deserialize on T)
    pub data: serde_json::Value,

    /// Ed25519 signature dalam hex (128 chars)
    pub signature: String,

    /// Public key hex (64 chars) — optional
    #[serde(default)]
    pub public_key: Option<String>,
}

/// OTA Bundle Verifier.
pub struct BundleVerifier {
    trusted_keys: Vec<VerifyingKey>,
    max_age_secs: u64,
    allowed_devices: Vec<String>,
}

impl BundleVerifier {
    /// Buat verifier dengan public key tunggal (32 bytes).
    pub fn new(public_key_bytes: &[u8; 32]) -> Result<Self> {
        let key = VerifyingKey::from_bytes(public_key_bytes)
            .map_err(|e| Error::InvalidSignature(e.to_string()))?;
        Ok(Self {
            trusted_keys: vec![key],
            max_age_secs: 90 * 24 * 3600,
            allowed_devices: Vec::new(),
        })
    }

    pub fn add_key(&mut self, key_bytes: &[u8; 32]) -> Result<()> {
        let key = VerifyingKey::from_bytes(key_bytes)
            .map_err(|e| Error::InvalidSignature(e.to_string()))?;
        self.trusted_keys.push(key);
        Ok(())
    }

    pub fn set_max_age(&mut self, secs: u64) {
        self.max_age_secs = secs;
    }
    pub fn set_allowed_devices(&mut self, devices: Vec<String>) {
        self.allowed_devices = devices;
    }

    /// Verifikasi signed bundle: signature + parse metadata + checks.
    pub fn verify_bundle(
        &self,
        signed: &SignedBundle,
        current_version: Option<&str>,
    ) -> Result<BundleMetadata> {
        // 1. Verify signature
        let data_bytes = serde_json::to_vec(&signed.data)?;
        let sig_bytes =
            hex::decode(&signed.signature).map_err(|e| Error::InvalidSignature(e.to_string()))?;

        if sig_bytes.len() != 64 {
            return Err(Error::InvalidSignature("Expected 64 bytes".into()));
        }
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let signature = Signature::from_bytes(&sig_arr);

        let mut verified = false;
        for key in &self.trusted_keys {
            if key.verify_strict(&data_bytes, &signature).is_ok() {
                verified = true;
                break;
            }
        }
        if !verified {
            return Err(Error::SignatureInvalid("No trusted key matches".into()));
        }

        // 2. Parse metadata
        let meta: BundleMetadata = serde_json::from_value(signed.data.clone())?;

        // 3. Device check
        if !self.allowed_devices.is_empty() && !self.allowed_devices.contains(&meta.device) {
            return Err(Error::IncompatibleDevice {
                device: meta.device.clone(),
            });
        }

        // 4. Version check
        if let Some(current) = current_version {
            if is_downgrade(current, &meta.version) {
                return Err(Error::Downgrade {
                    version: meta.version.clone(),
                    current: current.to_string(),
                });
            }
        }

        // 5. Timestamp check
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if meta.timestamp + self.max_age_secs < now {
            return Err(Error::Expired {
                timestamp: meta.timestamp,
                max_age_secs: self.max_age_secs,
            });
        }

        Ok(meta)
    }

    /// Verifikasi hash file.
    pub fn verify_file_hash(path: &str, data: &[u8], expected_sha256: &str) -> Result<()> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let actual = hex::encode(&hasher.finalize()[..]);
        if actual != expected_sha256 {
            return Err(Error::HashMismatch {
                file: path.to_string(),
                expected: expected_sha256.to_string(),
                actual,
            });
        }
        Ok(())
    }

    /// Generate keypair + return hex-encoded keys (untuk development).
    pub fn generate_keypair_hex() -> (String, String) {
        use rand_core::OsRng;
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        (
            hex::encode(signing_key.to_bytes()),
            hex::encode(verifying_key.to_bytes()),
        )
    }

    /// Sign bundle metadata → SignedBundle.
    pub fn sign_bundle(signing_key: &SigningKey, meta: &BundleMetadata) -> Result<SignedBundle> {
        let data_json = serde_json::to_value(meta)?;
        let data_bytes = serde_json::to_vec(&data_json)?;
        let signature = signing_key.sign(&data_bytes);
        Ok(SignedBundle {
            data: data_json,
            signature: hex::encode(signature.to_bytes()),
            public_key: Some(hex::encode(signing_key.verifying_key().to_bytes())),
        })
    }
}

fn is_downgrade(current: &str, newer: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse().ok()).collect() };
    let c = parse(current);
    let n = parse(newer);
    if c.is_empty() || n.is_empty() {
        return false;
    }
    for i in 0..c.len().max(n.len()) {
        let cv = c.get(i).copied().unwrap_or(0);
        let nv = n.get(i).copied().unwrap_or(0);
        if nv < cv {
            return true;
        }
        if nv > cv {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify() {
        let (sk_hex, vk_hex) = BundleVerifier::generate_keypair_hex();
        let sk_bytes: [u8; 32] = hex::decode(&sk_hex).unwrap().try_into().unwrap();
        let vk_bytes: [u8; 32] = hex::decode(&vk_hex).unwrap().try_into().unwrap();

        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifier = BundleVerifier::new(&vk_bytes).unwrap();

        let meta = BundleMetadata {
            version: "1.0.0".into(),
            device: "qemu-virt".into(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            channel: "stable".into(),
            critical: false,
            description: "Test".into(),
            download_url: None,
            caibx_url: None,
            rootfs_sha256: "abc".into(),
            rootfs_size: 0,
            files: vec![],
        };

        let signed = BundleVerifier::sign_bundle(&signing_key, &meta).unwrap();
        let verified = verifier.verify_bundle(&signed, None).unwrap();
        assert_eq!(verified.version, "1.0.0");
    }

    #[test]
    fn test_downgrade() {
        assert!(is_downgrade("2.0.0", "1.0.0"));
        assert!(!is_downgrade("1.0.0", "2.0.0"));
        assert!(!is_downgrade("1.0.0", "1.0.0"));
    }
}
