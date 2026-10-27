//! Local agent budget snapshot used by MCP tools until live RPC is wired.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayeeRule {
    pub asset: String,
    pub pay_to: String,
    #[serde(default)]
    pub rule_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub remaining_stroops: i128,
    #[serde(default)]
    pub remaining_xlm: f64,
    #[serde(default)]
    pub period_ledgers: u32,
    #[serde(default)]
    pub rule_count: u32,
    #[serde(default)]
    pub allowed_payees: Option<Vec<PayeeRule>>,
}

impl Default for BudgetStatus {
    fn default() -> Self {
        Self {
            remaining_stroops: 10_000_000,
            remaining_xlm: 1.0,
            period_ledgers: 17_280,
            rule_count: 3,
            allowed_payees: None,
        }
    }
}

pub fn load_budget() -> BudgetStatus {
    match std::env::var("AGENTPAY_STATE") {
        Ok(path) => load_budget_from_path(Path::new(&path)),
        Err(_) => BudgetStatus::default(),
    }
}

pub fn load_budget_from_path(path: &Path) -> BudgetStatus {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BudgetStatus::default();
    };
    let mut status: BudgetStatus = serde_json::from_str(&raw).unwrap_or_default();
    status.remaining_xlm = status.remaining_stroops as f64 / 10_000_000.0;
    status
}
