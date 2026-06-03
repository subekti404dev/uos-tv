//! pkgd — UOS TV Package Manager
//! ================================
//! pkgd mengelola instalasi, update, dan penghapusan aplikasi UOS TV.
//!
//! Format paket: .uosp (tar.gz + manifest.json)
//!   manifest.json:
//!   {
//!     "id": "com.uos.youtube",
//!     "name": "YouTube",
//!     "version": "1.2.0",
//!     "type": "webapp",       // webapp | native
//!     "url": "https://...",   // URL untuk webapp
//!     "icon": "icon.png",
//!     "permissions": ["network", "storage"]
//!   }
//!
//! Aplikasi disimpan di: /data/apps/{id}/
//! Registry: /data/apps/registry.json
//!
//! API via stardust:
//!   pkg.install { id, url }
//!   pkg.uninstall { id }
//!   pkg.update { id }
//!   pkg.list → response

mod installer;
mod registry;

use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());
    let apps_dir = std::env::var("UOS_APPS_DIR").unwrap_or_else(|_| "/data/apps".to_string());

    tracing::info!("pkgd starting (apps: {apps_dir})...");

    let registry = Arc::new(registry::AppRegistry::new(PathBuf::from(&apps_dir)));

    // Connect ke stardust
    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("pkgd").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust bus: {e}");
            None
        }
    };

    // Subscribe commands
    if let Some(ref c) = client {
        if let Ok(mut rx) = c.subscribe("pkg.command.*").await {
            let client = c.clone();
            let registry = registry.clone();
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    handle_command(&client, &registry, &msg).await;
                }
            });
        }
    }

    // Publish initial app list
    if let Some(ref c) = client {
        publish_app_list(c, &registry);
    }

    tracing::info!("pkgd ready");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    }
}

async fn handle_command(
    client: &stardust::Client,
    registry: &registry::AppRegistry,
    msg: &stardust::Message,
) {
    let method = msg.method.as_str();

    let params: serde_json::Value = match serde_json::from_slice(&msg.params) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Invalid params for {method}: {e}");
            return;
        }
    };

    match method {
        "pkg.command.install" => {
            let id = params["id"].as_str().unwrap_or("");
            let url = params["url"].as_str().unwrap_or("");
            match registry.install(id, url).await {
                Ok(_) => {
                    tracing::info!("App installed: {id}");
                    publish_app_list(client, registry);
                }
                Err(e) => tracing::error!("Install failed for {id}: {e}"),
            }
        }
        "pkg.command.uninstall" => {
            let id = params["id"].as_str().unwrap_or("");
            registry.uninstall(id);
            publish_app_list(client, registry);
        }
        "pkg.command.list" => {
            publish_app_list(client, registry);
        }
        _ => {}
    }
}

fn publish_app_list(client: &stardust::Client, registry: &registry::AppRegistry) {
    if let Ok(msg) = stardust::Message::new("pkg.app_list")
        .src("pkgd".to_string())
        .param(
            "apps",
            &serde_json::to_value(registry.list()).unwrap_or_default(),
        )
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}
