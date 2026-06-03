//! update-verify CLI tool — verify OTA bundle signatures

fn main() {
    println!("update-verify v{}", env!("CARGO_PKG_VERSION"));
    println!("Usage: update-verify <bundle.yaml.signed> [public_key.hex]");
}
