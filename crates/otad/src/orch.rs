//! Orchestrator — koordinasi alur OTA update.
//!
//! State machine:
//!
//!   IDLE ──► CHECKING ──► UPDATE_AVAILABLE ──► DOWNLOADING
//!     ▲                        │                    │
//!     │                        ▼                    ▼
//!     │                   NO_UPDATE             DOWNLOADED
//!     │                                              │
//!     │                                              ▼
//!     │                                         VERIFYING
//!     │                                              │
//!     │                                              ▼
//!     │                                         INSTALLING
//!     │                                              │
//!     │                                              ▼
//!     │                                     WAITING_REBOOT
//!     │                                              │
//!     │                                              ▼
//!     └────────────────────────────────────── REBOOTING

use crate::config::OtaConfig;
use crate::downloader::{self, Downloader};
use std::path::PathBuf;
use std::time::Duration;

use rauc_rs::RaucClient;
use update_verify::BundleVerifier;

/// State dari OTA process.
#[derive(Debug, Clone, PartialEq)]
pub enum OtaState {
    Idle,
    Checking,
    UpdateAvailable {
        version: String,
        size_bytes: u64,
        critical: bool,
    },
    NoUpdate,
    Downloading {
        version: String,
        progress_percent: u32,
    },
    Downloaded {
        version: String,
        path: PathBuf,
    },
    Verifying {
        version: String,
    },
    Installing {
        version: String,
    },
    WaitingReboot {
        version: String,
    },
    Rebooting,
    Error {
        message: String,
    },
}

pub struct Orchestrator {
    config: OtaConfig,
    verifier: BundleVerifier,
    downloader: Downloader,
    rauc: RaucClient,
    bus_socket: PathBuf,
    bus_client: Option<stardust::Client>,
    state: OtaState,
}

impl Orchestrator {
    pub fn new(
        config: OtaConfig,
        verifier: BundleVerifier,
        downloader: Downloader,
        rauc: RaucClient,
        bus_socket: PathBuf,
    ) -> Self {
        Self {
            config,
            verifier,
            downloader,
            rauc,
            bus_socket,
            bus_client: None,
            state: OtaState::Idle,
        }
    }

    pub fn set_bus_client(&mut self, client: stardust::Client) {
        self.bus_client = Some(client);
    }

    /// Main event loop — polling dengan interval.
    pub async fn run_polling_loop(&mut self) {
        let interval = Duration::from_secs(self.config.poll_interval_secs);

        // Cek update langsung saat startup
        if let Err(e) = self.check_and_update().await {
            tracing::error!("Initial update check failed: {e}");
        }

        loop {
            tokio::time::sleep(interval).await;
            tracing::debug!("Polling for updates...");

            if let Err(e) = self.check_and_update().await {
                tracing::error!("Update check failed: {e}");
                self.set_state(OtaState::Error {
                    message: e.to_string(),
                });
            }
        }
    }

    /// Check + download + install update (jika tersedia).
    pub async fn check_and_update(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.set_state(OtaState::Checking);

        // Dapatkan versi saat ini
        let current_version = self
            .rauc
            .current_version()?
            .unwrap_or_else(|| "0.0.0".to_string());

        // Check server
        let update_info = match self
            .downloader
            .check_update(&self.config.device, &self.config.channel, &current_version)
            .await?
        {
            Some(info) => info,
            None => {
                tracing::info!("No update available (current: {})", current_version);
                self.set_state(OtaState::NoUpdate);
                return Ok(());
            }
        };

        tracing::info!(
            "Update available: {} → {} ({} bytes)",
            current_version,
            update_info.version,
            update_info.size
        );

        self.set_state(OtaState::UpdateAvailable {
            version: update_info.version.clone(),
            size_bytes: update_info.size,
            critical: update_info.critical,
        });

        // Download
        self.set_state(OtaState::Downloading {
            version: update_info.version.clone(),
            progress_percent: 0,
        });

        let bundle_url = update_info.download_url.as_ref().ok_or("No download URL")?;

        let progress_cb: Option<downloader::ProgressFn> = Some(Box::new(|downloaded, total| {
            if total > 0 {
                let pct = ((downloaded as f64 / total as f64) * 100.0) as u32;
                tracing::debug!("Download progress: {}%", pct);
            }
        }));

        let bundle_path = self
            .downloader
            .download(
                bundle_url,
                &format!("update-{}.raucb", update_info.version),
                Some(&update_info.sha256),
                progress_cb,
            )
            .await?;

        self.set_state(OtaState::Downloaded {
            version: update_info.version.clone(),
            path: bundle_path.clone(),
        });

        // Verify
        self.set_state(OtaState::Verifying {
            version: update_info.version.clone(),
        });

        // Verify RAUC bundle signature
        self.rauc.verify_bundle(bundle_path.to_str().unwrap())?;

        tracing::info!("Bundle verified successfully");

        // Install
        if self.config.auto_install {
            self.install(&bundle_path)?;
        } else {
            tracing::info!(
                "Auto-install disabled. Bundle ready at: {}",
                bundle_path.display()
            );
            self.notify_ui(
                "update_ready",
                &serde_json::json!({
                    "version": update_info.version,
                    "path": bundle_path.to_string_lossy(),
                }),
            );
        }

        Ok(())
    }

    /// Install bundle ke slot inaktif.
    pub fn install(&mut self, bundle_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let version = match &self.state {
            OtaState::Downloaded { version, .. } => version.clone(),
            _ => "unknown".to_string(),
        };

        self.set_state(OtaState::Installing {
            version: version.clone(),
        });

        let options = rauc_rs::InstallOptions::default();
        self.rauc.install(bundle_path.to_str().unwrap(), &options)?;

        tracing::info!("Installation complete. Ready for reboot.");

        self.set_state(OtaState::WaitingReboot { version });

        if self.config.auto_reboot {
            self.reboot()?;
        } else {
            self.notify_ui(
                "reboot_needed",
                &serde_json::json!({
                    "message": "System update installed. Please reboot."
                }),
            );
        }

        Ok(())
    }

    /// Trigger system reboot.
    pub fn reboot(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.set_state(OtaState::Rebooting);

        tracing::info!("Rebooting system...");
        self.notify_ui(
            "rebooting",
            &serde_json::json!({
                "message": "System is rebooting to complete update..."
            }),
        );

        // Tunggu 2 detik agar notifikasi terkirim
        std::thread::sleep(Duration::from_secs(2));

        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{RebootMode, reboot};
            let _ = reboot(RebootMode::RB_AUTOBOOT);
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("Reboot not supported on this platform");
        }

        Ok(())
    }

    /// Konfirmasi bahwa boot ke slot baru berhasil.
    /// Dipanggil setelah reboot sukses.
    pub fn confirm_boot(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.rauc.mark_good()?;
        tracing::info!("Update confirmed — slot marked as good");
        Ok(())
    }

    /// Tandai boot gagal — trigger rollback.
    pub fn mark_boot_failed(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.rauc.mark_bad(None)?;
        tracing::info!("Update rejected — slot marked as bad");
        Ok(())
    }

    /// Update state dan notifikasi via stardust.
    fn set_state(&mut self, state: OtaState) {
        let old_state = std::mem::replace(&mut self.state, state.clone());

        // Notifikasi state change
        let payload = serde_json::json!({
            "state": format!("{:?}", state),
            "previous": format!("{:?}", old_state),
        });

        self.notify_ui("ota.state_changed", &payload);
    }

    /// Kirim notifikasi ke UI shell via stardust.
    fn notify_ui(&self, event: &str, payload: &serde_json::Value) {
        if let Some(ref client) = self.bus_client {
            let msg = match stardust::Message::new(format!("ota.{}", event))
                .src("otad".to_string())
                .param("payload", payload)
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("Failed to create notification message: {e}");
                    return;
                }
            };

            // Fire and forget — jangan block orchestrator
            let client = client.clone();
            tokio::spawn(async move {
                if let Err(e) = client.publish(msg).await {
                    tracing::warn!("Failed to notify UI: {e}");
                }
            });
        } else {
            tracing::debug!("No stardust client — UI notification skipped: {event}");
        }
    }
}
