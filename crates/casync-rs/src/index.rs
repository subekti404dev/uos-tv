//! CaiBxIndex — Content-Addressable Index format.
//!
//! Format .caibx:
//! ```cbor
//! {
//!   "magic": "CAIBX",
//!   "version": 1,
//!   "chunk_size_target": 65536,
//!   "total_size": 1048576,
//!   "chunks": [
//!     {"offset": 0, "size": 65536, "sha256": h'...'},
//!     {"offset": 65536, "size": 65536, "sha256": h'...'},
//!     ...
//!   ]
//! }
//! ```
//!
//! Index digunakan untuk:
//!   1. Mengetahui chunk mana yang dibutuhkan untuk reassemble file
//!   2. Membandingkan dengan index lama → delta computation
//!   3. Verifikasi integritas file setelah reassembly

use crate::chunker::{Chunk, Chunker};
use crate::{ChunkHash, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Magic bytes untuk file .caibx
const CAIBX_MAGIC: &str = "CAIBX";
const CAIBX_VERSION: u32 = 1;

/// Satu entry chunk dalam index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkEntry {
    /// Offset dalam file asli
    pub offset: u64,

    /// Ukuran chunk (bytes)
    pub size: u32,

    /// SHA-256 hash (32 bytes, disimpan sebagai byte array)
    #[serde(with = "hex_serde")]
    pub sha256: ChunkHash,
}

/// Index file format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaiBxIndex {
    /// Magic string "CAIBX"
    magic: String,

    /// Format version
    version: u32,

    /// Target chunk size (digunakan saat index dibuat)
    pub chunk_size_target: u32,

    /// Total ukuran file yang di-index (bytes)
    pub total_size: u64,

    /// Daftar chunk entries (diurutkan oleh offset)
    pub chunks: Vec<ChunkEntry>,
}

impl CaiBxIndex {
    /// Buat index baru dari daftar chunk.
    pub fn from_chunks(chunks: &[Chunk], chunk_size_target: u32) -> Self {
        let total_size: u64 = chunks.iter().map(|c| c.size as u64).sum();

        let entries: Vec<ChunkEntry> = chunks
            .iter()
            .map(|c| ChunkEntry {
                offset: c.offset,
                size: c.size,
                sha256: c.hash,
            })
            .collect();

        Self {
            magic: CAIBX_MAGIC.to_string(),
            version: CAIBX_VERSION,
            chunk_size_target,
            total_size,
            chunks: entries,
        }
    }

    /// Buat index dari file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        let chunker = Chunker::new();
        let chunks = chunker.chunk_bytes(&data);
        Ok(Self::from_chunks(&chunks, crate::DEFAULT_CHUNK_SIZE as u32))
    }

    /// Serialize index ke CBOR bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    /// Deserialize index dari CBOR bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let index: Self = ciborium::from_reader(data)?;

        // Validate magic
        if index.magic != CAIBX_MAGIC {
            return Err(Error::InvalidIndex(format!(
                "Invalid magic: {}",
                index.magic
            )));
        }

        // Validate version
        if index.version != CAIBX_VERSION {
            return Err(Error::InvalidIndex(format!(
                "Unsupported version: {}",
                index.version
            )));
        }

        // Validate chunks sorted
        for i in 1..index.chunks.len() {
            if index.chunks[i].offset <= index.chunks[i - 1].offset {
                return Err(Error::InvalidIndex("Chunks not sorted by offset".into()));
            }
        }

        Ok(index)
    }

    /// Simpan index ke file.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let data = self.to_bytes()?;
        std::fs::write(path, &data)?;
        Ok(())
    }

    /// Load index dari file.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::from_bytes(&data)
    }

    /// Bandingkan dua index — return set chunk hash yang ada di `new`
    /// tapi TIDAK ada di `old`. Ini adalah chunk yang harus di-download
    /// untuk delta update.
    pub fn diff(old: &Self, new: &Self) -> HashSet<ChunkHash> {
        let old_hashes: HashSet<ChunkHash> = old.chunks.iter().map(|c| c.sha256).collect();

        let new_hashes: HashSet<ChunkHash> = new.chunks.iter().map(|c| c.sha256).collect();

        // Chunk yang ada di new tapi tidak di old
        new_hashes.difference(&old_hashes).copied().collect()
    }

    /// Hitung statistik delta: berapa byte yang perlu di-download.
    pub fn delta_stats(old: &Self, new: &Self) -> DeltaStats {
        let missing = Self::diff(old, new);

        let total_new: u64 = new.chunks.iter().map(|c| c.size as u64).sum();
        let missing_bytes: u64 = new
            .chunks
            .iter()
            .filter(|c| missing.contains(&c.sha256))
            .map(|c| c.size as u64)
            .sum();

        let savings = if total_new > 0 {
            100.0 - (missing_bytes as f64 / total_new as f64 * 100.0)
        } else {
            0.0
        };

        DeltaStats {
            total_chunks: new.chunks.len() as u32,
            missing_chunks: missing.len() as u32,
            total_bytes: total_new,
            missing_bytes,
            savings_percent: savings,
        }
    }

    /// Verifikasi bahwa data yang direkonstruksi sesuai dengan index.
    pub fn verify(&self, data: &[u8]) -> Result<()> {
        use sha2::{Digest, Sha256};

        for entry in &self.chunks {
            let end = (entry.offset + entry.size as u64) as usize;
            if end > data.len() {
                return Err(Error::InvalidIndex(format!(
                    "Chunk at offset {} extends beyond data length {}",
                    entry.offset,
                    data.len()
                )));
            }

            let chunk_data = &data[entry.offset as usize..end];
            let mut hasher = Sha256::new();
            hasher.update(chunk_data);
            let result = hasher.finalize();
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&result);

            if hash != entry.sha256 {
                return Err(Error::HashMismatch {
                    offset: entry.offset,
                    expected: hex::encode(entry.sha256),
                    actual: hex::encode(hash),
                });
            }
        }

        Ok(())
    }
}

/// Statistik delta antara dua index.
#[derive(Debug, Clone)]
pub struct DeltaStats {
    /// Total chunk di new index
    pub total_chunks: u32,

    /// Chunk yang perlu di-download
    pub missing_chunks: u32,

    /// Total bytes new index
    pub total_bytes: u64,

    /// Bytes yang perlu di-download
    pub missing_bytes: u64,

    /// Penghematan bandwidth (%)
    pub savings_percent: f64,
}

/// Serde helper untuk hex-encoded SHA256.
mod hex_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(hash: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        let hex_str = hex::encode(hash);
        hex_str.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let hex_str: String = String::deserialize(d)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "Expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_index() -> CaiBxIndex {
        let data = b"Hello World! This is test data for CAIBX index testing. ".repeat(100);
        let chunker = Chunker::fixed(64);
        let chunks = chunker.chunk_bytes(&data);
        CaiBxIndex::from_chunks(&chunks, 64)
    }

    #[test]
    fn test_index_roundtrip() {
        let index = make_test_index();
        let bytes = index.to_bytes().unwrap();
        let loaded = CaiBxIndex::from_bytes(&bytes).unwrap();

        assert_eq!(index.total_size, loaded.total_size);
        assert_eq!(index.chunks.len(), loaded.chunks.len());
        assert_eq!(index.chunks[0].sha256, loaded.chunks[0].sha256);
    }

    #[test]
    fn test_verify_valid() {
        let data = b"Hello World! ".repeat(50);
        let chunker = Chunker::fixed(256);
        let chunks = chunker.chunk_bytes(&data);
        let index = CaiBxIndex::from_chunks(&chunks, 256);

        assert!(index.verify(&data).is_ok());
    }

    #[test]
    fn test_verify_invalid() {
        let data = b"Hello World! ".repeat(50);
        let chunker = Chunker::fixed(256);
        let chunks = chunker.chunk_bytes(&data);
        let index = CaiBxIndex::from_chunks(&chunks, 256);

        let mut corrupted = data.clone();
        corrupted[100] ^= 0xFF;

        assert!(index.verify(&corrupted).is_err());
    }

    #[test]
    fn test_delta_computation() {
        // Simulasi: data lama berubah sedikit
        let old_data = b"AAAA".repeat(1000); // 4KB of 'A'
        let new_data = {
            let mut d = old_data.clone();
            // Ubah 10% data di tengah
            for i in 1000..1400 {
                d[i] = b'B';
            }
            d
        };

        let chunker = Chunker::fixed(256);
        let old_chunks = chunker.chunk_bytes(&old_data);
        let new_chunks = chunker.chunk_bytes(&new_data);

        let old_idx = CaiBxIndex::from_chunks(&old_chunks, 256);
        let new_idx = CaiBxIndex::from_chunks(&new_chunks, 256);

        let stats = CaiBxIndex::delta_stats(&old_idx, &new_idx);

        // Harus ada penghematan karena banyak chunk yang sama
        assert!(stats.missing_chunks > 0); // Ada perubahan
        assert!(stats.missing_chunks < stats.total_chunks); // Tidak semua berubah
        assert!(stats.savings_percent > 0.0);
    }
}
