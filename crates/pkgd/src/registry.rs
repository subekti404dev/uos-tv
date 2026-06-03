//! App registry — manage installed apps

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub app_type: String,
    pub url: Option<String>,
    pub icon: Option<String>,
    pub permissions: Vec<String>,
}

pub struct AppRegistry {
    apps_dir: PathBuf,
    apps: HashMap<String, AppEntry>,
    registry_file: PathBuf,
}

impl AppRegistry {
    pub fn new(apps_dir: PathBuf) -> Self {
        let registry_file = apps_dir.join("registry.json");
        let apps = if registry_file.exists() {
            serde_json::from_slice(&std::fs::read(&registry_file).unwrap_or_default())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Auto-discover pre-installed apps
        let mut reg = Self {
            apps_dir,
            apps,
            registry_file,
        };
        reg.discover_system_apps();
        reg
    }

    /// Discover built-in system apps
    fn discover_system_apps(&mut self) {
        let system_apps = vec![
            AppEntry {
                id: "com.uos.luna".into(),
                name: "Home".into(),
                version: "1.0.0".into(),
                app_type: "webapp".into(),
                url: Some("http://localhost:8080".into()),
                icon: Some("luna.png".into()),
                permissions: vec!["system".into()],
            },
            AppEntry {
                id: "com.uos.settings".into(),
                name: "Settings".into(),
                version: "1.0.0".into(),
                app_type: "webapp".into(),
                url: Some("http://localhost:8080/settings".into()),
                icon: Some("settings.png".into()),
                permissions: vec!["system".into(), "network".into()],
            },
        ];

        for app in system_apps {
            self.apps.entry(app.id.clone()).or_insert(app);
        }
        self.save();
    }

    pub fn list(&self) -> Vec<&AppEntry> {
        self.apps.values().collect()
    }

    pub async fn install(&self, id: &str, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!("Installing: {id} from {url}");

        let app_dir = self.apps_dir.join(id);
        std::fs::create_dir_all(&app_dir)?;

        // Download package (placeholder — gunakan downloader otad)
        let response = reqwest::get(url).await?.bytes().await?;

        // Verify hash (dummy)
        let mut hasher = Sha256::new();
        hasher.update(&response);
        let _hash = hex::encode(hasher.finalize());

        // Extract to app dir
        std::fs::write(app_dir.join("bundle.uosp"), &response)?;
        tracing::info!("Package downloaded: {} bytes", response.len());

        // Write manifest placeholder
        let manifest = serde_json::json!({
            "id": id,
            "name": id,
            "version": "1.0.0",
            "type": "webapp",
            "url": url,
            "permissions": []
        });
        std::fs::write(
            app_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;

        Ok(())
    }

    pub fn uninstall(&self, id: &str) {
        let app_dir = self.apps_dir.join(id);
        if app_dir.exists() {
            let _ = std::fs::remove_dir_all(&app_dir);
            tracing::info!("Uninstalled: {id}");
        }
    }

    fn save(&self) {
        if let Some(parent) = self.registry_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_vec_pretty(&self.apps) {
            let _ = std::fs::write(&self.registry_file, data);
        }
    }
}
