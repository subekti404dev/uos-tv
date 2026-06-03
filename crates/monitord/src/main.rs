//! monitord — Process Supervisor untuk UOS TV
//! ==========================================
//!
//! monitord bertanggung jawab atas lifecycle semua system services.
//! Membaca service manifest dari /usr/share/uos/services.d/*.yaml,
//! membangun dependency graph, dan memulai service sesuai urutan
//! topologi-sort.
//!
//! Fitur:
//!   - Service manifest (YAML): nama, binary, args, dependencies, restart policy
//!   - Dependency graph dengan topologi sort
//!   - Parallel startup untuk service independen
//!   - Restart policy: always, on-failure, never
//!   - Crash loop detection: max 3 restart dalam 30 detik → stop
//!   - Health check via Stardust IPC (ping service)
//!   - Log forwarding ke logd

use monitord::graph;
use monitord::manifest;
mod supervisor;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "monitord", about = "UOS TV Process Supervisor")]
struct Args {
    /// Direktori service manifests
    #[arg(short, long, default_value = "/usr/share/uos/services.d")]
    manifests: PathBuf,

    /// Path ke stardust socket (untuk health check + notifikasi)
    #[arg(short, long, default_value = "/run/uos/bus.sock")]
    bus_socket: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("UOS_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let args = Args::parse();

    tracing::info!("monitord v{} starting...", env!("CARGO_PKG_VERSION"));
    tracing::info!("Loading manifests from {}", args.manifests.display());

    // 1. Load service manifests
    let services = match manifest::load_from_dir(&args.manifests) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to load manifests: {e}");
            std::process::exit(1);
        }
    };

    tracing::info!("Loaded {} services", services.len());

    // 2. Build & verify dependency graph
    let graph = match graph::DependencyGraph::new(&services) {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("Invalid dependency graph: {e}");
            std::process::exit(1);
        }
    };

    // 3. Compute startup order (topological sort)
    let startup_order = graph.topological_sort();
    tracing::info!(
        "Startup order: [{}]",
        startup_order
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(" → ")
    );

    // 4. Start supervisor
    let mut supervisor = supervisor::Supervisor::new(services, graph, args.bus_socket);

    // 5. Boot sequence
    if let Err(e) = supervisor.boot().await {
        tracing::error!("Boot failed: {e}");
        std::process::exit(1);
    }

    // 6. Main loop — monitor services, handle exits
    supervisor.run().await;
}
