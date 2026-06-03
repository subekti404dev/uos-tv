//! Supervisor — Process lifecycle management.
//!
//! Manages startup, monitoring, restart, and shutdown of all services.

use crate::graph::DependencyGraph;
use crate::manifest::{RestartPolicy, ServiceManifest};
use dashmap::DashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::broadcast;

/// Status dari satu service.
#[derive(Debug, Clone, PartialEq)]
enum ServiceStatus {
    /// Belum dimulai
    Pending,
    /// Sedang proses startup
    Starting,
    /// Running normal
    Running,
    /// Gagal start / crash — menunggu restart
    Failed(String),
    /// Crash loop detected — berhenti restart
    CrashedLoop(String),
    /// Service berhenti (restart = never)
    Stopped,
}

/// Informasi runtime untuk satu service.
struct ServiceRuntime {
    manifest: ServiceManifest,
    status: ServiceStatus,
    child: Option<Child>,
    pid: Option<u32>,
    restart_count: u32,
    crash_timestamps: Vec<Instant>,
    started_at: Option<Instant>,
}

impl ServiceRuntime {
    fn new(manifest: ServiceManifest) -> Self {
        Self {
            manifest,
            status: ServiceStatus::Pending,
            child: None,
            pid: None,
            restart_count: 0,
            crash_timestamps: Vec::new(),
            started_at: None,
        }
    }
}

pub struct Supervisor {
    services: Arc<DashMap<String, ServiceRuntime>>,
    graph: DependencyGraph,
    startup_order: Vec<String>,
    bus_socket: PathBuf,
    shutdown_tx: broadcast::Sender<()>,
}

impl Supervisor {
    pub fn new(
        manifests: Vec<ServiceManifest>,
        graph: DependencyGraph,
        bus_socket: PathBuf,
    ) -> Self {
        let startup_order: Vec<String> = graph
            .topological_sort()
            .iter()
            .map(|s| s.name.clone())
            .collect();

        let services = Arc::new(DashMap::new());
        for m in manifests {
            services.insert(m.name.clone(), ServiceRuntime::new(m));
        }

        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            services,
            graph,
            startup_order,
            bus_socket,
            shutdown_tx,
        }
    }

    /// Boot sequence: mulai semua service sesuai urutan.
    pub async fn boot(&mut self) -> Result<(), String> {
        tracing::info!("=== UOS TV Boot Sequence ===");
        let start = Instant::now();

        for name in &self.startup_order {
            let result = self.start_service(name).await;

            match result {
                Ok(()) => {
                    let elapsed = start.elapsed();
                    tracing::info!("  [OK]  {name} ({:.1}s)", elapsed.as_secs_f32());
                }
                Err(e) => {
                    let svc = self.services.get(name).unwrap();
                    if svc.manifest.critical {
                        tracing::error!("  [FAIL] {name} (CRITICAL): {e}");
                        return Err(format!("Critical service '{name}' failed to start: {e}"));
                    } else {
                        tracing::warn!("  [WARN] {name}: {e} (non-critical, continuing)");
                    }
                }
            }
        }

        let total = start.elapsed();
        tracing::info!("=== Boot complete in {:.1}s ===", total.as_secs_f32());
        Ok(())
    }

    /// Mulai satu service.
    async fn start_service(&self, name: &str) -> Result<(), String> {
        let manifest = {
            let svc = self.services.get(name).ok_or("service not found")?;
            svc.manifest.clone()
        };

        // Tunggu semua dependency selesai start
        for dep in &manifest.dependencies {
            self.wait_until_running(dep).await?;
        }

        // Spawn process
        let mut cmd = Command::new(&manifest.binary);
        cmd.args(&manifest.args);

        // Environment
        for env_var in &manifest.env {
            if let Some((key, value)) = env_var.split_once('=') {
                cmd.env(key, value);
            }
        }
        cmd.env("UOS_SERVICE_NAME", &manifest.name);
        cmd.env("STARDUST_SOCKET", self.bus_socket.to_str().unwrap_or(""));
        cmd.env("UOS_LOG", "info");

        // Redirect stdout/stderr
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Capture caps for pre_exec closure
        let keep_caps = manifest.caps.keep.clone();

        // Set process group + drop capabilities
        unsafe {
            cmd.pre_exec(move || {
                // Buat process group baru
                libc::setpgid(0, 0);
                // Drop capabilities from bounding set
                monitord::sec::apply_capability_bounds(&keep_caps);
                Ok(())
            });
        }

        tracing::debug!("Starting {}: {} {:?}", name, manifest.binary, manifest.args);

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", manifest.binary))?;

        let pid = child.id();

        // Update runtime state
        {
            let mut svc = self.services.get_mut(name).unwrap();
            svc.status = ServiceStatus::Starting;
            svc.child = Some(child);
            svc.pid = pid;
            svc.started_at = Some(Instant::now());
        }

        tracing::info!("{} started (PID {})", name, pid.map_or(0, |p| p));

        // Health check — jika ada, tunggu sampai service ping
        if let Some(health_method) = &manifest.health_check {
            let timeout = Duration::from_secs(manifest.startup_timeout_secs);

            match tokio::time::timeout(timeout, self.wait_for_health(name, health_method)).await {
                Ok(Ok(())) => {
                    tracing::debug!("{} health check OK", name);
                }
                Ok(Err(e)) => {
                    tracing::warn!("{} health check failed: {}", name, e);
                    // Tetap lanjut — health check failure bukan berarti service mati
                }
                Err(_) => {
                    tracing::warn!(
                        "{} health check timed out after {}s",
                        name,
                        timeout.as_secs()
                    );
                }
            }
        }

        // Mark as running
        {
            let mut svc = self.services.get_mut(name).unwrap();
            svc.status = ServiceStatus::Running;
        }

        Ok(())
    }

    /// Tunggu sampai service running.
    async fn wait_until_running(&self, name: &str) -> Result<(), String> {
        let start = Instant::now();
        let timeout = Duration::from_secs(30);

        loop {
            if start.elapsed() > timeout {
                return Err(format!("Timeout waiting for {name}"));
            }

            let status = {
                self.services
                    .get(name)
                    .map(|s| s.status.clone())
                    .unwrap_or(ServiceStatus::Failed("not found".into()))
            };

            match status {
                ServiceStatus::Running => return Ok(()),
                ServiceStatus::Failed(ref reason) => {
                    return Err(format!("Dependency {name} failed: {reason}"));
                }
                ServiceStatus::CrashedLoop(_) => {
                    return Err(format!("Dependency {name} crashed-loop"));
                }
                _ => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Perform a health check on a service.
    /// Returns Ok(()) if healthy, Err(reason) if unhealthy.
    async fn check_health(&self, name: &str, method: &str) -> Result<(), String> {
        match method {
            m if m.starts_with("tcp:") => {
                // TCP port check: "tcp:80" or "tcp:127.0.0.1:8080"
                let addr = m.strip_prefix("tcp:").unwrap();
                let target = if addr.contains(':') {
                    addr.to_string()
                } else {
                    format!("127.0.0.1:{addr}")
                };
                match tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio::net::TcpStream::connect(&target),
                )
                .await
                {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(e)) => Err(format!("TCP connect {target}: {e}")),
                    Err(_) => Err(format!("TCP connect {target}: timeout")),
                }
            }
            m if m.starts_with("stardust.") || m.starts_with("rpc:") => {
                // Stardust RPC ping: "stardust.ping" or "rpc:audiod.status"
                let rpc_method = if let Some(r) = m.strip_prefix("rpc:") {
                    r.to_string()
                } else {
                    m.to_string()
                };
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    self.stardust_rpc_call(name, &rpc_method),
                )
                .await
                {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(format!("RPC {rpc_method}: timeout")),
                }
            }
            m if m == "process" || m.is_empty() => {
                // Process alive check — already handled by monitor_services
                Ok(())
            }
            _ => {
                tracing::warn!("{name}: unknown health check method '{method}'");
                Ok(()) // Unknown method — assume OK
            }
        }
    }

    /// Simple stardust RPC call via Unix socket.
    /// Sends a raw CBOR frame and reads response.
    async fn stardust_rpc_call(&self, _name: &str, method: &str) -> Result<(), String> {
        let socket_path = &self.bus_socket;

        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(mut stream) => {
                // Build a simple ping message using stardust Message
                let msg = stardust::Message::new(method)
                    .src("monitord.health".to_string())
                    .dst("*".to_string());

                // Encode as CBOR frame
                let mut cbor_bytes = Vec::new();
                if ciborium::into_writer(&msg, &mut cbor_bytes).is_err() {
                    return Err("CBOR encode failed".into());
                }

                // Write frame header (4 bytes LE) + payload
                let len = cbor_bytes.len() as u32;
                stream
                    .write_all(&len.to_le_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stream
                    .write_all(&cbor_bytes)
                    .await
                    .map_err(|e| e.to_string())?;

                // Read response (best-effort, don't block health check)
                let mut buf = vec![0u8; 1024];
                let _ = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await;

                Ok(())
            }
            Err(e) => Err(format!("stardust connect: {e}")),
        }
    }

    /// Wait for service health check via startup timeout.
    async fn wait_for_health(&self, name: &str, method: &str) -> Result<(), String> {
        // Poll health until it passes
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut failures = 0u32;

        loop {
            if Instant::now() > deadline {
                return Err(format!(
                    "Health check '{method}' timed out after {failures} failures"
                ));
            }

            match self.check_health(name, method).await {
                Ok(()) => {
                    if failures > 0 {
                        tracing::info!("{name}: health OK after {failures} retries");
                    }
                    return Ok(());
                }
                Err(e) => {
                    failures += 1;
                    tracing::debug!("{name}: health check retry {failures}: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Main supervisor loop — monitor services, handle crashes, health checks, watchdog.
    pub async fn run(&mut self) {
        tracing::info!("Supervisor running, monitoring services...");

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut health_tick = tokio::time::interval(Duration::from_secs(5));
        let mut watchdog_tick = tokio::time::interval(Duration::from_secs(10));

        // Track health failures for auto-restart
        let health_failures: Arc<DashMap<String, u32>> = Arc::new(DashMap::new());

        // Initialize hardware watchdog if available
        let watchdog_fd = self.init_watchdog();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Shutdown signal received");
                    self.shutdown_all().await;
                    break;
                }
                _ = tick.tick() => {
                    // Check process liveness + handle crashes
                    self.monitor_services().await;
                }
                _ = health_tick.tick() => {
                    // Periodic health checks on all running services
                    self.health_check_all(&health_failures).await;
                }
                _ = watchdog_tick.tick() => {
                    // Kick hardware watchdog
                    if let Some(ref fd) = watchdog_fd {
                        self.kick_watchdog(fd);
                    }
                }
            }
        }
    }

    /// Initialize hardware watchdog device.
    /// Returns file descriptor if /dev/watchdog is available.
    fn init_watchdog(&self) -> Option<std::fs::File> {
        match std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/watchdog")
        {
            Ok(f) => {
                tracing::info!("Hardware watchdog: /dev/watchdog enabled");
                Some(f)
            }
            Err(_) => {
                tracing::debug!("Hardware watchdog: /dev/watchdog not available");
                None
            }
        }
    }

    /// Kick the hardware watchdog to prevent system reset.
    fn kick_watchdog(&self, fd: &std::fs::File) {
        use std::io::Write;
        let mut f = fd;
        // Write a magic character to keep watchdog alive
        // Most watchdogs accept any write, some need specific character
        if f.write_all(b"\n").is_err() {
            tracing::warn!("Watchdog kick failed");
        }
    }

    /// Run health checks on all running services.
    /// Accumulates failures and triggers restart after threshold.
    async fn health_check_all(&self, failures: &DashMap<String, u32>) {
        let max_health_failures = 3u32;

        for entry in self.services.iter() {
            let name = entry.key().clone();
            let health_method = {
                let svc = entry.value();
                if !matches!(svc.status, ServiceStatus::Running) {
                    continue;
                }
                svc.manifest.health_check.clone()
            };

            let method = match health_method {
                Some(ref m) if !m.is_empty() => m.clone(),
                _ => continue, // No health check configured
            };

            match self.check_health(&name, &method).await {
                Ok(()) => {
                    // Healthy — reset failure count
                    failures.remove(&name);
                }
                Err(e) => {
                    let count = failures
                        .entry(name.clone())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                    let c = *count;

                    if c >= max_health_failures {
                        tracing::error!(
                            "{name}: health check FAILED {c} times ({e}) — restarting..."
                        );
                        failures.remove(&name);

                        // Kill and restart
                        self.kill_service(&name).await;
                        let mut svc = self.services.get_mut(&name).unwrap();
                        svc.status = ServiceStatus::Failed(format!("health: {e}"));

                        // Restart service
                        let delay = svc.manifest.restart_delay_ms;
                        drop(svc);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        if let Err(e) = self.start_service(&name).await {
                            tracing::error!("Failed to restart {name}: {e}");
                        }
                    } else {
                        tracing::warn!(
                            "{name}: health check failed ({c}/{max_health_failures}): {e}"
                        );
                    }
                }
            }
        }
    }

    /// Kill a service process forcibly.
    async fn kill_service(&self, name: &str) {
        // Snapshot pid from the service state
        let pid = { self.services.get(name).and_then(|s| s.pid) };

        if let Some(pid) = pid {
            let p = nix::unistd::Pid::from_raw(pid as i32);
            let _ = nix::sys::signal::kill(p, nix::sys::signal::Signal::SIGTERM);
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = nix::sys::signal::kill(p, nix::sys::signal::Signal::SIGKILL);
        }

        // Clean up runtime state
        if let Some(mut svc) = self.services.get_mut(name) {
            if let Some(ref mut child) = svc.child {
                let _ = child.start_kill();
            }
            svc.child = None;
            svc.pid = None;
        }
    }

    /// Monitor semua service — cek yang mati, handle restart.
    async fn monitor_services(&self) {
        let mut to_restart = Vec::new();

        // Collect service yang perlu dicek
        for entry in self.services.iter() {
            let name = entry.key().clone();
            let needs_check = {
                let svc = entry.value();
                matches!(svc.status, ServiceStatus::Running | ServiceStatus::Starting)
            };

            if needs_check {
                // Cek apakah process masih alive
                let mut svc = self.services.get_mut(&name).unwrap();

                if let Some(ref mut child) = svc.child {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code();
                            tracing::warn!("{} exited (code: {:?})", name, code);

                            svc.status = ServiceStatus::Failed(format!("exit code: {code:?}"));
                            svc.child = None;

                            // Tentukan apakah perlu restart
                            let should_restart = match svc.manifest.restart {
                                RestartPolicy::Always => true,
                                RestartPolicy::OnFailure => code != Some(0),
                                RestartPolicy::Never => false,
                            };

                            // Crash loop detection
                            let now = Instant::now();
                            let window_secs = svc.manifest.crash_window_secs;
                            svc.crash_timestamps.push(now);
                            svc.crash_timestamps.retain(|t| {
                                now.duration_since(*t) < Duration::from_secs(window_secs)
                            });

                            if svc.crash_timestamps.len() > svc.manifest.max_crash_count as usize {
                                tracing::error!(
                                    "{} crash loop detected ({} crashes in {}s)",
                                    name,
                                    svc.crash_timestamps.len(),
                                    svc.manifest.crash_window_secs
                                );
                                svc.status = ServiceStatus::CrashedLoop(format!(
                                    "{} crashes in {}s",
                                    svc.crash_timestamps.len(),
                                    svc.manifest.crash_window_secs
                                ));

                                if svc.manifest.critical {
                                    tracing::error!(
                                        "CRITICAL service {} crashed-loop — SYSTEM PANIC",
                                        name
                                    );
                                    self.shutdown_all().await;
                                    std::process::exit(1);
                                }
                            } else if should_restart {
                                to_restart.push((name.clone(), svc.manifest.restart_delay_ms));
                            } else {
                                svc.status = ServiceStatus::Stopped;
                            }
                        }
                        Ok(None) => {
                            // Masih running
                        }
                        Err(e) => {
                            tracing::error!("{} try_wait error: {e}", name);
                        }
                    }
                }
            }
        }

        // Restart service yang perlu (setelah delay)
        for (name, _delay_ms) in to_restart {
            tracing::info!("Restarting {}...", name);
            if let Err(e) = self.start_service(&name).await {
                tracing::error!("Failed to restart {}: {}", name, e);
            }
        }
    }

    /// Shutdown semua service (reverse order).
    async fn shutdown_all(&self) {
        tracing::info!("Shutting down all services...");

        for name in self.startup_order.iter().rev() {
            let pid = self.services.get(name).and_then(|s| s.pid);
            let mut svc = self.services.get_mut(name).unwrap();

            if let Some(ref mut child) = svc.child {
                tracing::info!("Stopping {}...", name);

                // Kirim SIGTERM
                if let Some(pid) = pid {
                    let p = nix::unistd::Pid::from_raw(pid as i32);
                    let _ = nix::sys::signal::kill(p, nix::sys::signal::Signal::SIGTERM);
                }

                tokio::time::sleep(Duration::from_secs(2)).await;

                // SIGKILL jika masih hidup
                if let Ok(None) = child.try_wait() {
                    if let Some(pid) = pid {
                        let p = nix::unistd::Pid::from_raw(pid as i32);
                        let _ = nix::sys::signal::kill(p, nix::sys::signal::Signal::SIGKILL);
                    }
                    let _ = child.wait().await;
                }

                svc.status = ServiceStatus::Stopped;
                svc.child = None;
                tracing::info!("{} stopped", name);
            }
        }

        tracing::info!("All services stopped");
    }
}
