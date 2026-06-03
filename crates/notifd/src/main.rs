//! notifd — UOS TV Notification Daemon
//! =====================================
//! notifd menangani notifikasi sistem:
//!   - Toast/popup notifications
//!   - OTA update available
//!   - Low storage warning
//!   - Network status changes
//!   - App notifications
//!
//! Notifikasi dikirim ke UI shell (Luna) via stardust.
//! Format: { id, app, title, body, icon, priority, actions, timeout }
//!
//! Priority levels:
//!   - low: info only
//!   - normal: standard notification
//!   - high: important (OTA, security)
//!   - critical: must acknowledge (storage full, system error)
//!
//! API via stardust:
//!   notification.command.show  → { app, title, body, priority, ... }
//!   notification.command.dismiss { id }
//!   notification.command.dismiss_all

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub icon: Option<String>,
    pub timeout_ms: u64,
    pub actions: Vec<NotificationAction>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAction {
    pub id: String,
    pub label: String,
}

struct NotificationStore {
    notifications: HashMap<String, Notification>,
    max_visible: usize,
}

impl NotificationStore {
    fn new() -> Self {
        Self {
            notifications: HashMap::new(),
            max_visible: 3,
        }
    }
    fn add(&mut self, notif: Notification) {
        self.notifications.insert(notif.id.clone(), notif);
    }
    fn dismiss(&mut self, id: &str) {
        self.notifications.remove(id);
    }
    fn dismiss_all(&mut self) {
        self.notifications.clear();
    }
    fn visible(&self) -> Vec<&Notification> {
        let mut all: Vec<_> = self.notifications.values().collect();
        all.sort_by_key(|n| n.timestamp);
        all.reverse();
        all.truncate(self.max_visible);
        all
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("notifd starting...");

    let store = Arc::new(Mutex::new(NotificationStore::new()));

    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("notifd").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust: {e}");
            None
        }
    };

    if let Some(ref c) = client {
        let client = c.clone();

        // notification.command.show
        {
            let client = client.clone();
            let store = store.clone();
            if let Ok(mut rx) = c.subscribe("notification.command.show").await {
                tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Ok(params) = serde_json::from_slice::<serde_json::Value>(&msg.params)
                        {
                            let id = Uuid::new_v4().to_string();
                            let notif = Notification {
                                id,
                                app: params["app"].as_str().unwrap_or("system").into(),
                                title: params["title"].as_str().unwrap_or("").into(),
                                body: params["body"].as_str().unwrap_or("").into(),
                                priority: params["priority"].as_str().unwrap_or("normal").into(),
                                icon: params["icon"].as_str().map(String::from),
                                timeout_ms: params["timeout_ms"].as_u64().unwrap_or(5000),
                                actions: vec![],
                                timestamp: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis() as u64,
                            };
                            store.lock().unwrap().add(notif);
                            publish_notifications(&client, &store.lock().unwrap());
                        }
                    }
                });
            }
        }

        // notification.command.dismiss
        {
            let client = client.clone();
            let store = store.clone();
            if let Ok(mut rx) = c.subscribe("notification.command.dismiss").await {
                tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let Ok(params) = serde_json::from_slice::<serde_json::Value>(&msg.params)
                        {
                            if let Some(id) = params["id"].as_str() {
                                store.lock().unwrap().dismiss(id);
                                publish_notifications(&client, &store.lock().unwrap());
                            }
                        }
                    }
                });
            }
        }

        // notification.command.dismiss_all
        {
            let client = client.clone();
            let store = store.clone();
            if let Ok(mut rx) = c.subscribe("notification.command.dismiss_all").await {
                tokio::spawn(async move {
                    while let _msg = rx.recv().await {
                        store.lock().unwrap().dismiss_all();
                        publish_notifications(&client, &store.lock().unwrap());
                    }
                });
            }
        }
    }

    tracing::info!("notifd ready");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

fn publish_notifications(client: &stardust::Client, store: &NotificationStore) {
    let visible = store.visible();
    let payload = serde_json::json!({
        "notifications": visible,
        "count": store.notifications.len(),
    });
    if let Ok(msg) = stardust::Message::new("notification.list")
        .src("notifd".to_string())
        .param("payload", &payload)
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}
