//! otad — OTA Update Daemon untuk UOS TV
//! =====================================
//!
//! otad bertanggung jawab atas seluruh lifecycle OTA update:
//!
//!   POLLING    → Cek update server tiap N jam
//!   DOWNLOAD   → Unduh bundle / delta chunks
//!   VERIFY     → Verifikasi signature + hash
//!   INSTALL    → Install via RAUC ke slot inaktif
//!   REBOOT     → Trigger reboot ke slot baru
//!   CONFIRM    → Mark-good setelah boot berhasil
//!
//! Arsitektur:
//!
//!   otad ──► Update Server (HTTPS)    ← polling
//!   otad ──► casync-rs               ← delta chunking
//!   otad ──► update-verify            ← signature check
//!   otad ──► rauc-rs                  ← install bundle
//!   otad ──► stardust                 ← notifikasi UI

mod config;
mod downloader;
mod orch;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "otad", about = "UOS TV OTA Update Daemon")]
struct Args {
    /// Konfigurasi file (YAML)
    #[arg(short, long, default_value = "/etc/uos/ota.yaml")]
    config: PathBuf,

    /// Path ke stardust socket
    #[arg(short, long, default_value = "/run/uos/bus.sock")]
    bus_socket: PathBuf,

    /// Public key untuk verifikasi (hex)
    #[arg(short, long)]
    public_key: Option<String>,

    /// Check sekali lalu exit (untuk testing)
    #[arg(long)]
    check_once: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let args = Args::parse();

    tracing::info!("otad v{} starting...", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = config::OtaConfig::load(&args.config).unwrap_or_else(|e| {
        tracing::warn!("Failed to load config: {e}, using defaults");
        config::OtaConfig::default()
    });

    tracing::info!("Update server: {}", config.server_url);
    tracing::info!("Polling interval: {}s", config.poll_interval_secs);
    tracing::info!("Channel: {}", config.channel);
    tracing::info!("Device: {}", config.device);

    // Init verifier
    let verifier = match &args.public_key {
        Some(key_hex) => {
            let key_bytes = hex::decode(key_hex).expect("Invalid public key hex");
            let mut key_arr = [0u8; 32];
            key_arr.copy_from_slice(&key_bytes);
            let mut v = update_verify::BundleVerifier::new(&key_arr).expect("Invalid public key");
            v.set_allowed_devices(vec![config.device.clone()]);
            v
        }
        None => {
            tracing::warn!("No public key provided — signature verification DISABLED (dev mode)");
            // Dev mode: terima semua bundle tanpa verifikasi
            // Di production, hardcode public key di kernel
            update_verify::BundleVerifier::new(&[0u8; 32]).unwrap()
        }
    };

    // Init downloader
    let downloader = downloader::Downloader::new(
        &config.server_url,
        &config.download_dir,
        &config.tls_ca_bundle,
    );

    // Init RAUC client
    let rauc = rauc_rs::RaucClient::new();

    // Init orchestrator
    let bus_socket = args.bus_socket.clone();
    let mut orch = orch::Orchestrator::new(config, verifier, downloader, rauc, bus_socket);

    // Connect ke stardust
    match stardust::Client::connect(&args.bus_socket).await {
        Ok(client) => {
            client.register("otad").await.ok();
            orch.set_bus_client(client);
        }
        Err(e) => {
            tracing::warn!("Failed to connect to stardust: {e}");
        }
    }

    if args.check_once {
        // Check update sekali lalu exit
        if let Err(e) = orch.check_and_update().await {
            tracing::error!("Update check failed: {e}");
            std::process::exit(1);
        }
    } else {
        // Loop polling
        orch.run_polling_loop().await;
    }
}
