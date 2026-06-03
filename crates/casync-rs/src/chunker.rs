//! Chunker — memecah data stream menjadi chunk.
//!
//! Dua strategi:
//!   - **Fixed-size**: chunk berukuran tepat `chunk_size` bytes.
//!     Sederhana dan cepat. Digunakan saat content-defined tidak diperlukan.
//!   - **Content-defined (Gear Hash)**: chunk boundary ditentukan oleh
//!     konten, sehingga chunk yang sama akan tetap sama meski data
//!     disisipi/dihapus di tengah file. Lebih baik untuk delta updates.

use crate::{ChunkHash, DEFAULT_CHUNK_SIZE, GEAR_TABLE, MAX_CHUNK_SIZE, MIN_CHUNK_SIZE};
use sha2::{Digest, Sha256};

/// Satu chunk hasil chunking.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Offset dalam source stream
    pub offset: u64,

    /// Ukuran chunk (bytes)
    pub size: u32,

    /// Data chunk (owned)
    pub data: Vec<u8>,

    /// SHA-256 hash (dihitung saat chunk dibuat)
    pub hash: ChunkHash,
}

impl Chunk {
    /// Buat chunk baru + hitung hash.
    pub fn new(offset: u64, data: Vec<u8>) -> Self {
        let size = data.len() as u32;
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            let result = hasher.finalize();
            let mut h = [0u8; 32];
            h.copy_from_slice(&result);
            h
        };
        Self {
            offset,
            size,
            data,
            hash,
        }
    }

    /// Hex string dari hash.
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash)
    }
}

/// Strategi chunking.
#[derive(Debug, Clone, Copy)]
pub enum ChunkingStrategy {
    /// Fixed-size chunking
    Fixed { size: usize },

    /// Content-defined chunking via gear hash
    GearHash {
        min_size: usize,
        max_size: usize,
        mask: u64,
    },
}

impl Default for ChunkingStrategy {
    fn default() -> Self {
        ChunkingStrategy::GearHash {
            min_size: MIN_CHUNK_SIZE,
            max_size: MAX_CHUNK_SIZE,
            mask: (1u64 << 16) - 1, // 16-bit mask → ~64KB average
        }
    }
}

/// Chunker: memecah data menjadi chunk.
pub struct Chunker {
    strategy: ChunkingStrategy,
}

impl Chunker {
    /// Buat chunker baru dengan fixed-size strategy.
    pub fn fixed(chunk_size: usize) -> Self {
        Self {
            strategy: ChunkingStrategy::Fixed { size: chunk_size },
        }
    }

    /// Buat chunker baru dengan content-defined strategy (default).
    pub fn new() -> Self {
        Self {
            strategy: ChunkingStrategy::default(),
        }
    }

    /// Buat chunker dengan custom gear hash parameters.
    pub fn gear_hash(min_size: usize, max_size: usize, mask_bits: u32) -> Self {
        Self {
            strategy: ChunkingStrategy::GearHash {
                min_size,
                max_size,
                mask: (1u64 << mask_bits) - 1,
            },
        }
    }

    /// Chunk data dari bytes slice.
    /// Return iterator-style Vec<Chunk>.
    pub fn chunk_bytes(&self, data: &[u8]) -> Vec<Chunk> {
        match self.strategy {
            ChunkingStrategy::Fixed { size } => chunk_fixed(data, size),
            ChunkingStrategy::GearHash {
                min_size,
                max_size,
                mask,
            } => chunk_gear(data, min_size, max_size, mask),
        }
    }

    /// Chunk data dari reader (synchronous).
    pub fn chunk_reader<R: std::io::Read>(&self, mut reader: R) -> std::io::Result<Vec<Chunk>> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(self.chunk_bytes(&buf))
    }
}

/// Fixed-size chunking.
fn chunk_fixed(data: &[u8], chunk_size: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut offset = 0u64;

    for slice in data.chunks(chunk_size) {
        chunks.push(Chunk::new(offset, slice.to_vec()));
        offset += slice.len() as u64;
    }

    chunks
}

/// Content-defined chunking via gear hash.
///
/// Gear hash adalah rolling hash berbasis lookup table.
/// Untuk setiap byte, kita update hash = (hash << 1) + GEAR_TABLE[byte].
/// Ketika (hash & mask) == 0, kita tandai chunk boundary.
///
/// Boundary hanya berlaku jika ukuran chunk > min_size.
/// Jika ukuran chunk mencapai max_size, paksa boundary.
fn chunk_gear(data: &[u8], min_size: usize, max_size: usize, mask: u64) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut chunk_start = 0usize;
    let mut hash: u64 = 0;
    let mut pos = 0usize;

    while pos < data.len() {
        // Update gear hash
        hash = (hash << 1).wrapping_add(GEAR_TABLE[data[pos] as usize]);

        let chunk_len = pos + 1 - chunk_start;

        // Check boundary condition
        let is_boundary = (hash & mask) == 0 && chunk_len >= min_size;
        let forced = chunk_len >= max_size;

        if is_boundary || forced {
            let chunk_data = data[chunk_start..pos + 1].to_vec();
            let offset = chunk_start as u64;
            chunks.push(Chunk::new(offset, chunk_data));

            chunk_start = pos + 1;
            hash = 0;
        }

        pos += 1;
    }

    // Sisa data terakhir
    if chunk_start < data.len() {
        let chunk_data = data[chunk_start..].to_vec();
        let offset = chunk_start as u64;
        chunks.push(Chunk::new(offset, chunk_data));
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_chunking() {
        let data = b"Hello, World! This is test data for chunking.";
        let chunker = Chunker::fixed(8);
        let chunks = chunker.chunk_bytes(data);

        // Harus ada beberapa chunk
        assert!(chunks.len() > 1);

        // Total size harus sama
        let total: usize = chunks.iter().map(|c| c.size as usize).sum();
        assert_eq!(total, data.len());

        // Reassemble
        let mut reassembled = Vec::new();
        for chunk in &chunks {
            reassembled.extend_from_slice(&chunk.data);
        }
        assert_eq!(reassembled, data);
    }

    #[test]
    fn test_gear_chunking() {
        // Data random — gear hash harus menghasilkan chunk boundaries
        let data = vec![0u8; 200_000]; // 200 KB
        let chunker = Chunker::new();
        let chunks = chunker.chunk_bytes(&data);

        // Harus ada beberapa chunk
        assert!(chunks.len() > 1);

        // Total harus match
        let total: usize = chunks.iter().map(|c| c.size as usize).sum();
        assert_eq!(total, data.len());
    }

    #[test]
    fn test_chunk_hash_consistent() {
        let data = b"hello world";
        let chunk1 = Chunk::new(0, data.to_vec());
        let chunk2 = Chunk::new(0, data.to_vec());

        assert_eq!(chunk1.hash, chunk2.hash);
        assert_eq!(chunk1.hash_hex(), chunk2.hash_hex());
    }
}
