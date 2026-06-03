//! casync-rs — Content-Addressable Synchronization in pure Rust
//! ============================================================
//!
//! CASync memungkinkan delta-based file synchronization dengan cara:
//!
//! 1. **Chunking** — file dipecah menjadi chunk berukuran ~64KB (rata-rata)
//!    menggunakan content-defined chunking (gear hash) atau fixed-size.
//! 2. **Hashing** — setiap chunk di-hash dengan SHA-256.
//! 3. **Indexing** — membuat file index (.caibx) yang memetakan
//!    offset file → hash chunk.
//! 4. **Delta** — dengan membandingkan dua index (lama vs baru),
//!    kita tahu chunk mana yang sudah dimiliki client.
//!
//! Format index (.caibx):
//!   - Magic: b"CAIBX\n"
//!   - Version: u32 (1)
//!   - Chunk size target: u32 (default 65536)
//!   - Chunk list: [(offset: u64, size: u32, sha256: [u8; 32]), ...]
//!
//! Semua diserialize dengan CBOR untuk compactness.

pub mod chunker;
pub mod index;
pub mod store;

pub use chunker::Chunker;
pub use index::{CaiBxIndex, ChunkEntry};
pub use store::ChunkStore;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Hash mismatch at chunk {offset}: expected {expected}, got {actual}")]
    HashMismatch {
        offset: u64,
        expected: String,
        actual: String,
    },

    #[error("Invalid index file: {0}")]
    InvalidIndex(String),

    #[error("CBOR error: {0}")]
    Cbor(#[from] ciborium::de::Error<std::io::Error>),

    #[error("CBOR serialization error: {0}")]
    CborSer(#[from] ciborium::ser::Error<std::io::Error>),
}

pub type Result<T> = std::result::Result<T, Error>;

/// SHA-256 hash (32 bytes)
pub type ChunkHash = [u8; 32];

/// Default target chunk size: 64 KB
pub const DEFAULT_CHUNK_SIZE: usize = 65536;

/// Minimum chunk size (content-defined): 32 KB
pub const MIN_CHUNK_SIZE: usize = 32768;

/// Maximum chunk size (content-defined): 128 KB
pub const MAX_CHUNK_SIZE: usize = 131072;

/// Gear hash table untuk content-defined chunking.
/// Ini adalah tabel pseudo-random 256-entry yang digunakan
/// oleh gear hash algorithm untuk menentukan chunk boundary.
const GEAR_TABLE: [u64; 256] = generate_gear_table();

const fn generate_gear_table() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        // Simple pseudo-random: multiply by a large prime
        table[i] = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        i += 1;
    }
    table
}
