//! stardustd — Stardust IPC Broker Daemon

use clap::Parser;
use stardust::broker::Broker;

#[derive(Parser, Debug)]
#[command(name = "stardustd", about = "UOS TV IPC Message Broker")]
struct Args {
    /// Unix socket path for internal service communication
    #[arg(short, long, default_value = "/run/uos/bus.sock")]
    socket: String,

    /// WebSocket address for Luna UI bridge (e.g., 127.0.0.1:9090)
    #[arg(long)]
    ws_addr: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("STARDUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let args = Args::parse();

    if let Some(parent) = std::path::Path::new(&args.socket).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    tracing::info!("stardustd starting on {}", args.socket);
    if let Some(ref ws) = args.ws_addr {
        tracing::info!("WebSocket bridge: ws://{ws}");
    }

    let mut broker = Broker::new(args.socket);
    if let Some(ws) = args.ws_addr {
        broker = broker.with_ws(ws);
    }

    if let Err(e) = broker.run().await {
        tracing::error!("broker fatal error: {e}");
        std::process::exit(1);
    }
}
