//! Service manifest parser.
//!
//! Format YAML:
//! ```yaml
//! name: lumind
//! description: "Wayland compositor"
//! binary: /usr/bin/lumind
//! args:
//!   - "--tty"
//!   - "7"
//! dependencies:
//!   - dispald
//!   - inputd
//! restart: always          # always | on-failure | never
//! restart_delay_ms: 1000   # delay sebelum restart
//! max_crash_count: 3       # max crash dalam window
//! crash_window_secs: 30    # crash count window
//! critical: true           # jika true, crash = system panic
//! ```

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("IO error reading manifest: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Invalid manifest for '{name}': {reason}")]
    Invalid { name: String, reason: String },

    #[error("Duplicate service name: {0}")]
    Duplicate(String),
}

/// Security capabilities for a service.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SecurityCapabilities {
    /// Capabilities to allow (keep). Omission = drop.
    /// Default: empty = drop all.
    #[serde(default)]
    pub keep: Vec<String>,

    /// Bind mount paths (src:dst pairs).
    #[serde(default)]
    pub mounts: Vec<String>,

    /// Read-only rootfs for this service.
    #[serde(default)]
    pub read_only_root: bool,
}

impl Default for SecurityCapabilities {
    fn default() -> Self {
        Self {
            keep: vec![],
            mounts: vec![],
            read_only_root: false,
        }
    }
}

/// Restart policy untuk service.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Selalu restart — gunakan untuk service critical
    Always,
    /// Restart hanya jika exit code != 0
    OnFailure,
    /// Tidak pernah restart
    Never,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::OnFailure
    }
}

/// Satu entry service manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceManifest {
    /// Nama unik service (contoh: "lumind", "otad")
    pub name: String,

    /// Deskripsi human-readable
    #[serde(default)]
    pub description: String,

    /// Path ke binary executable
    pub binary: String,

    /// Command-line arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables (KEY=VALUE)
    #[serde(default)]
    pub env: Vec<String>,

    /// Nama service yang harus running SEBELUM service ini
    #[serde(default)]
    pub dependencies: Vec<String>,

    /// Nama service yang harus running SETELAH service ini (optional)
    #[serde(default)]
    pub after: Vec<String>,

    /// Restart policy
    #[serde(default)]
    pub restart: RestartPolicy,

    /// Delay sebelum restart (ms)
    #[serde(default = "default_restart_delay")]
    pub restart_delay_ms: u64,

    /// Maximum crash count dalam window
    #[serde(default = "default_max_crash")]
    pub max_crash_count: u32,

    /// Crash count reset window (detik)
    #[serde(default = "default_crash_window")]
    pub crash_window_secs: u64,

    /// Jika true + crash = system panic (untuk service critical)
    #[serde(default)]
    pub critical: bool,

    /// Timeout startup (detik) — jika service tidak ready dalam waktu ini, dianggap gagal
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_secs: u64,

    /// Health check method (via stardust)
    #[serde(default)]
    pub health_check: Option<String>,

    /// Security capabilities (capabilities, mounts, seccomp)
    #[serde(default)]
    pub caps: SecurityCapabilities,
}

fn default_restart_delay() -> u64 {
    1000
}
fn default_max_crash() -> u32 {
    3
}
fn default_crash_window() -> u64 {
    30
}
fn default_startup_timeout() -> u64 {
    15
}

/// Load semua service manifests dari direktori.
pub fn load_from_dir(dir: &Path) -> Result<Vec<ServiceManifest>, ManifestError> {
    let mut services = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if !dir.exists() {
        return Err(ManifestError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Directory not found: {}", dir.display()),
        )));
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        // Hanya file .yaml / .yml
        match path.extension().and_then(|e| e.to_str()) {
            Some("yaml") | Some("yml") => {}
            _ => continue,
        }

        let content = std::fs::read_to_string(&path)?;
        let manifest: ServiceManifest = serde_yaml::from_str(&content)?;

        // Validasi
        if manifest.name.is_empty() {
            return Err(ManifestError::Invalid {
                name: path.display().to_string(),
                reason: "name is required".into(),
            });
        }

        if manifest.binary.is_empty() {
            return Err(ManifestError::Invalid {
                name: manifest.name,
                reason: "binary path is required".into(),
            });
        }

        // Cek duplikasi
        if !seen.insert(manifest.name.clone()) {
            return Err(ManifestError::Duplicate(manifest.name));
        }

        tracing::debug!(
            "Loaded manifest: {} (deps: {:?})",
            manifest.name,
            manifest.dependencies
        );

        services.push(manifest);
    }

    Ok(services)
}

/// Default service manifests untuk development/testing.
pub fn default_services() -> Vec<ServiceManifest> {
    vec![
        svc_logd(),
        svc_devmand(),
        svc_dispald(),
        svc_inputd(),
        svc_lumind(),
        svc_audiod(),
        svc_netmd(),
        svc_notifd(),
        svc_powermand(),
        svc_pkgd(),
        svc_otad(),
    ]
}

fn svc_base(name: &str, bin: &str, desc: &str) -> ServiceManifest {
    ServiceManifest {
        name: name.into(),
        description: desc.into(),
        binary: bin.into(),
        args: vec![],
        env: vec![],
        dependencies: vec![],
        after: vec![],
        restart: RestartPolicy::Always,
        restart_delay_ms: 1000,
        max_crash_count: 3,
        crash_window_secs: 30,
        critical: false,
        startup_timeout_secs: 10,
        health_check: None,
        caps: SecurityCapabilities::default(),
    }
}

fn svc_logd() -> ServiceManifest {
    ServiceManifest {
        startup_timeout_secs: 5,
        health_check: Some("logd.ping".into()),
        restart_delay_ms: 500,
        max_crash_count: 5,
        ..svc_base("logd", "/usr/bin/logd", "Centralized logging daemon")
    }
}

fn svc_devmand() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["logd".into()],
        health_check: Some("devmand.ping".into()),
        ..svc_base("devmand", "/usr/bin/devmand", "Device manager")
    }
}

fn svc_dispald() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["devmand".into()],
        critical: true,
        health_check: Some("dispald.ping".into()),
        ..svc_base("dispald", "/usr/bin/dispald", "Display manager")
    }
}

fn svc_inputd() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["devmand".into()],
        restart_delay_ms: 500,
        startup_timeout_secs: 5,
        health_check: Some("inputd.ping".into()),
        ..svc_base("inputd", "/usr/bin/inputd", "Input manager")
    }
}

fn svc_lumind() -> ServiceManifest {
    ServiceManifest {
        args: vec!["--tty".into(), "7".into()],
        env: vec!["WAYLAND_DISPLAY=wayland-0".into()],
        dependencies: vec!["dispald".into(), "inputd".into()],
        restart_delay_ms: 2000,
        critical: true,
        startup_timeout_secs: 15,
        health_check: Some("lumind.ping".into()),
        ..svc_base("lumind", "/usr/bin/lumind", "Wayland compositor")
    }
}

fn svc_audiod() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["devmand".into()],
        after: vec!["lumind".into()],
        health_check: Some("audiod.ping".into()),
        ..svc_base("audiod", "/usr/bin/audiod", "Audio manager")
    }
}

fn svc_netmd() -> ServiceManifest {
    ServiceManifest {
        after: vec!["lumind".into()],
        health_check: Some("netmd.ping".into()),
        ..svc_base("netmd", "/usr/bin/netmd", "Network manager")
    }
}

fn svc_notifd() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["logd".into()],
        after: vec!["lumind".into()],
        restart_delay_ms: 500,
        startup_timeout_secs: 5,
        health_check: Some("notifd.ping".into()),
        ..svc_base("notifd", "/usr/bin/notifd", "Notification bus")
    }
}

fn svc_powermand() -> ServiceManifest {
    ServiceManifest {
        after: vec!["lumind".into()],
        startup_timeout_secs: 5,
        health_check: Some("powermand.ping".into()),
        ..svc_base("powermand", "/usr/bin/powermand", "Power manager")
    }
}

fn svc_pkgd() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["netmd".into()],
        restart: RestartPolicy::OnFailure,
        restart_delay_ms: 2000,
        crash_window_secs: 60,
        ..svc_base("pkgd", "/usr/bin/pkgd", "Package manager")
    }
}

fn svc_otad() -> ServiceManifest {
    ServiceManifest {
        dependencies: vec!["netmd".into()],
        restart_delay_ms: 5000,
        crash_window_secs: 60,
        startup_timeout_secs: 15,
        health_check: Some("otad.ping".into()),
        ..svc_base("otad", "/usr/bin/otad", "OTA update manager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_services_valid() {
        let services = default_services();
        assert_eq!(services.len(), 11);

        // Verifikasi tidak ada duplikasi
        let names: Vec<_> = services.iter().map(|s| &s.name).collect();
        let mut unique = std::collections::HashSet::new();
        for name in &names {
            assert!(unique.insert(name), "Duplicate: {name}");
        }

        // Verifikasi semua dependency merujuk ke service yang ada
        let name_set: std::collections::HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
        for svc in &services {
            for dep in &svc.dependencies {
                assert!(
                    name_set.contains(dep.as_str()),
                    "Unknown dependency '{}' in service '{}'",
                    dep,
                    svc.name
                );
            }
        }
    }
}
