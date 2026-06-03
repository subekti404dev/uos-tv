//! Dependency Graph untuk service startup ordering.
//!
//! Membangun directed acyclic graph (DAG) dari service manifests,
//! lalu melakukan topological sort untuk menentukan urutan startup.

use crate::manifest::ServiceManifest;
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GraphError {
    #[error("Circular dependency detected involving: {0:?}")]
    CircularDependency(Vec<String>),

    #[error("Missing dependency: service '{service}' requires '{dep}' which does not exist")]
    MissingDependency { service: String, dep: String },
}

/// Dependency graph untuk services.
pub struct DependencyGraph {
    /// Semua services indexed by name
    services: HashMap<String, ServiceManifest>,

    /// Adjacency list: service_name → set of services that depend ON it
    /// (ini adalah reverse dependency — "siapa yang butuh saya")
    dependents: HashMap<String, HashSet<String>>,

    /// Adjacency list: service_name → dependencies (services that must start first)
    dependencies: HashMap<String, HashSet<String>>,
}

impl DependencyGraph {
    /// Bangun graph dari daftar service manifests.
    pub fn new(services: &[ServiceManifest]) -> Result<Self, GraphError> {
        let service_map: HashMap<String, ServiceManifest> = services
            .iter()
            .map(|s| (s.name.clone(), s.clone()))
            .collect();

        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        let mut dependents: HashMap<String, HashSet<String>> = HashMap::new();

        // Inisialisasi
        for svc in services {
            deps.entry(svc.name.clone()).or_default();
            dependents.entry(svc.name.clone()).or_default();
        }

        // Bangun edges
        for svc in services {
            for dep_name in &svc.dependencies {
                // Validasi: dependency harus ada
                if !service_map.contains_key(dep_name) {
                    return Err(GraphError::MissingDependency {
                        service: svc.name.clone(),
                        dep: dep_name.clone(),
                    });
                }

                // service → dep (service butuh dep)
                deps.entry(svc.name.clone())
                    .or_default()
                    .insert(dep_name.clone());

                // dep → service (dep dibutuhkan oleh service)
                dependents
                    .entry(dep_name.clone())
                    .or_default()
                    .insert(svc.name.clone());
            }
        }

        let graph = Self {
            services: service_map,
            dependents,
            dependencies: deps,
        };

        // Verifikasi tidak ada circular dependency
        graph.verify_no_cycles()?;

        Ok(graph)
    }

    /// Verifikasi tidak ada circular dependency via DFS cycle detection.
    fn verify_no_cycles(&self) -> Result<(), GraphError> {
        // DFS dengan warna: 0 = unvisited, 1 = in-progress, 2 = done
        let mut color: HashMap<&str, u8> = self.services.keys().map(|k| (k.as_str(), 0)).collect();

        // Untuk tracking cycle
        let mut path: Vec<String> = Vec::new();

        for name in self.services.keys() {
            if color[name.as_str()] == 0 {
                if let Err(cycle) = self.dfs_cycle_detect(name, &mut color, &mut path) {
                    return Err(cycle);
                }
            }
        }

        Ok(())
    }

    fn dfs_cycle_detect<'a>(
        &'a self,
        node: &'a str,
        color: &mut HashMap<&'a str, u8>,
        path: &mut Vec<String>,
    ) -> Result<(), GraphError> {
        color.insert(node, 1); // in-progress
        path.push(node.to_string());

        if let Some(dep_set) = self.dependencies.get(node) {
            for dep in dep_set {
                let dep_color = color[dep.as_str()];
                if dep_color == 1 {
                    // Cycle detected!
                    // Extract cycle path
                    let cycle_start = path.iter().position(|n| n == dep).unwrap();
                    let cycle: Vec<String> = path[cycle_start..].to_vec();
                    return Err(GraphError::CircularDependency(cycle));
                }
                if dep_color == 0 {
                    self.dfs_cycle_detect(dep, color, path)?;
                }
            }
        }

        color.insert(node, 2); // done
        path.pop();
        Ok(())
    }

    /// Topological sort — menghasilkan urutan startup.
    /// Service tanpa dependency → paling awal.
    /// Service dengan dependency → setelah dependency selesai.
    ///
    /// Menggunakan Kahn's algorithm (BFS-based).
    pub fn topological_sort(&self) -> Vec<&ServiceManifest> {
        let mut result = Vec::new();

        // Hitung in-degree (jumlah dependency yang belum terpenuhi)
        let mut in_degree: HashMap<&str, usize> = self
            .services
            .keys()
            .map(|k| (k.as_str(), self.dependencies.get(k).map_or(0, |d| d.len())))
            .collect();

        // Queue: service dengan in-degree 0
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&name, _)| name)
            .collect();

        while let Some(name) = queue.pop_front() {
            result.push(&self.services[name]);

            // Kurangi in-degree untuk semua dependents
            if let Some(dep_set) = self.dependents.get(name) {
                for dependent in dep_set {
                    let deg = in_degree.get_mut(dependent.as_str()).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }

        // Jika result.len() < services.len(), ada cycle (tapi seharusnya sudah dicek)
        if result.len() != self.services.len() {
            tracing::error!(
                "Topological sort incomplete: {} / {} services sorted",
                result.len(),
                self.services.len()
            );
        }

        result
    }

    /// Get service by name.
    pub fn get(&self, name: &str) -> Option<&ServiceManifest> {
        self.services.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort() {
        let services = vec![
            ServiceManifest {
                name: "a".into(),
                dependencies: vec![],
                ..default_manifest("a", "/bin/a")
            },
            ServiceManifest {
                name: "b".into(),
                dependencies: vec!["a".into()],
                ..default_manifest("b", "/bin/b")
            },
            ServiceManifest {
                name: "c".into(),
                dependencies: vec!["a".into(), "b".into()],
                ..default_manifest("c", "/bin/c")
            },
            ServiceManifest {
                name: "d".into(),
                dependencies: vec!["c".into()],
                ..default_manifest("d", "/bin/d")
            },
        ];

        let graph = DependencyGraph::new(&services).unwrap();
        let order: Vec<&str> = graph
            .topological_sort()
            .iter()
            .map(|s| s.name.as_str())
            .collect();

        // a harus sebelum b, b sebelum c, c sebelum d
        let pos_a = order.iter().position(|&n| n == "a").unwrap();
        let pos_b = order.iter().position(|&n| n == "b").unwrap();
        let pos_c = order.iter().position(|&n| n == "c").unwrap();
        let pos_d = order.iter().position(|&n| n == "d").unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn test_circular_dependency_detection() {
        let services = vec![
            ServiceManifest {
                name: "a".into(),
                dependencies: vec!["b".into()],
                ..default_manifest("a", "/bin/a")
            },
            ServiceManifest {
                name: "b".into(),
                dependencies: vec!["a".into()],
                ..default_manifest("b", "/bin/b")
            },
        ];

        let result = DependencyGraph::new(&services);
        assert!(result.is_err());

        if let Err(GraphError::CircularDependency(cycle)) = result {
            assert!(cycle.contains(&"a".to_string()));
            assert!(cycle.contains(&"b".to_string()));
        } else {
            panic!("Expected CircularDependency error");
        }
    }

    #[test]
    fn test_missing_dependency() {
        let services = vec![ServiceManifest {
            name: "a".into(),
            dependencies: vec!["nonexistent".into()],
            ..default_manifest("a", "/bin/a")
        }];

        let result = DependencyGraph::new(&services);
        assert!(result.is_err());
    }

    fn default_manifest(name: &str, binary: &str) -> ServiceManifest {
        ServiceManifest {
            name: name.into(),
            description: String::new(),
            binary: binary.into(),
            args: vec![],
            env: vec![],
            dependencies: vec![],
            after: vec![],
            restart: crate::manifest::RestartPolicy::OnFailure,
            restart_delay_ms: 1000,
            max_crash_count: 3,
            crash_window_secs: 30,
            critical: false,
            startup_timeout_secs: 15,
            health_check: None,
            caps: crate::manifest::SecurityCapabilities::default(),
        }
    }
}
