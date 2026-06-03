//! Downloader — HTTP client untuk OTA update.
//!
//! Mendukung:
//!   - Resume download (Range requests)
//!   - Bandwidth throttling
//!   - SHA-256 verification setelah download
//!   - Progress tracking

use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SHA256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Download failed after {retries} retries: {reason}")]
    RetryExhausted { retries: u32, reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Progress callback: (downloaded_bytes, total_bytes)
pub type ProgressFn = Box<dyn Fn(u64, u64) + Send + Sync>;

pub struct Downloader {
    client: Client,
    server_url: String,
    download_dir: PathBuf,
}

impl Downloader {
    pub fn new(server_url: &str, download_dir: &str, tls_ca_bundle: &str) -> Self {
        let mut client_builder = Client::builder()
            .timeout(Duration::from_secs(3600)) // 1 jam timeout
            .connect_timeout(Duration::from_secs(30));

        // Load custom CA bundle for TLS verification
        if !tls_ca_bundle.is_empty() && std::path::Path::new(tls_ca_bundle).exists() {
            let ca_bytes = std::fs::read(tls_ca_bundle).unwrap_or_default();
            if !ca_bytes.is_empty() {
                let cert = reqwest::Certificate::from_pem(&ca_bytes).ok();
                if let Some(cert) = cert {
                    client_builder = client_builder.add_root_certificate(cert);
                    tracing::info!("TLS: loaded CA bundle from {tls_ca_bundle}");
                }
            }
        }

        let client = client_builder
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            server_url: server_url.to_string(),
            download_dir: PathBuf::from(download_dir),
        }
    }

    /// Cek apakah update tersedia.
    /// API: GET /api/v1/update/check?device={device}&channel={channel}&current={version}
    pub async fn check_update(
        &self,
        device: &str,
        channel: &str,
        current_version: &str,
    ) -> Result<Option<UpdateInfo>> {
        let url = format!(
            "{}/api/v1/update/check?device={}&channel={}&current={}",
            self.server_url, device, channel, current_version
        );

        let resp = self.client.get(&url).send().await?;

        if resp.status() == 204 {
            // No update available
            return Ok(None);
        }

        let info: UpdateInfo = resp.json().await?;
        Ok(Some(info))
    }

    /// Download file dengan progress tracking. Resume-supported.
    pub async fn download(
        &self,
        url: &str,
        filename: &str,
        expected_sha256: Option<&str>,
        progress: Option<ProgressFn>,
    ) -> Result<PathBuf> {
        let output_path = self.download_dir.join(filename);

        // Buat direktori
        std::fs::create_dir_all(&self.download_dir)?;

        // Cek apakah file sudah ada + ukurannya
        let existing_size = if output_path.exists() {
            std::fs::metadata(&output_path)?.len()
        } else {
            0
        };

        // Build request dengan Range header untuk resume
        let mut request = self.client.get(url);
        if existing_size > 0 {
            request = request.header("Range", format!("bytes={}-", existing_size));
        }

        let resp = request.send().await?;

        if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT || existing_size == 0 {
            let total_size = existing_size + resp.content_length().unwrap_or(0);

            let bytes = if existing_size > 0 {
                // Resume: append ke file existing
                let chunk = resp.bytes().await?;
                let mut file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&output_path)
                    .await?;
                file.write_all(&chunk).await?;
                chunk
            } else {
                // Fresh download
                let chunk = resp.bytes().await?;
                tokio::fs::write(&output_path, &chunk).await?;
                chunk
            };

            // Progress callback
            if let Some(ref cb) = progress {
                cb(bytes.len() as u64 + existing_size, total_size);
            }

            // Verify hash
            if let Some(expected) = expected_sha256 {
                let actual = self.sha256_file(&output_path)?;
                if actual != expected {
                    // Hapus file korup
                    let _ = tokio::fs::remove_file(&output_path).await;
                    return Err(Error::HashMismatch {
                        expected: expected.to_string(),
                        actual,
                    });
                }
            }

            tracing::info!(
                "Downloaded: {} ({} bytes)",
                filename,
                bytes.len() + existing_size as usize
            );
        }

        Ok(output_path)
    }

    /// Download casync index (.caibx) dari server.
    pub async fn download_index(&self, update_info: &UpdateInfo) -> Result<PathBuf> {
        let caibx_url = update_info.caibx_url.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No caibx_url in update info")
        })?;

        let filename = format!("update-{}.caibx", update_info.version);
        self.download(caibx_url, &filename, None, None).await
    }

    /// Hitung SHA-256 dari file lokal.
    fn sha256_file(&self, path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        let data = std::fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let result = hasher.finalize();
        Ok(hex::encode(&result[..]))
    }
}

/// Informasi update dari server.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateInfo {
    /// Versi baru
    pub version: String,

    /// Release channel
    pub channel: String,

    /// Timestamp build
    pub timestamp: u64,

    /// Apakah update critical
    #[serde(default)]
    pub critical: bool,

    /// Deskripsi / changelog
    #[serde(default)]
    pub description: String,

    /// URL untuk download bundle penuh
    #[serde(default)]
    pub download_url: Option<String>,

    /// URL untuk casync index (.caibx)
    #[serde(default)]
    pub caibx_url: Option<String>,

    /// SHA-256 dari bundle
    pub sha256: String,

    /// Ukuran bundle (bytes)
    pub size: u64,

    /// Ukuran delta jika menggunakan casync (bytes)
    #[serde(default)]
    pub delta_size: Option<u64>,

    /// Signature bundle (hex)
    pub signature: String,
}
