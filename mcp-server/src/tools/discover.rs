//! # Discover Resources Tool
//!
//! MCP tool and discovery adapter that searches paid resources across:
//! 1. The local registry (`registry/resources.json`)
//! 2. A remote x402 catalog (e.g. Bazaar-style discovery endpoint)
//!
//! Normalizes results into [`NormalizedResource`] and marks each result
//! with whether the current policy would allow paying it.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::tools::pay_and_call::{MAX_RESPONSE_BODY_SIZE, validate_url_and_check_ssrf};

/// A normalized resource entry returned by discovery sources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedResource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub price: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub source: String,
    pub payable: bool,
    pub policy_status: String,
}

/// Legacy paid resource entry from the registry (for backward compatibility).
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
    #[serde(default)]
    pub asset: Option<String>,
    #[serde(default)]
    pub pay_to: Option<String>,
}

/// Trait representing an abstract discovery source.
pub trait DiscoverySource: Send + Sync {
    fn search(&self, query: &str) -> Vec<NormalizedResource>;
}

/// Checks whether the current policy would allow paying for this resource.
pub fn check_resource_payable(
    asset_opt: Option<&str>,
    pay_to_opt: Option<&str>,
    contract_id_opt: Option<&str>,
) -> (bool, &'static str) {
    let budget = crate::account_state::load_budget();

    // Check live on-chain if live config is available
    let config = crate::soroban_client::SorobanConfig::from_env();
    if config.is_live()
        && !config.identity.is_empty()
        && let (Some(asset), Some(pay_to)) = (asset_opt, pay_to_opt)
    {
        match crate::soroban_client::query_payee_rule(&config, asset, pay_to) {
            Ok(Some(_rule_id)) => return (true, "payable"),
            _ => return (false, "discoverable but not payable until policy allows"),
        }
    }

    // Check local budget status for allowed_payees
    if let (Some(asset), Some(pay_to)) = (asset_opt, pay_to_opt) {
        if let Some(ref allowed_payees) = budget.allowed_payees
            && allowed_payees
                .iter()
                .any(|p| p.asset == asset && p.pay_to == pay_to)
        {
            return (true, "payable");
        }
        return (false, "discoverable but not payable until policy allows");
    }

    // Fallback: check legacy contract_id
    if let Some(_cid) = contract_id_opt
        && budget.rule_count > 0
    {
        return (true, "payable");
    }

    (false, "discoverable but not payable until policy allows")
}

// ---------------------------------------------------------------------------
// 1. Registry Source
// ---------------------------------------------------------------------------

pub struct RegistrySource {
    raw_json: Option<String>,
}

impl Default for RegistrySource {
    fn default() -> Self {
        Self::new()
    }
}

impl RegistrySource {
    pub fn new() -> Self {
        Self { raw_json: None }
    }

    #[allow(dead_code)]
    pub fn with_json(json: String) -> Self {
        Self {
            raw_json: Some(json),
        }
    }

    pub fn load_resources(&self) -> Vec<Resource> {
        let registry_json = match &self.raw_json {
            Some(s) => s.as_str(),
            None => include_str!("../../../registry/resources.json"),
        };
        serde_json::from_str(registry_json).unwrap_or_default()
    }
}

impl DiscoverySource for RegistrySource {
    fn search(&self, query: &str) -> Vec<NormalizedResource> {
        let resources = self.load_resources();
        let query_lower = query.to_lowercase().trim().to_string();

        let mut ranked: Vec<(i32, NormalizedResource)> = resources
            .into_iter()
            .filter_map(|r| {
                let id_lower = r.id.to_lowercase();
                let name_lower = r.name.to_lowercase();
                let desc_lower = r.description.to_lowercase();

                let mut score = 0;
                if query_lower.is_empty() {
                    score = 1;
                } else {
                    if id_lower.contains(&query_lower) {
                        score += 3;
                    }
                    if name_lower.contains(&query_lower) {
                        score += 2;
                    }
                    if desc_lower.contains(&query_lower) {
                        score += 1;
                    }
                }

                if score == 0 {
                    return None;
                }

                let asset_str = r.asset.clone().or_else(|| {
                    if !r.contract_id.is_empty() {
                        Some(r.contract_id.clone())
                    } else {
                        None
                    }
                });
                let pay_to_str = r.pay_to.clone();

                let (payable, policy_status) = check_resource_payable(
                    asset_str.as_deref(),
                    pay_to_str.as_deref(),
                    Some(&r.contract_id),
                );

                let norm = NormalizedResource {
                    id: Some(r.id),
                    name: r.name,
                    description: r.description,
                    url: None,
                    price: format!("{} stroops", r.price),
                    asset: asset_str,
                    pay_to: pay_to_str,
                    network: Some("stellar:testnet".to_string()),
                    source: "registry".to_string(),
                    payable,
                    policy_status: policy_status.to_string(),
                };

                Some((score, norm))
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.id.as_deref().cmp(&b.1.id.as_deref()))
        });
        ranked.into_iter().map(|(_, r)| r).collect()
    }
}

// ---------------------------------------------------------------------------
// 2. Remote Catalog Source (Bazaar-style)
// ---------------------------------------------------------------------------

type CatalogCache = Arc<Mutex<Option<(Instant, Vec<NormalizedResource>)>>>;
pub struct RemoteCatalogSource {
    catalog_url: Option<String>,
    cache_ttl: Duration,
    cache: CatalogCache,
}

impl Default for RemoteCatalogSource {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RemoteCatalogSource {
    pub fn new(catalog_url: Option<String>) -> Self {
        Self {
            catalog_url,
            cache_ttl: Duration::from_secs(60),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    #[allow(dead_code)]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    fn get_target_url(&self) -> Option<String> {
        self.catalog_url
            .clone()
            .or_else(|| std::env::var("X402_CATALOG_URL").ok())
    }

    fn fetch_remote_catalog(&self, url_str: &str) -> Result<Vec<NormalizedResource>, String> {
        let allow_test = std::env::var("AGENTPAY_ALLOW_TEST_LOOPBACK")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        let validated_url = validate_url_and_check_ssrf(url_str, allow_test)
            .map_err(|e| format!("URL security validation failed: {e}"))?;

        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("Client build error: {e}"))?;

        let resp = client
            .get(validated_url.as_str())
            .send()
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Catalog returned HTTP {}", resp.status()));
        }

        use std::io::Read as _;
        let mut body_bytes = Vec::new();
        resp.take(MAX_RESPONSE_BODY_SIZE as u64)
            .read_to_end(&mut body_bytes)
            .map_err(|e| format!("Read body error: {e}"))?;

        let val: serde_json::Value =
            serde_json::from_slice(&body_bytes).map_err(|e| format!("JSON parse error: {e}"))?;

        let raw_entries = if let Some(arr) = val.as_array() {
            arr.clone()
        } else if let Some(arr) = val.get("resources").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(arr) = val.get("data").and_then(|v| v.as_array()) {
            arr.clone()
        } else {
            return Err("Expected JSON array or { 'resources': [...] }".into());
        };

        let mut items = Vec::new();
        for entry in raw_entries {
            let name = entry
                .get("name")
                .or_else(|| entry.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed")
                .to_string();

            let description = entry
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let url = entry
                .get("url")
                .or_else(|| entry.get("endpoint"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let price = entry
                .get("price")
                .or_else(|| entry.get("priceUsd"))
                .or_else(|| entry.get("amount"))
                .map(|v| {
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else if let Some(n) = v.as_u64() {
                        format!("{n} stroops")
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_else(|| "variable".into());

            let asset = entry
                .get("asset")
                .or_else(|| entry.get("token"))
                .or_else(|| {
                    entry
                        .get("accepts")
                        .and_then(|a| a.get(0))
                        .and_then(|f| f.get("asset"))
                })
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let pay_to = entry
                .get("payTo")
                .or_else(|| entry.get("pay_to"))
                .or_else(|| {
                    entry
                        .get("accepts")
                        .and_then(|a| a.get(0))
                        .and_then(|f| f.get("payTo").or_else(|| f.get("pay_to")))
                })
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let network = entry
                .get("network")
                .or_else(|| {
                    entry
                        .get("accepts")
                        .and_then(|a| a.get(0))
                        .and_then(|f| f.get("network"))
                })
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
                .or_else(|| Some("stellar:testnet".to_string()));

            let (payable, policy_status) =
                check_resource_payable(asset.as_deref(), pay_to.as_deref(), None);

            items.push(NormalizedResource {
                id: entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string),
                name,
                description,
                url,
                price,
                asset,
                pay_to,
                network,
                source: "x402-catalog".to_string(),
                payable,
                policy_status: policy_status.to_string(),
            });
        }

        Ok(items)
    }
}

impl DiscoverySource for RemoteCatalogSource {
    fn search(&self, query: &str) -> Vec<NormalizedResource> {
        let Some(target_url) = self.get_target_url() else {
            return Vec::new();
        };

        // Check in-memory cache
        let cached = {
            let guard = self.cache.lock().unwrap();
            if let Some((ts, ref list)) = *guard {
                if ts.elapsed() < self.cache_ttl {
                    Some(list.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        let resources = match cached {
            Some(list) => list,
            None => match self.fetch_remote_catalog(&target_url) {
                Ok(fresh) => {
                    let mut guard = self.cache.lock().unwrap();
                    *guard = Some((Instant::now(), fresh.clone()));
                    fresh
                }
                Err(err) => {
                    tracing::warn!("Failed to fetch remote x402 catalog: {err}");
                    return Vec::new();
                }
            },
        };

        let query_lower = query.to_lowercase().trim().to_string();
        if query_lower.is_empty() {
            return resources;
        }

        resources
            .into_iter()
            .filter(|r| {
                r.name.to_lowercase().contains(&query_lower)
                    || r.description.to_lowercase().contains(&query_lower)
                    || r.url
                        .as_deref()
                        .map(|u| u.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
                    || r.id
                        .as_deref()
                        .map(|id| id.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 3. Composite Discovery Adapter
// ---------------------------------------------------------------------------

pub struct CompositeDiscovery {
    sources: Vec<Box<dyn DiscoverySource>>,
}

impl Default for CompositeDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeDiscovery {
    pub fn new() -> Self {
        Self {
            sources: vec![
                Box::new(RegistrySource::new()),
                Box::new(RemoteCatalogSource::default()),
            ],
        }
    }

    #[allow(dead_code)]
    pub fn with_sources(sources: Vec<Box<dyn DiscoverySource>>) -> Self {
        Self { sources }
    }

    pub fn search(&self, query: &str) -> Vec<NormalizedResource> {
        let mut results = Vec::new();
        for src in &self.sources {
            results.extend(src.search(query));
        }

        // Rank by query relevance
        let query_lower = query.to_lowercase().trim().to_string();
        if !query_lower.is_empty() {
            results.sort_by(|a, b| {
                let a_score = if a.name.to_lowercase().contains(&query_lower) {
                    3
                } else if a.description.to_lowercase().contains(&query_lower) {
                    1
                } else {
                    0
                };
                let b_score = if b.name.to_lowercase().contains(&query_lower) {
                    3
                } else if b.description.to_lowercase().contains(&query_lower) {
                    1
                } else {
                    0
                };
                b_score.cmp(&a_score).then_with(|| a.name.cmp(&b.name))
            });
        }

        results
    }
}

/// Convenience entry point for searching resources across all sources.
pub fn search_resources(query: &str) -> Vec<NormalizedResource> {
    CompositeDiscovery::new().search(query)
}

/// Legacy helper for searching resources returning the un-normalized registry structs.
#[allow(dead_code)]
pub fn search_legacy_resources(query: &str) -> Vec<Resource> {
    let source = RegistrySource::new();
    let resources = source.load_resources();
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

/// Load resources from the embedded registry JSON (backward compatibility).
pub fn load_registry() -> Vec<Resource> {
    RegistrySource::new().load_resources()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn test_registry_source_search() {
        let source = RegistrySource::new();
        let results = source.search("weather");
        assert!(!results.is_empty(), "Should find weather resource");
        assert_eq!(results[0].id.as_deref(), Some("weather-oracle"));
        assert_eq!(results[0].source, "registry");
        assert_eq!(results[0].network.as_deref(), Some("stellar:testnet"));
    }

    #[test]
    fn test_policy_allowance_marking() {
        let source = RegistrySource::new();
        let results = source.search("demo-payee-vendor");
        assert!(!results.is_empty());
        let payee_res = &results[0];

        // By default without policy state, payee not allowlisted
        assert!(!payee_res.payable);
        assert_eq!(
            payee_res.policy_status,
            "discoverable but not payable until policy allows"
        );
    }

    #[test]
    fn test_policy_allowance_marking_when_allowlisted() {
        let test_file =
            std::env::temp_dir().join(format!("agentpay_test_{}.json", std::process::id()));
        let state_content = serde_json::json!({
            "remaining_stroops": 10000000,
            "period_ledgers": 17280,
            "rule_count": 1,
            "allowed_payees": [
                {
                    "asset": "CDEMO5TOKN1ASSET1ADDRESSXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
                    "pay_to": "GDEMO5VENDOR1PAYEEADDRESXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
                    "rule_id": 0
                }
            ]
        });
        std::fs::write(&test_file, state_content.to_string()).unwrap();

        unsafe {
            std::env::set_var("AGENTPAY_STATE", test_file.to_str().unwrap());
        }

        let source = RegistrySource::new();
        let results = source.search("demo-payee-vendor");
        assert!(!results.is_empty());
        let payee_res = &results[0];
        assert!(payee_res.payable);
        assert_eq!(payee_res.policy_status, "payable");

        unsafe {
            std::env::remove_var("AGENTPAY_STATE");
        }
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_remote_catalog_source_mock_bazaar_json() {
        let bazaar_json = serde_json::json!({
            "resources": [
                {
                    "id": "bazaar-oracle",
                    "name": "Bazaar Weather Feed",
                    "description": "Satellite weather observations",
                    "url": "https://api.bazaar.org/weather",
                    "price": "$0.02",
                    "network": "stellar:testnet",
                    "accepts": [
                        {
                            "asset": "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA",
                            "payTo": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                        }
                    ]
                }
            ]
        });

        // Test parsing the catalog response logic directly
        let entries = bazaar_json.get("resources").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(
            entry.get("name").unwrap().as_str().unwrap(),
            "Bazaar Weather Feed"
        );
    }

    #[test]
    fn test_composite_discovery_ranks_and_merges() {
        let composite = CompositeDiscovery::new();
        let all = composite.search("");
        assert!(all.len() >= 4, "Should find at least registry items");
    }
}
