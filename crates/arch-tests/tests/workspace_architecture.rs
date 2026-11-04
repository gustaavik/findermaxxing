//! Runs the dependency rules against the real workspace.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::path::Path;
use std::process::Command;

use arch_tests::DependencyGraph;

#[test]
fn workspace_respects_layering_and_purity_rules() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("cargo metadata should run");
    assert!(output.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata is JSON");
    let graph = DependencyGraph::from_metadata(&metadata).expect("metadata has a resolve graph");

    let violations = graph.violations();
    assert!(violations.is_empty(), "architecture violations:\n  {}", violations.join("\n  "));
}
