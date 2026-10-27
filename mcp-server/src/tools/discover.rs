//! # Discover Resources Tool
//!
//! MCP tool that searches the local resource registry for paid resources
//! matching a query string. Performs case-insensitive substring matching
//! against resource names and descriptions.

use serde::{Deserialize, Serialize};

/// A paid resource entry from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub contract_id: String,
    pub method: String,
    pub price: u64,
    pub description: String,
    #[serde(default)]
    pub fee_stroops: u64,
    #[serde(default)]
    pub token: String,
}

/// Load resources from the embedded registry JSON.
pub fn load_registry() -> Vec<Resource> {
    let registry_json = include_str!("../../../registry/resources.json");
    serde_json::from_str(registry_json).unwrap_or_default()
}

/// Search resources by query (case-insensitive substring match).
pub fn search_resources(query: &str) -> Vec<Resource> {
    let resources = load_registry();
    let query_lower = query.to_lowercase();
    if query_lower.is_empty() {
        return resources;
    }

    let mut ranked: Vec<(i32, Resource)> = resources
        .into_iter()
        .filter_map(|r| {
            let mut score = 0;
            if r.id.to_lowercase().contains(&query_lower) {
                score += 3;
            }
            if r.name.to_lowercase().contains(&query_lower) {
                score += 2;
            }
            if r.description.to_lowercase().contains(&query_lower) {
                score += 1;
            }
            (score > 0).then_some((score, r))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    ranked.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_registry() {
        let resources = load_registry();
        assert!(
            !resources.is_empty(),
            "Registry should have at least one resource"
        );
    }

    #[test]
    fn test_search_resources() {
        let results = search_resources("weather");
        assert!(!results.is_empty(), "Should find weather resource");
    }

    #[test]
    fn test_search_ranks_id_matches_first() {
        let results = search_resources("weather");
        assert_eq!(results[0].id, "weather-oracle");
    }

    #[test]
    fn test_empty_query_returns_all() {
        let results = search_resources("");
        assert_eq!(results.len(), load_registry().len());
    }

    #[test]
    fn test_search_no_match() {
        let results = search_resources("nonexistent_resource_xyz");
        assert!(results.is_empty(), "Should find no resources");
    }
}
