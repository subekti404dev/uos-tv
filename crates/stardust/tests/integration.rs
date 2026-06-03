//! Integration tests for UOS TV — IPC flow
//! ==========================================
//! Tests:
//!   1. stardust broker startup → connect → publish → subscribe → receive
//!   2. monitord service manifest parsing → dependency sort
//!   3. update-verify sign → verify round-trip
//!   4. casync index → chunk → delta

#![allow(unused_imports)]

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// Test: IPC round-trip via stardust (Unix socket).
    /// Starts broker, connects client, publishes, subscribes, receives.
    #[tokio::test]
    #[ignore = "requires timing-sensitive IPC — run on Linux target"]
    async fn test_stardust_pub_sub() {
        let socket = "/tmp/test-uos-bus.sock";
        let _ = std::fs::remove_file(socket);

        // Start broker
        let broker = stardust::Broker::new(socket.to_string());
        let broker_handle = tokio::spawn(async move {
            broker.run().await.ok();
        });

        // Give broker time to start and accept connections
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Connect client A (subscriber)
        let client_a = stardust::Client::connect(socket).await;
        assert!(client_a.is_ok(), "Client A connect failed");
        let client_a = client_a.unwrap();

        // Subscribe
        let mut rx = client_a.subscribe("test.topic").await;
        assert!(rx.is_ok(), "Subscribe failed");
        let mut rx = rx.unwrap();

        // Small delay to let subscription propagate
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect client B (publisher)
        let client_b = stardust::Client::connect(socket).await;
        assert!(client_b.is_ok(), "Client B connect failed");
        let client_b = client_b.unwrap();
        client_b.register("test-publisher").await.ok();

        // Publish
        let msg = stardust::Message::new("test.topic")
            .src("test-publisher".to_string())
            .param("hello", &"world")
            .unwrap();
        let result = client_b.publish(msg).await;
        assert!(result.is_ok(), "Publish failed");

        // Receive
        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(received.is_ok(), "Timeout waiting for message");
        let received = received.unwrap();
        assert!(received.is_some(), "No message received");
        let msg = received.unwrap();
        assert_eq!(msg.method, "test.topic");
        assert_eq!(msg.src, "test-publisher");

        // Cleanup
        broker_handle.abort();
    }

    /// Test: monitord manifest parsing.
    #[test]
    fn test_monitord_manifest_parsing() {
        let yaml = r#"
name: test-service
description: "A test service"
binary: /usr/bin/test-svc
args:
  - "--verbose"
dependencies:
  - logd
  - stardust
restart: always
critical: true
startup_timeout_secs: 10
"#;
        let manifest: monitord::manifest::ServiceManifest =
            serde_yaml::from_str(yaml).expect("Parse failed");

        assert_eq!(manifest.name, "test-service");
        assert_eq!(manifest.binary, "/usr/bin/test-svc");
        assert_eq!(manifest.args, vec!["--verbose"]);
        assert_eq!(manifest.dependencies, vec!["logd", "stardust"]);
        assert!(manifest.critical);
        assert_eq!(manifest.startup_timeout_secs, 10);
    }

    /// Test: update-verify sign → verify round-trip.
    #[test]
    fn test_sign_and_verify() {
        let (sk_hex, vk_hex) = update_verify::BundleVerifier::generate_keypair_hex();
        let sk: [u8; 32] = hex::decode(&sk_hex).unwrap().try_into().unwrap();
        let vk: [u8; 32] = hex::decode(&vk_hex).unwrap().try_into().unwrap();

        let signing_key = ed25519_dalek::SigningKey::from_bytes(&sk);
        let verifier = update_verify::BundleVerifier::new(&vk).unwrap();

        let meta = update_verify::BundleMetadata {
            version: "1.0.0".into(),
            device: "qemu-virt".into(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            channel: "stable".into(),
            critical: false,
            description: "Test bundle".into(),
            download_url: None,
            caibx_url: None,
            rootfs_sha256: "abc123".into(),
            rootfs_size: 1024,
            files: vec![],
        };

        let signed = update_verify::BundleVerifier::sign_bundle(&signing_key, &meta).unwrap();
        let verified = verifier.verify_bundle(&signed, None).unwrap();
        assert_eq!(verified.version, "1.0.0");
    }

    /// Test: monitord dependency graph topological sort.
    #[test]
    fn test_dependency_sort() {
        use monitord::graph::DependencyGraph;
        use monitord::manifest::{RestartPolicy, ServiceManifest};

        let manifests = vec![
            ServiceManifest {
                name: "logd".into(),
                dependencies: vec![],
                binary: "/usr/bin/logd".into(),
                args: vec![],
                env: vec![],
                description: "".into(),
                after: vec![],
                restart: RestartPolicy::Always,
                restart_delay_ms: 1000,
                max_crash_count: 3,
                crash_window_secs: 30,
                critical: false,
                startup_timeout_secs: 5,
                health_check: None,
                caps: Default::default(),
            },
            ServiceManifest {
                name: "stardust".into(),
                dependencies: vec![],
                binary: "/usr/bin/stardustd".into(),
                args: vec![],
                env: vec![],
                description: "".into(),
                after: vec![],
                restart: RestartPolicy::Always,
                restart_delay_ms: 500,
                max_crash_count: 5,
                crash_window_secs: 30,
                critical: true,
                startup_timeout_secs: 5,
                health_check: None,
                caps: Default::default(),
            },
            ServiceManifest {
                name: "netmd".into(),
                dependencies: vec!["logd".into(), "stardust".into()],
                binary: "/usr/bin/netmd".into(),
                args: vec![],
                env: vec![],
                description: "".into(),
                after: vec![],
                restart: RestartPolicy::Always,
                restart_delay_ms: 1000,
                max_crash_count: 3,
                crash_window_secs: 30,
                critical: false,
                startup_timeout_secs: 10,
                health_check: None,
                caps: Default::default(),
            },
        ];

        let graph = DependencyGraph::new(&manifests).expect("Graph creation failed");
        let order = graph.topological_sort();
        assert_eq!(order.len(), 3);

        let names: Vec<&str> = order.iter().map(|s| s.name.as_str()).collect();

        // logd and stardust (no deps) should come before netmd (depends on both)
        let netmd_pos = names.iter().position(|&n| n == "netmd").unwrap();
        let logd_pos = names.iter().position(|&n| n == "logd").unwrap();
        let stardust_pos = names.iter().position(|&n| n == "stardust").unwrap();

        assert!(logd_pos < netmd_pos, "logd must start before netmd");
        assert!(stardust_pos < netmd_pos, "stardust must start before netmd");
    }

    /// Test: Manifest parsing with health_check field.
    #[test]
    fn test_manifest_health_check_field() {
        let yaml = r#"
name: luna-httpd
binary: /usr/bin/lunad
health_check: tcp:80
"#;
        let manifest: monitord::manifest::ServiceManifest =
            serde_yaml::from_str(yaml).expect("Parse failed");
        assert_eq!(manifest.health_check, Some("tcp:80".to_string()));
    }

    /// Test: Invalid YAML is rejected.
    #[test]
    fn test_invalid_manifest_rejected() {
        let yaml = "not: valid: yaml: [unclosed";
        let result: Result<monitord::manifest::ServiceManifest, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Invalid YAML should fail parsing");
    }

    /// Test: Dependency cycle detection.
    #[test]
    fn test_cycle_detection() {
        use monitord::graph::DependencyGraph;
        use monitord::manifest::{RestartPolicy, ServiceManifest};

        let svc = |name: &str, deps: &[&str]| ServiceManifest {
            name: name.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            binary: "/bin/svc".into(),
            args: vec![],
            env: vec![],
            description: "".into(),
            after: vec![],
            restart: RestartPolicy::Always,
            restart_delay_ms: 1000,
            max_crash_count: 3,
            crash_window_secs: 30,
            critical: false,
            startup_timeout_secs: 5,
            health_check: None,
            caps: Default::default(),
        };

        let result = DependencyGraph::new(&[
            svc("A", &["B"]),
            svc("B", &["A"]), // Cycle: B→A→B
        ]);
        assert!(result.is_err(), "Cycle should be detected");
    }

    /// Test: Missing required fields rejected.
    #[test]
    fn test_missing_required_fields() {
        let yaml = "description: no name or binary\nrestart: always\n";
        let result: Result<monitord::manifest::ServiceManifest, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Missing 'name' and 'binary' should fail");
    }

    /// Test: Message param accumulation (regression).
    #[test]
    fn test_message_param_accumulation() {
        let msg = stardust::Message::new("topic")
            .src("test".to_string())
            .param("key1", &"val1")
            .unwrap()
            .param("key2", &"val2")
            .unwrap();
        let s = std::str::from_utf8(&msg.params).unwrap();
        assert!(s.contains("key1") && s.contains("val1"), "key1 present");
        assert!(s.contains("key2") && s.contains("val2"), "key2 present");
    }

    /// Test: Multiple message pub/sub stream.
    #[tokio::test]
    #[ignore = "requires timing-sensitive IPC — run on Linux target"]
    async fn test_multiple_message_stream() {
        let socket = "/tmp/test-uos-stream.sock";
        let _ = std::fs::remove_file(socket);

        let broker = stardust::Broker::new(socket.to_string());
        let broker_handle = tokio::spawn(async move {
            broker.run().await.ok();
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let sub = stardust::Client::connect(socket).await.unwrap();
        let mut rx = sub.subscribe("multi").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let pub_client = stardust::Client::connect(socket).await.unwrap();
        pub_client.register("multi-pub").await.ok();

        for i in 0u32..5 {
            let msg = stardust::Message::new("multi")
                .src("multi-pub".to_string())
                .param("seq", &i)
                .unwrap();
            pub_client.publish(msg).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let mut count = 0;
        for _ in 0..5 {
            let r = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
            if let Ok(Some(_)) = r {
                count += 1;
            }
        }
        assert_eq!(count, 5, "Should receive all 5 messages");
        broker_handle.abort();
    }
}
