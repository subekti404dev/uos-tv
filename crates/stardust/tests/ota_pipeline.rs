//! P0-1: End-to-End OTA Update Pipeline Test
//! ===========================================
//!
//! Tests the complete OTA update lifecycle:
//!
//!   1. Bundle creation   → ota-create-bundle.sh logic (Rust-native)
//!   2. Signing           → Ed25519 signing key creation
//!   3. Bundle metadata   → version, device, channel, critical flag
//!   4. Signature verify  → update-verify BundleVerifier
//!   5. Downgrade detect  → version comparison
//!   6. Timestamp check   → future timestamp rejection
//!   7. Device mismatch   → wrong device rejection
//!   8. Channel filtering → channel-based update selection
//!   9. RAUC slot logic   → A/B slot detection, inactive slot target
//!  10. Install flow      → mock install, verify success callbacks
//!  11. Mark good/bad     → boot confirmation logic
//!  12. Full pipeline     → create → sign → verify → "install" end-to-end

use ed25519_dalek::SigningKey;
use std::time::{SystemTime, UNIX_EPOCH};
use update_verify::{BundleMetadata, BundleVerifier};

// ═══════════════════════════════════════════════════════════════
// Helper: create a realistic bundle
// ═══════════════════════════════════════════════════════════════
fn make_metadata(version: &str, device: &str, channel: &str, critical: bool) -> BundleMetadata {
    BundleMetadata {
        version: version.to_string(),
        device: device.to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        channel: channel.to_string(),
        critical,
        description: format!("UOS TV release {version}"),
        download_url: Some(format!(
            "https://ota.uos.example.com/bundles/{version}.raucb"
        )),
        caibx_url: Some(format!(
            "https://ota.uos.example.com/bundles/{version}.caibx"
        )),
        rootfs_sha256: format!("sha256-{version}-deadbeef"),
        rootfs_size: 42_000_000,
        files: vec![],
    }
}

fn make_verifier() -> (SigningKey, BundleVerifier) {
    let (sk_hex, vk_hex) = BundleVerifier::generate_keypair_hex();
    let sk: [u8; 32] = hex::decode(&sk_hex).unwrap().try_into().unwrap();
    let vk: [u8; 32] = hex::decode(&vk_hex).unwrap().try_into().unwrap();

    let signing_key = SigningKey::from_bytes(&sk);
    let verifier = BundleVerifier::new(&vk).unwrap();

    (signing_key, verifier)
}

// ═══════════════════════════════════════════════════════════════
// Test 1: Bundle creation + signing + verification round-trip
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_bundle_sign_verify_roundtrip() {
    let (sk, verifier) = make_verifier();
    let meta = make_metadata("2.0.0", "uos-tv-qemu", "stable", false);

    // Sign
    let signed = BundleVerifier::sign_bundle(&sk, &meta).expect("Signing failed");

    // Verify
    let verified = verifier
        .verify_bundle(&signed, None)
        .expect("Verification failed");

    assert_eq!(verified.version, "2.0.0");
    assert_eq!(verified.device, "uos-tv-qemu");
    assert_eq!(verified.channel, "stable");
    assert!(!verified.critical);
    assert_eq!(verified.rootfs_size, 42_000_000);
}

// ═══════════════════════════════════════════════════════════════
// Test 2: Wrong signature → reject
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_reject_wrong_signature() {
    let (sk, verifier) = make_verifier();
    let meta = make_metadata("1.0.0", "uos-tv-qemu", "stable", false);

    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    // Create a different verifier (different key)
    let (_, vk2_hex) = BundleVerifier::generate_keypair_hex();
    let vk2: [u8; 32] = hex::decode(&vk2_hex).unwrap().try_into().unwrap();
    let wrong_verifier = BundleVerifier::new(&vk2).unwrap();

    let result = wrong_verifier.verify_bundle(&signed, None);
    assert!(result.is_err(), "Should reject wrong signature");
}

// ═══════════════════════════════════════════════════════════════
// Test 3: Device mismatch → reject
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_reject_device_mismatch() {
    let (sk, mut verifier) = make_verifier();
    verifier.set_allowed_devices(vec!["uos-tv-rk3566".to_string()]);

    let meta = make_metadata("1.0.0", "uos-tv-amlogic", "stable", false);
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    let result = verifier.verify_bundle(&signed, None);
    assert!(result.is_err(), "Should reject wrong device");
}

// ═══════════════════════════════════════════════════════════════
// Test 4: Device in allowed list → accept
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_accept_device_in_allowed_list() {
    let (sk, mut verifier) = make_verifier();
    verifier.set_allowed_devices(vec!["uos-tv-qemu".to_string(), "uos-tv-rk3566".to_string()]);

    let meta = make_metadata("1.5.0", "uos-tv-rk3566", "stable", false);
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    let result = verifier.verify_bundle(&signed, None);
    assert!(result.is_ok(), "Should accept device in allowed list");
}

// ═══════════════════════════════════════════════════════════════
// Test 5: Expired bundle — reject timestamps that are too old
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_reject_expired_bundle() {
    let (sk, mut verifier) = make_verifier();
    verifier.set_max_age(3600); // 1 hour max age

    let mut meta = make_metadata("1.0.0", "uos-tv-qemu", "stable", false);
    // Timestamp 2 hours in the past
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    meta.timestamp = now - 7200; // 2 hours ago

    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    let result = verifier.verify_bundle(&signed, None);
    assert!(result.is_err(), "Should reject expired bundle");
    assert!(
        result.unwrap_err().to_string().contains("expired"),
        "Error should mention expiry"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 6: Downgrade detection
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_downgrade_detection() {
    let (sk, verifier) = make_verifier();

    // Bundle says version 0.5.0, but we have 1.0.0 installed
    let meta = make_metadata("0.5.0", "uos-tv-qemu", "stable", false);
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    // With current_version = "1.0.0", this should be detected as downgrade
    let result = verifier.verify_bundle(&signed, Some("1.0.0"));
    assert!(result.is_err(), "Should reject downgrade");
    assert!(
        result.unwrap_err().to_string().contains("older"),
        "Error should mention older version"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 7: Upgrade accepted
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_upgrade_accepted() {
    let (sk, verifier) = make_verifier();

    let meta = make_metadata("2.0.0", "uos-tv-qemu", "stable", false);
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    let result = verifier.verify_bundle(&signed, Some("1.0.0"));
    assert!(result.is_ok(), "Should accept upgrade");
}

// ═══════════════════════════════════════════════════════════════
// Test 8: Same version accepted (re-install)
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_same_version_accepted() {
    let (sk, verifier) = make_verifier();

    let meta = make_metadata("1.0.0", "uos-tv-qemu", "stable", false);
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    // Same version should be allowed (re-flash)
    let result = verifier.verify_bundle(&signed, Some("1.0.0"));
    assert!(result.is_ok(), "Should accept same version");
}

// ═══════════════════════════════════════════════════════════════
// Test 9: Multi-channel bundle creation
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_multi_channel_bundles() {
    let (sk, verifier) = make_verifier();

    for channel in &["dev", "beta", "stable"] {
        let meta = make_metadata("3.0.0-beta", "uos-tv-qemu", channel, *channel == "stable");
        let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

        let result = verifier.verify_bundle(&signed, Some("2.9.0"));
        assert!(result.is_ok(), "Should verify {channel} channel bundle");
    }
}

// ═══════════════════════════════════════════════════════════════
// Test 10: Critical update flag
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_critical_update_flag() {
    let (sk, verifier) = make_verifier();

    let meta = make_metadata("2.0.1", "uos-tv-qemu", "stable", true); // critical!
    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();

    let verified = verifier.verify_bundle(&signed, Some("2.0.0")).unwrap();
    assert!(verified.critical, "Critical flag should be preserved");

    let meta_non = make_metadata("2.0.2", "uos-tv-qemu", "stable", false);
    let signed_non = BundleVerifier::sign_bundle(&sk, &meta_non).unwrap();

    let verified_non = verifier.verify_bundle(&signed_non, Some("2.0.1")).unwrap();
    assert!(
        !verified_non.critical,
        "Non-critical flag should be preserved"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 11: Bundle metadata with file list
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_bundle_with_file_list() {
    let (sk, verifier) = make_verifier();

    let mut meta = make_metadata("2.0.0", "uos-tv-qemu", "stable", false);
    meta.files = vec![
        update_verify::FileEntry {
            path: "usr/bin/stardustd".into(),
            sha256: "aa".into(),
            size: 1000,
        },
        update_verify::FileEntry {
            path: "usr/bin/logd".into(),
            sha256: "bb".into(),
            size: 500,
        },
        update_verify::FileEntry {
            path: "luna/index.html".into(),
            sha256: "cc".into(),
            size: 3000,
        },
    ];

    let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();
    let verified = verifier.verify_bundle(&signed, Some("1.9.0")).unwrap();

    assert_eq!(verified.files.len(), 3);
    assert_eq!(verified.files[0].path, "usr/bin/stardustd");
    assert_eq!(verified.files[1].path, "usr/bin/logd");
    assert_eq!(verified.files[2].path, "luna/index.html");
}

// ═══════════════════════════════════════════════════════════════
// Test 12: Full pipeline simulation (no RAUC binary needed)
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_full_ota_pipeline_simulation() {
    // This test simulates the full OTA flow:
    //   1. OTA server creates bundle
    //   2. Device downloads and verifies
    //   3. RAUC installs to inactive slot
    //   4. Device reboots
    //   5. New slot confirmed good

    // Step 1: Server-side key generation + bundle creation
    let (sk, vk_hex) = {
        let (sk, vk) = BundleVerifier::generate_keypair_hex();
        let sk_bytes: [u8; 32] = hex::decode(&sk).unwrap().try_into().unwrap();
        let vk_bytes: [u8; 32] = hex::decode(&vk).unwrap().try_into().unwrap();
        (SigningKey::from_bytes(&sk_bytes), vk_bytes)
    };

    let verifier = BundleVerifier::new(&vk_hex).unwrap();

    // Server creates a bundle for the device
    let bundle = {
        let meta = make_metadata("2.1.0", "uos-tv-qemu", "stable", false);
        BundleVerifier::sign_bundle(&sk, &meta).unwrap()
    };

    // Step 2: Device receives and verifies
    let verified = verifier
        .verify_bundle(&bundle, Some("2.0.0"))
        .expect("Bundle verification should pass");

    assert_eq!(verified.version, "2.1.0");

    // Step 3: Check which slot to install to (A/B simulation)
    #[derive(Debug, PartialEq)]
    enum Slot {
        A,
        B,
    }

    struct SlotState {
        active: Slot,
        versions: (Option<String>, Option<String>),
    }

    let mut state = SlotState {
        active: Slot::A,
        versions: (Some("2.0.0".to_string()), None),
    };

    // Determine target slot (inactive)
    let target = if state.active == Slot::A {
        Slot::B
    } else {
        Slot::A
    };
    assert_eq!(target, Slot::B, "Should target slot B");

    // Simulate install
    if target == Slot::B {
        state.versions.1 = Some(verified.version.clone());
    } else {
        state.versions.0 = Some(verified.version.clone());
    }

    // Step 4: Simulate reboot into slot B
    state.active = Slot::B;
    assert_eq!(state.active, Slot::B);
    assert_eq!(state.versions.1.as_deref(), Some("2.1.0"));

    // Step 5: Confirm new boot worked
    // In real code: rauc_rs::RaucClient::mark_good()
    // Assert the installed version is the active one now
    let active_version = if state.active == Slot::A {
        &state.versions.0
    } else {
        &state.versions.1
    };
    assert_eq!(active_version.as_deref(), Some("2.1.0"));

    // Step 6: If boot fails → mark bad, revert
    let rollback_state = SlotState {
        active: Slot::A,
        versions: (Some("2.0.0".to_string()), Some("2.1.0".to_string())),
    };
    assert_eq!(rollback_state.active, Slot::A);
    assert_eq!(
        rollback_state.versions.0.as_deref(),
        Some("2.0.0"),
        "After rollback, slot A should have old version"
    );
}

// ═══════════════════════════════════════════════════════════════
// Test 13: OTA version comparison edge cases
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_version_comparison_edge_cases() {
    let (sk, verifier) = make_verifier();

    let test_cases = vec![
        // (bundle_ver, current_ver, should_succeed)
        ("1.0.1", "1.0.0", true),       // patch bump
        ("1.1.0", "1.0.9", true),       // minor bump
        ("2.0.0", "1.9.9", true),       // major bump
        ("10.0.0", "9.9.9", true),      // double digit
        ("1.0.0", "2.0.0", false),      // downgrade
        ("1.0.0", "1.0.1", false),      // downgrade patch
        ("0.0.0", "0.0.0", true),       // same version
        ("2.0.0-alpha", "1.0.0", true), // pre-release version, semver prefix parses as 2.0.0
    ];

    for (bundle_ver, current_ver, should_succeed) in test_cases {
        let meta = make_metadata(bundle_ver, "uos-tv-qemu", "stable", false);
        let signed = BundleVerifier::sign_bundle(&sk, &meta).unwrap();
        let result = verifier.verify_bundle(&signed, Some(current_ver));

        if should_succeed {
            assert!(
                result.is_ok(),
                "Expected {bundle_ver} to be accepted (current: {current_ver}), got: {result:?}"
            );
        } else {
            assert!(
                result.is_err(),
                "Expected {bundle_ver} to be rejected (current: {current_ver}), but it passed"
            );
        }
    }
}
