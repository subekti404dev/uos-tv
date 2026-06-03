//! casync-rs binary — CLI untuk content-addressable sync
//!
//! Commands:
//!   make INDEX   — buat index dari file/direktori
//!   extract      — rekonstruksi file dari index + store
//!   diff         — bandingkan dua index, hitung delta
//!   verify       — verifikasi index terhadap file

use casync_rs::{CaiBxIndex, ChunkStore, Chunker};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "casync-rs", about = "Content-Addressable Sync (pure Rust)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Buat index dari file
    Make {
        /// Input file
        input: String,

        /// Output index file (.caibx)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Rekonstruksi file dari index + chunk store
    Extract {
        /// Index file (.caibx)
        index: String,

        /// Chunk store directory
        #[arg(short, long, default_value = "chunks")]
        store: String,

        /// Output file
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Bandingkan dua index (delta stats)
    Diff {
        old_index: String,
        new_index: String,
    },
    /// Verifikasi file terhadap index
    Verify { index: String, file: String },
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Make { input, output } => {
            let path = std::path::Path::new(&input);
            let index = CaiBxIndex::from_file(path).expect("Failed to create index");
            let out = output.unwrap_or_else(|| format!("{}.caibx", input));
            index
                .save(std::path::Path::new(&out))
                .expect("Failed to save index");
            println!("Index saved: {out}");
            println!("  Total size: {} bytes", index.total_size);
            println!("  Chunks: {}", index.chunks.len());
        }
        Commands::Extract {
            index,
            store,
            output,
        } => {
            let idx = CaiBxIndex::load(std::path::Path::new(&index)).expect("Failed to load index");
            let store = ChunkStore::new(&store).expect("Failed to open store");
            let data = store.reconstruct(&idx).expect("Failed to reconstruct");
            let out = output.unwrap_or_else(|| format!("{}.reconstructed", index));
            std::fs::write(&out, &data).expect("Failed to write output");
            println!("Reconstructed: {out} ({} bytes)", data.len());
        }
        Commands::Diff {
            old_index,
            new_index,
        } => {
            let old = CaiBxIndex::load(std::path::Path::new(&old_index))
                .expect("Failed to load old index");
            let new = CaiBxIndex::load(std::path::Path::new(&new_index))
                .expect("Failed to load new index");
            let stats = CaiBxIndex::delta_stats(&old, &new);
            println!("Delta stats:");
            println!("  Total chunks: {}", stats.total_chunks);
            println!("  Missing chunks: {}", stats.missing_chunks);
            println!("  Total bytes: {}", stats.total_bytes);
            println!("  Download needed: {} bytes", stats.missing_bytes);
            println!("  Savings: {:.1}%", stats.savings_percent);
        }
        Commands::Verify { index, file } => {
            let idx = CaiBxIndex::load(std::path::Path::new(&index)).expect("Failed to load index");
            let data = std::fs::read(&file).expect("Failed to read file");
            match idx.verify(&data) {
                Ok(()) => println!("✓ Verification passed"),
                Err(e) => println!("✗ Verification failed: {e}"),
            }
        }
    }
}
