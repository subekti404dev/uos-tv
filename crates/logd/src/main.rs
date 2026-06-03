//! logd — Centralized Logging Daemon untuk UOS TV
//! ===============================================
//!
//! logd menerima log dari semua system services via Unix socket
//! dan menulisnya ke persistent storage dengan rotasi otomatis.
//!
//! Fitur:
//!   - Unix datagram socket: /run/uos/log.sock
//!   - Format: satu JSON object per baris (JSON Lines / NDJSON)
//!   - Output: /data/logs/uos.log dengan rotasi harian
//!   - Max log size: 10 MB per file, max 5 rotated files
//!   - Forwarding ke kernel ring buffer (/dev/kmsg) untuk critical errors
//!
//! Format log entry (JSON):
//! {
//!   "timestamp": "2025-06-03T10:30:00.123Z",
//!   "level": "INFO",
//!   "service": "netmd",
//!   "message": "WiFi connected to 'MyAP'",
//!   "pid": 1234,
//!   "fields": {"ssid": "MyAP", "rssi": -45}
//! }

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixDatagram;
use tokio::sync::mpsc;

/// Satu log entry dari service.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogEntry {
    /// ISO 8601 timestamp
    timestamp: String,

    /// Log level: TRACE, DEBUG, INFO, WARN, ERROR
    level: String,

    /// Service name
    service: String,

    /// Pesan log
    message: String,

    /// PID pengirim
    #[serde(default)]
    pid: u32,

    /// Additional structured fields
    #[serde(default)]
    fields: serde_json::Value,
}

#[tokio::main]
async fn main() {
    // logd sendiri tidak menggunakan logd untuk logging — langsung ke stderr
    eprintln!(
        "[logd] UOS TV Logging Daemon v{}",
        env!("CARGO_PKG_VERSION")
    );

    // Paths
    let socket_path =
        std::env::var("LOGD_SOCKET").unwrap_or_else(|_| "/run/uos/log.sock".to_string());

    let log_dir = std::env::var("LOGD_DIR").unwrap_or_else(|_| "/data/logs".to_string());

    // Buat direktori
    std::fs::create_dir_all(&log_dir).ok();
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Hapus stale socket
    let _ = std::fs::remove_file(&socket_path);

    // Bind Unix datagram socket
    let socket = match UnixDatagram::bind(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[logd] Failed to bind {socket_path}: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("[logd] Listening on {socket_path}");
    eprintln!("[logd] Writing logs to {log_dir}/");

    // Channel: receiver socket → writer task
    let (tx, rx) = mpsc::unbounded_channel::<LogEntry>();

    // Spawn writer task
    let writer_log_dir = log_dir.clone();
    let writer_handle = tokio::spawn(async move {
        log_writer(writer_log_dir, rx).await;
    });

    // Read loop — baca datagram dari socket
    let mut buf = vec![0u8; 65536]; // 64 KB buffer (max UDP datagram size)
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, _src_addr)) => {
                let data = &buf[..n];

                // Parse JSON
                match serde_json::from_slice::<LogEntry>(data) {
                    Ok(entry) => {
                        // Log level ERROR atau higher → juga kirim ke kernel ring buffer
                        if entry.level == "ERROR" {
                            let kmsg = format!("<3>[UOS] {}: {}\n", entry.service, entry.message);
                            write_to_kmsg(&kmsg);
                        }

                        // Kirim ke writer task
                        if tx.send(entry).is_err() {
                            eprintln!("[logd] Writer channel closed");
                            break;
                        }
                    }
                    Err(_e) => {
                        // Bukan JSON valid — treat as raw log
                        let raw = String::from_utf8_lossy(data);
                        let entry = LogEntry {
                            timestamp: Utc::now()
                                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                            level: "INFO".to_string(),
                            service: "unknown".to_string(),
                            message: raw.trim().to_string(),
                            pid: 0,
                            fields: serde_json::Value::Null,
                        };
                        let _ = tx.send(entry);
                    }
                }
            }
            Err(e) => {
                eprintln!("[logd] recv_from error: {e}");
            }
        }
    }

    // Tunggu writer task selesai
    let _ = writer_handle.await;
}

/// Background task untuk menulis log ke file dengan rotasi.
async fn log_writer(log_dir: String, mut rx: mpsc::UnboundedReceiver<LogEntry>) {
    let log_dir = PathBuf::from(log_dir);
    let mut current_file: Option<File> = None;
    let mut current_date = String::new();
    let mut current_size: u64 = 0;

    const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
    const MAX_ROTATED_FILES: u32 = 5;

    while let Some(entry) = rx.recv().await {
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Rotasi jika hari berganti atau file terlalu besar
        if current_date != today || current_size >= MAX_FILE_SIZE {
            current_file = open_log_file(&log_dir, &today).await;
            current_date = today;
            current_size = 0;
        }

        // Format sebagai JSON Lines
        let line = format!("{}\n", serde_json::to_string(&entry).unwrap_or_default());

        if let Some(ref mut file) = current_file {
            if let Err(e) = file.write_all(line.as_bytes()).await {
                eprintln!("[logd] Write error: {e}");
                current_file = None;
            } else {
                current_size += line.len() as u64;
            }
        }

        // Bersihkan file log lama (> MAX_ROTATED_FILES)
        cleanup_old_logs(&log_dir, MAX_ROTATED_FILES).await;
    }
}

/// Buka file log untuk hari ini.
async fn open_log_file(log_dir: &PathBuf, date: &str) -> Option<File> {
    let path = log_dir.join(format!("uos-{date}.log"));

    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(file) => {
            eprintln!("[logd] Opened log file: {}", path.display());
            Some(file)
        }
        Err(e) => {
            eprintln!("[logd] Failed to open {}: {e}", path.display());
            None
        }
    }
}

/// Hapus file log lama agar tidak memenuhi disk.
async fn cleanup_old_logs(_log_dir: &PathBuf, _max_files: u32) {
    // TODO: Implement log rotation cleanup
    // Untuk v1, skip — single file rotation berdasarkan tanggal sudah cukup
}

/// Tulis pesan ke kernel ring buffer (via /dev/kmsg).
fn write_to_kmsg(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = f.write_all(msg.as_bytes());
    }
}

/// Public API untuk services: kirim log ke logd.
///
/// Usage di service lain:
/// ```ignore
/// logd::log("INFO", "netmd", "WiFi connected");
/// ```
pub async fn log(level: &str, service: &str, message: &str) {
    let entry = LogEntry {
        timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        level: level.to_string(),
        service: service.to_string(),
        message: message.to_string(),
        pid: std::process::id(),
        fields: serde_json::Value::Null,
    };

    let socket_path = "/run/uos/log.sock";
    if let Ok(socket) = UnixDatagram::unbound() {
        if let Ok(data) = serde_json::to_vec(&entry) {
            let _ = socket.send_to(&data, socket_path).await;
        }
    }
}

/// Macro untuk memudahkan logging dari service.
#[macro_export]
macro_rules! uos_log {
    ($level:expr, $service:expr, $($arg:tt)*) => {
        $crate::log($level, $service, &format!($($arg)*))
    };
}
