//! Dependency-rule checker over `cargo metadata` (ADR 0001).
//!
//! Two rule kinds:
//! - **layering**: which workspace crates a crate may depend on (inward only);
//! - **purity**: inner layers may not reach infrastructure crates, even transitively.
//!
//! Only normal (non-dev, non-build) dependency edges count: tests may use fakes and runtimes.

use std::collections::{BTreeSet, HashMap, VecDeque};

/// Infrastructure crates the inner layers must never depend on. A trailing
/// `*` matches a prefix (`aws-*` covers every AWS SDK crate).
pub const FORBIDDEN_IN_CORE: &[&str] =
    &["cxx", "cxx-qt*", "qt-*", "tokio*", "rusqlite", "aws-*", "opendal", "notify", "ort", "reqwest", "hyper"];

/// Crates that must stay free of infrastructure.
pub const CORE_CRATES: &[&str] = &["domain", "application"];

/// Allowed workspace dependencies per workspace crate. Crates not listed here
/// (adapters matched by prefix aside) may not depend on any workspace crate.
pub const LAYER_RULES: &[(&str, &[&str])] = &[
    ("domain", &[]),
    ("application", &["domain"]),
    ("vfs-contract-tests", &["domain", "application"]),
    ("adapter-*", &["domain", "application"]),
    ("presentation", &["domain", "application"]),
    ("app", &["*"]),
];

/// Normal-dependency graph keyed by crate name.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    edges: HashMap<String, BTreeSet<String>>,
    workspace: BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("cargo metadata output has no `{0}`")]
    Missing(&'static str),
}

impl DependencyGraph {
    #[must_use]
    pub fn new(workspace: impl IntoIterator<Item = String>) -> Self {
        Self { edges: HashMap::new(), workspace: workspace.into_iter().collect() }
    }

    pub fn add_edge(&mut self, from: &str, to: &str) {
        self.edges.entry(from.to_owned()).or_default().insert(to.to_owned());
    }

    /// Builds the graph from `cargo metadata --format-version 1` output.
    ///
    /// # Errors
    /// [`MetadataError::Missing`] when expected fields are absent.
    pub fn from_metadata(metadata: &serde_json::Value) -> Result<Self, MetadataError> {
        let names = package_names(metadata)?;
        let workspace = metadata["workspace_members"]
            .as_array()
            .ok_or(MetadataError::Missing("workspace_members"))?
            .iter()
            .filter_map(|id| id.as_str().and_then(|id| names.get(id)).cloned());
        let mut graph = Self::new(workspace);
        let nodes = metadata["resolve"]["nodes"].as_array().ok_or(MetadataError::Missing("resolve.nodes"))?;
        for node in nodes {
            let Some(from) = node["id"].as_str().and_then(|id| names.get(id)) else { continue };
            for dep in node["deps"].as_array().into_iter().flatten().filter(|dep| is_normal_edge(dep)) {
                if let Some(to) = dep["pkg"].as_str().and_then(|id| names.get(id)) {
                    graph.add_edge(from, to);
                }
            }
        }
        Ok(graph)
    }

    /// Every crate reachable from `start` through normal edges, excluding `start`.
    #[must_use]
    pub fn transitive_dependencies(&self, start: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([start]);
        while let Some(current) = queue.pop_front() {
            for next in self.edges.get(current).into_iter().flatten() {
                if seen.insert(next.clone()) {
                    queue.push_back(next);
                }
            }
        }
        seen
    }

    /// Human-readable rule violations; empty means the architecture holds.
    #[must_use]
    pub fn violations(&self) -> Vec<String> {
        let mut found = self.purity_violations();
        found.extend(self.layering_violations());
        found
    }

    fn purity_violations(&self) -> Vec<String> {
        CORE_CRATES
            .iter()
            .flat_map(|core| {
                self.transitive_dependencies(core)
                    .into_iter()
                    .filter(|dep| FORBIDDEN_IN_CORE.iter().any(|pattern| matches_pattern(pattern, dep)))
                    .map(move |dep| format!("`{core}` (core layer) transitively depends on forbidden crate `{dep}`"))
            })
            .collect()
    }

    fn layering_violations(&self) -> Vec<String> {
        self.workspace
            .iter()
            .flat_map(|krate| {
                let allowed = allowed_workspace_deps(krate);
                self.edges
                    .get(krate)
                    .into_iter()
                    .flatten()
                    .filter(|dep| self.workspace.contains(*dep))
                    .filter(move |dep| !allowed.iter().any(|pattern| matches_pattern(pattern, dep)))
                    .map(move |dep| {
                        format!("`{krate}` must not depend on workspace crate `{dep}` (layers point inward)")
                    })
            })
            .collect()
    }
}

fn package_names(metadata: &serde_json::Value) -> Result<HashMap<String, String>, MetadataError> {
    let packages = metadata["packages"].as_array().ok_or(MetadataError::Missing("packages"))?;
    Ok(packages.iter().filter_map(|p| Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned()))).collect())
}

fn is_normal_edge(dep: &serde_json::Value) -> bool {
    dep["dep_kinds"].as_array().into_iter().flatten().any(|kind| kind["kind"].is_null())
}

fn allowed_workspace_deps(krate: &str) -> &'static [&'static str] {
    LAYER_RULES.iter().find(|(pattern, _)| matches_pattern(pattern, krate)).map_or(&[], |(_, allowed)| allowed)
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &str)]) -> DependencyGraph {
        let workspace = ["domain", "application", "adapter-local-fs", "presentation", "app"].map(String::from);
        let mut graph = DependencyGraph::new(workspace);
        for (from, to) in edges {
            graph.add_edge(from, to);
        }
        graph
    }

    #[test]
    fn clean_graph_has_no_violations() {
        let g = graph(&[
            ("application", "domain"),
            ("application", "futures"),
            ("adapter-local-fs", "application"),
            ("adapter-local-fs", "tokio"),
            ("presentation", "application"),
            ("app", "adapter-local-fs"),
            ("app", "presentation"),
        ]);
        assert_eq!(g.violations(), Vec::<String>::new());
    }

    #[test]
    fn transitive_runtime_dependency_in_core_is_caught() {
        let g = graph(&[("application", "futures"), ("futures", "tokio-util")]);
        assert_eq!(g.violations().len(), 1);
        assert!(g.violations()[0].contains("tokio-util"));
    }

    #[test]
    fn outward_workspace_dependency_is_caught() {
        let g = graph(&[("presentation", "adapter-local-fs"), ("domain", "application")]);
        let violations = g.violations();
        assert_eq!(violations.len(), 2, "{violations:?}");
    }
}
