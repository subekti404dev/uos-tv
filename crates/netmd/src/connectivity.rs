//! Internet connectivity check (captive portal detection)

use std::time::Duration;

/// Cek apakah internet benar-benar tersedia.
///
/// Strategy: HTTP HEAD ke known endpoint.
/// Endpoint: http://detect.uos-tv.local/check (self-hosted)
/// Fallback: http://connectivity-check.ubuntu.com (public)
pub async fn check_internet() -> bool {
    let urls = [
        "http://detect.uos-tv.local/check",
        "http://connectivity-check.ubuntu.com",
    ];

    for url in &urls {
        match reqwest::Client::new()
            .head(*url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => return true,
            Ok(_) => continue,
            Err(_) => continue,
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Butuh internet
    async fn test_connectivity_check() {
        let has_internet = check_internet().await;
        println!("Internet available: {has_internet}");
    }
}
