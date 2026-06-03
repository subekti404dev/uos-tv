//! ChunkStore — penyimpanan chunk berbasis content-addressable.
//!
//! Layout di disk:
//!   {store_dir}/
//!     chunks/
//!       ab/
//!         abcdef0123456789...  (full 64-char hex hash)
//!
//! Setiap chunk disimpan sebagai file dengan nama = hex(hash).
//! Direktori 2-char pertama digunakan untuk menghindari
//! terlalu banyak file dalam satu direktori.

use crate::chunker::Chunk;
use crate::index::CaiBxIndex;
use crate::{ChunkHash, Result};
use std::io;
use std::path::{Path, PathBuf};

/// Penyimpanan chunk berbasis filesystem.
pub struct ChunkStore {
    /// Root directory chunk store
    root: PathBuf,
}

impl ChunkStore {
    /// Buat chunk store baru.
    /// Direktori akan dibuat jika belum ada.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let chunks_dir = Self::chunks_dir(&root);
        std::fs::create_dir_all(&chunks_dir)?;

        Ok(Self { root })
    }

    /// Get direktori chunks/
    fn chunks_dir(root: &Path) -> PathBuf {
        root.join("chunks")
    }

    /// Get path untuk chunk dengan hash tertentu.
    /// Format: {root}/chunks/{first2}/{full_hash}
    fn chunk_path(&self, hash: &ChunkHash) -> PathBuf {
        let hex = hex::encode(hash);
        let dir = &hex[..2];
        Self::chunks_dir(&self.root).join(dir).join(&hex)
    }

    /// Simpan chunk ke store.
    /// Return true jika chunk sudah ada (dedup), false jika baru disimpan.
    pub fn store_chunk(&self, chunk: &Chunk) -> io::Result<bool> {
        let path = self.chunk_path(&chunk.hash);

        // Jika sudah ada, skip
        if path.exists() {
            return Ok(true);
        }

        // Buat direktori subdir
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Tulis chunk data
        std::fs::write(&path, &chunk.data)?;
        Ok(false)
    }

    /// Simpan semua chunk dari index + rekonstruksi file.
    /// Chunk disimpan ke store, lalu file output di-rakit.
    pub fn store_from_index(
        &self,
        index: &CaiBxIndex,
        chunks: &[(ChunkHash, Vec<u8>)],
    ) -> io::Result<Vec<u8>> {
        // Simpan semua chunk ke store
        for (hash, data) in chunks {
            let path = self.chunk_path(hash);
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, data)?;
            }
        }

        // Rekonstruksi file dari chunk di store
        self.reconstruct(index)
    }

    /// Rekonstruksi file dari chunk yang sudah ada di store.
    pub fn reconstruct(&self, index: &CaiBxIndex) -> io::Result<Vec<u8>> {
        let mut output = Vec::with_capacity(index.total_size as usize);

        for entry in &index.chunks {
            let path = self.chunk_path(&entry.sha256);

            if !path.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Missing chunk: {} at offset {}",
                        hex::encode(entry.sha256),
                        entry.offset
                    ),
                ));
            }

            let data = std::fs::read(&path)?;
            output.extend_from_slice(&data);
        }

        Ok(output)
    }

    /// Cek apakah chunk dengan hash tertentu ada di store.
    pub fn has_chunk(&self, hash: &ChunkHash) -> bool {
        self.chunk_path(hash).exists()
    }

    /// Cek chunk mana yang sudah ada di store vs perlu download.
    /// Return: (present_hashes, missing_hashes)
    pub fn check_chunks(&self, index: &CaiBxIndex) -> (Vec<ChunkHash>, Vec<ChunkHash>) {
        let mut present = Vec::new();
        let mut missing = Vec::new();

        for entry in &index.chunks {
            if self.has_chunk(&entry.sha256) {
                present.push(entry.sha256);
            } else {
                missing.push(entry.sha256);
            }
        }

        (present, missing)
    }

    /// Bersihkan chunk yang tidak digunakan (orphaned chunks).
    /// Ini akan menghapus chunk yang tidak direferensikan oleh index.
    pub fn cleanup(&self, referenced_hashes: &[ChunkHash]) -> io::Result<u32> {
        use std::collections::HashSet;
        let refs: HashSet<_> = referenced_hashes.iter().collect();
        let mut removed = 0u32;

        let chunks_dir = Self::chunks_dir(&self.root);
        if !chunks_dir.exists() {
            return Ok(0);
        }

        // Iterasi direktori 2-char
        for entry in std::fs::read_dir(&chunks_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            for file in std::fs::read_dir(entry.path())? {
                let file = file?;
                let name = file.file_name();
                let name_str = name.to_string_lossy();

                // Decode hex → hash
                if let Ok(bytes) = hex::decode(name_str.as_ref()) {
                    if bytes.len() == 32 {
                        let mut hash = [0u8; 32];
                        hash.copy_from_slice(&bytes);

                        if !refs.contains(&hash) {
                            std::fs::remove_file(file.path())?;
                            removed += 1;
                        }
                    }
                }
            }
        }

        Ok(removed)
    }

    /// Dapatkan data chunk dari store.
    pub fn get_chunk_data(&self, hash: &ChunkHash) -> io::Result<Vec<u8>> {
        let path = self.chunk_path(hash);
        std::fs::read(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, Chunker};
    use crate::index::CaiBxIndex;

    #[test]
    fn test_store_and_reconstruct() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ChunkStore::new(tmp.path()).unwrap();

        let data = b"Hello World! ".repeat(500);
        let chunker = Chunker::fixed(128);
        let chunks = chunker.chunk_bytes(&data);
        let index = CaiBxIndex::from_chunks(&chunks, 128);

        // Simpan semua chunk
        for chunk in &chunks {
            store.store_chunk(chunk).unwrap();
        }

        // Rekonstruksi
        let reconstructed = store.reconstruct(&index).unwrap();
        assert_eq!(reconstructed, &data[..]);
    }

    #[test]
    fn test_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ChunkStore::new(tmp.path()).unwrap();

        let chunk = Chunk::new(0, b"test data".to_vec());

        // Pertama: harus false (baru)
        assert!(!store.store_chunk(&chunk).unwrap());

        // Kedua: harus true (sudah ada)
        assert!(store.store_chunk(&chunk).unwrap());
    }
}
