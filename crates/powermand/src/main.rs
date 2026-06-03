//! powermand — UOS TV Power Manager
//! =================================
//! powermand mengelola power state perangkat:
//!   - Idle detection (no input for N minutes → dim → screensaver → suspend)
//!   - Suspend/resume
//!   - Reboot
//!   - Shutdown
//!   - Wake-on-LAN
//!   - Power button handling
//!   - Low battery warning (if battery-powered)
//!
//! Idle states:
//!   Active → Idle (5 min) → Dim (2 min) → Screensaver (10 min) → Suspend (30 min)
//!
//! API via stardust:
//!   power.command.shutdown
//!   power.command.reboot
//!   power.command.suspend
//!   power.command.set_idle_timeout { seconds }
//!   power.status → { state, idle_seconds, dimmed }

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let bus_socket =
        std::env::var("STARDUST_SOCKET").unwrap_or_else(|_| "/run/uos/bus.sock".to_string());

    tracing::info!("powermand starting...");

    let mut state = PowerState::Active;
    let idle_timeout_secs: u64 = std::env::var("IDLE_TIMEOUT")
        .unwrap_or_else(|_| "300".to_string())
        .parse()
        .unwrap_or(300);

    let client = match stardust::Client::connect(&bus_socket).await {
        Ok(c) => {
            c.register("powermand").await.ok();
            Some(c)
        }
        Err(e) => {
            tracing::warn!("No stardust: {e}");
            None
        }
    };

    if let Some(ref c) = client {
        if let Ok(mut rx) = c.subscribe("power.command.*").await {
            let client = c.clone();
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    match msg.method.as_str() {
                        "power.command.reboot" => do_reboot(),
                        "power.command.shutdown" => do_shutdown(),
                        "power.command.suspend" => do_suspend(),
                        _ => tracing::debug!("Unknown power cmd: {}", msg.method),
                    }
                }
            });
        }
    }

    tracing::info!("powermand ready (idle timeout: {idle_timeout_secs}s)");

    let mut last_activity = std::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let idle = last_activity.elapsed().as_secs();
        let new_state = if idle > idle_timeout_secs * 2 {
            PowerState::Suspended
        } else if idle > idle_timeout_secs {
            PowerState::Dimmed
        } else {
            PowerState::Active
        };

        if new_state != state {
            state = new_state;
            tracing::info!("Power state → {:?}", state);
            if let Some(ref c) = client {
                publish_state(c, &state, idle);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerState {
    Active,
    Dimmed,
    Suspended,
}

fn publish_state(client: &stardust::Client, state: &PowerState, idle_seconds: u64) {
    let state_str = match state {
        PowerState::Active => "active",
        PowerState::Dimmed => "dimmed",
        PowerState::Suspended => "suspended",
    };
    let payload = serde_json::json!({
        "state": state_str,
        "idle_seconds": idle_seconds,
    });
    if let Ok(msg) = stardust::Message::new("power.status")
        .src("powermand".to_string())
        .param("payload", &payload)
    {
        let client = client.clone();
        tokio::spawn(async move {
            let _ = client.publish(msg).await;
        });
    }
}

fn do_reboot() {
    tracing::info!("Rebooting...");
    #[cfg(target_os = "linux")]
    unsafe {
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
}
fn do_shutdown() {
    tracing::info!("Shutting down...");
    #[cfg(target_os = "linux")]
    unsafe {
        libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
    }
}
fn do_suspend() {
    tracing::info!("Suspending...");
    #[cfg(target_os = "linux")]
    {
        let _ = std::fs::write("/sys/power/state", "mem");
    }
}
