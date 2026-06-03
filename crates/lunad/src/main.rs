// lunad — Tiny HTTP static file server for Luna UI
// ==================================================
// Serves Luna UI HTML/CSS/JS from /var/www/luna (or --root <path>).
// Supports: GET, HEAD, MIME type detection, directory index (index.html fallback).
// No external HTTP deps — pure tokio TCP.

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

static DEFAULT_ROOT: &str = "/var/www/luna";
static DEFAULT_PORT: u16 = 80;

fn mime_type(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}

fn resolve_path(root: &str, request_path: &str) -> Option<PathBuf> {
    // Decode percent-encoded chars
    let decoded = percent_decode(request_path);
    // Security: prevent directory traversal
    let path = PathBuf::from(root).join(decoded.trim_start_matches('/'));
    let canonical = path.canonicalize().ok()?;
    let root_canonical = PathBuf::from(root).canonicalize().ok()?;
    if canonical.starts_with(&root_canonical) {
        Some(canonical)
    } else {
        None
    }
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let Ok(byte) =
                    u8::from_str_radix(&std::str::from_utf8(&[hi, lo]).unwrap_or("00"), 16)
                {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn parse_request(line: &str) -> (&str, &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        (parts[0], parts[1])
    } else {
        ("GET", "/")
    }
}

async fn handle_connection(mut stream: tokio::net::TcpStream, root: String) -> std::io::Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = match tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf))
        .await
    {
        Ok(Ok(n)) => n,
        _ => return Ok(()),
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("GET / HTTP/1.0");
    let (method, req_path) = parse_request(first_line);

    // Only handle GET and HEAD
    if method != "GET" && method != "HEAD" {
        let resp = "HTTP/1.0 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes()).await;
        return Ok(());
    }

    // Strip query string
    let clean_path = req_path.split('?').next().unwrap_or(req_path);

    // Resolve to filesystem path
    let fs_path = if clean_path == "/" || clean_path.is_empty() {
        resolve_path(&root, "/index.html")
    } else {
        resolve_path(&root, clean_path)
    };

    match fs_path {
        Some(path) if path.is_file() => match tokio::fs::read(&path).await {
            Ok(content) => {
                let mime = mime_type(path.to_str().unwrap_or(""));
                let resp = format!(
                        "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        mime,
                        content.len()
                    );
                let _ = stream.write_all(resp.as_bytes()).await;
                if method == "GET" {
                    let _ = stream.write_all(&content).await;
                }
            }
            Err(_) => {
                let resp = "HTTP/1.0 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        },
        _ => {
            let body = b"<html><body><h1>404 Not Found</h1></body></html>";
            let resp = format!(
                "HTTP/1.0 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            if method == "GET" {
                let _ = stream.write_all(body).await;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::init();

    // Parse CLI: lunad [--root <path>] [--port <num>]
    let args: Vec<String> = std::env::args().collect();
    let mut root = DEFAULT_ROOT.to_string();
    let mut port = DEFAULT_PORT;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" | "-r" => {
                i += 1;
                if i < args.len() {
                    root = args[i].clone();
                }
            }
            "--port" | "-p" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().unwrap_or(DEFAULT_PORT);
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Ensure root exists
    if !PathBuf::from(&root).exists() {
        tracing::error!("Root directory not found: {}", root);
        std::process::exit(1);
    }

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(
        "Luna HTTP server listening on http://{} (root: {})",
        addr,
        root
    );

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tracing::debug!("Connection from {}", peer);
                let root = root.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, root).await {
                        tracing::debug!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::error!("Accept error: {}", e);
            }
        }
    }
}
