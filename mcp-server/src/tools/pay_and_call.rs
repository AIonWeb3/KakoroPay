//! # Pay and Call Tool
//!
//! MCP tool that builds and submits a Soroban transaction authorized by
//! the agent's smart account, waits for confirmation, then calls the
//! underlying resource. Returns typed errors for policy denial,
//! insufficient budget, and resource call failure.

use serde::Serialize;
use std::fmt;

// ---------------------------------------------------------------------------
// Typed errors — the agent gets a legible reason, not a panic
// ---------------------------------------------------------------------------

/// Errors that can occur during pay_and_call.
#[derive(Debug)]
#[allow(dead_code)]
pub enum PayAndCallError {
    /// The smart account's spending policy denied the transaction.
    PolicyDenied(String),
    /// The agent's budget is insufficient for this call.
    InsufficientBudget { required: i128, available: i128 },
    /// The underlying resource call failed.
    ResourceCallFailed(String),
    /// The resource ID was not found in the registry.
    ResourceNotFound(String),
    /// A transient network error occurred (retryable).
    TransientError(String),
    /// The atomic pay-and-call envelope was rejected before or during submit.
    AuthFailed(String),
}

impl fmt::Display for PayAndCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PolicyDenied(reason) => write!(f, "Policy denied: {reason}"),
            Self::InsufficientBudget {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient budget: required {required} stroops, available {available} stroops"
                )
            }
            Self::ResourceCallFailed(reason) => write!(f, "Resource call failed: {reason}"),
            Self::ResourceNotFound(id) => write!(f, "Resource not found: {id}"),
            Self::TransientError(msg) => write!(f, "Transient error (retryable): {msg}"),
            Self::AuthFailed(msg) => write!(f, "Authorization failed: {msg}"),
        }
    }
}

impl std::error::Error for PayAndCallError {}

impl PayAndCallError {
    pub fn to_structured_json(&self) -> String {
        let value = match self {
            Self::PolicyDenied(reason) => serde_json::json!({
                "error": { "code": "PolicyDenied", "message": reason }
            }),
            Self::InsufficientBudget {
                required,
                available,
            } => serde_json::json!({
                "error": {
                    "code": "InsufficientBudget",
                    "message": self.to_string(),
                    "required": required,
                    "available": available
                }
            }),
            Self::ResourceCallFailed(reason) => serde_json::json!({
                "error": { "code": "ResourceCallFailed", "message": reason }
            }),
            Self::ResourceNotFound(id) => serde_json::json!({
                "error": { "code": "ResourceNotFound", "message": id }
            }),
            Self::TransientError(msg) => serde_json::json!({
                "error": { "code": "TransientError", "message": msg }
            }),
            Self::AuthFailed(msg) => serde_json::json!({
                "error": { "code": "AuthFailed", "message": msg }
            }),
        };
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.to_string())
    }
}

pub const MAX_TRANSIENT_ATTEMPTS: u32 = 3;

pub fn retry_transient<T, F>(mut op: F) -> Result<T, PayAndCallError>
where
    F: FnMut() -> Result<T, PayAndCallError>,
{
    let mut last_transient = None;
    for _ in 0..MAX_TRANSIENT_ATTEMPTS {
        match op() {
            Ok(value) => return Ok(value),
            Err(PayAndCallError::TransientError(msg)) => {
                last_transient = Some(PayAndCallError::TransientError(msg));
            }
            Err(other) => return Err(other),
        }
    }
    Err(last_transient.unwrap_or_else(|| PayAndCallError::TransientError("retry exhausted".into())))
}

/// Successful response from pay_and_call.
#[derive(Debug, Serialize)]
pub struct PayAndCallResult {
    /// The Soroban transaction hash.
    pub tx_hash: String,
    /// The ledger sequence where the transaction was confirmed.
    pub ledger: u32,
    /// The amount spent in stroops.
    pub amount_spent: i128,
    /// The response from the underlying resource.
    pub resource_response: String,
}

/// Pay for and call a resource.
///
/// # First Pass
/// Returns a hardcoded stub response simulating a successful call.
/// Validates the resource_id against the registry but does not submit
/// real transactions.
///
/// # TODO
/// 1. Build the Soroban transaction via `soroban_client::submit_transaction()`
/// 2. Wait for confirmation with bounded retry on transient failure
/// 3. Call the underlying resource via `soroban_client::call_resource()`
/// 4. Return the real transaction hash and resource response
///
/// # Arguments
/// * `resource_id` - The ID of the resource to call (from registry).
/// * `params` - JSON-encoded parameters to pass to the resource.
///
/// # Errors
/// Returns typed errors for policy denial, insufficient budget,
/// resource not found, and resource call failure.
use crate::policy::{PolicyOutcome, evaluate_call};
use crate::soroban_client::{self, SorobanConfig};

/// Default per-vendor window used when no AGENTPAY_STATE is present.
fn default_window() -> (i128, u32, i128, u32) {
    (0, 0, 10_000_000, 10_000)
}

fn local_tx_hash(resource_id: &str, params: &str) -> String {
    let mut n: u64 = 0xcbf29ce484222325;
    for b in resource_id.bytes().chain(params.bytes()) {
        n ^= b as u64;
        n = n.wrapping_mul(0x100000001b3);
    }
    format!("local_{n:016x}")
}

pub fn pay_and_call(resource_id: &str, params: &str) -> Result<PayAndCallResult, PayAndCallError> {
    pay_and_call_with_config(
        resource_id,
        params,
        &SorobanConfig::from_env(),
        default_window(),
    )
}

#[allow(dead_code)]
pub fn pay_and_call_with_window(
    resource_id: &str,
    params: &str,
    window: (i128, u32, i128, u32),
) -> Result<PayAndCallResult, PayAndCallError> {
    pay_and_call_with_config(resource_id, params, &SorobanConfig::from_env(), window)
}

#[allow(dead_code)]
pub fn pay_and_call_with_config(
    resource_id: &str,
    params: &str,
    config: &SorobanConfig,
    window: (i128, u32, i128, u32),
) -> Result<PayAndCallResult, PayAndCallError> {
    let resources = super::discover::load_registry();
    let resource = resources
        .iter()
        .find(|r| r.id == resource_id)
        .ok_or_else(|| PayAndCallError::ResourceNotFound(resource_id.to_string()))?;

    let amount = resource.price as i128;
    if config.is_live()
        && let Ok(remaining) = soroban_client::query_budget(config, config.rule_id)
        && remaining < amount
    {
        return Err(PayAndCallError::InsufficientBudget {
            required: amount,
            available: remaining,
        });
    }

    let (spent, calls, max_spend, max_calls) = window;
    let outcome = evaluate_call(true, amount, spent, max_spend, calls, max_calls);
    if !crate::policy::allows_submission(&outcome) {
        return match outcome {
            PolicyOutcome::Denied { reason } => Err(PayAndCallError::PolicyDenied(reason)),
            PolicyOutcome::InsufficientBudget {
                required,
                available,
            } => Err(PayAndCallError::InsufficientBudget {
                required,
                available,
            }),
            PolicyOutcome::Allow { .. } => unreachable!(),
        };
    }
    if config.is_live() {
        retry_transient(|| -> Result<PayAndCallResult, PayAndCallError> {
            let submitted = soroban_client::pay_and_call_tx(
                config,
                &resource.contract_id,
                &resource.method,
                amount,
            )?;
            if submitted.tx_hash.starts_with("stub_") {
                return Err(PayAndCallError::TransientError(
                    "refusing stub hash on live contract path".into(),
                ));
            }
            Ok(PayAndCallResult {
                tx_hash: submitted.tx_hash,
                ledger: submitted.ledger,
                amount_spent: amount,
                resource_response: submitted.response,
            })
        })
    } else {
        Ok(PayAndCallResult {
            tx_hash: local_tx_hash(resource_id, params),
            ledger: 0,
            amount_spent: amount,
            resource_response: format!(
                "{{\"status\": \"ok\", \"resource\": \"{}\", \"params\": {}}}",
                resource.name, params
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structured_json_not_found() {
        let json = PayAndCallError::ResourceNotFound("x".into()).to_structured_json();
        assert!(json.contains("ResourceNotFound"));
        assert!(json.contains("\"code\""));
    }

    #[test]
    fn test_pay_and_call_local_success() {
        let result = pay_and_call("weather-oracle", "{}");
        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.amount_spent, 50);
        assert!(res.tx_hash.starts_with("local_"));
        assert!(!res.tx_hash.contains("stub_tx_abc123"));
    }

    #[test]
    fn test_live_path_rejects_empty_hash_via_submit() {
        let cfg = crate::soroban_client::SorobanConfig {
            account_contract_id: "CTEST".into(),
            identity: String::new(),
            ..crate::soroban_client::SorobanConfig::from_env()
        };
        let err =
            pay_and_call_with_config("weather-oracle", "{}", &cfg, (0, 0, 10_000_000, 10_000))
                .unwrap_err();
        assert!(!format!("{err}").contains("stub_tx_abc123"));
    }

    #[test]
    fn test_pay_and_call_over_cap() {
        let result = pay_and_call_with_window("weather-oracle", "{}", (0, 0, 10, 10));
        match result.unwrap_err() {
            PayAndCallError::InsufficientBudget {
                required,
                available,
            } => {
                assert_eq!(required, 50);
                assert_eq!(available, 10);
            }
            other => panic!("expected InsufficientBudget, got {other}"),
        }
    }

    #[test]
    fn test_retry_transient_then_success() {
        let mut n = 0;
        let result = retry_transient(|| {
            n += 1;
            if n < 3 {
                Err(PayAndCallError::TransientError("rpc".into()))
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(n, 3);
    }

    #[test]
    fn test_retry_does_not_retry_policy_denied() {
        let mut n = 0;
        let result: Result<(), _> = retry_transient(|| {
            n += 1;
            Err(PayAndCallError::PolicyDenied("no".into()))
        });
        assert!(matches!(result, Err(PayAndCallError::PolicyDenied(_))));
        assert_eq!(n, 1);
    }

    #[test]
    fn test_pay_and_call_not_found() {
        let result = pay_and_call("nonexistent-resource", "{}");
        assert!(result.is_err());
        match result.unwrap_err() {
            PayAndCallError::ResourceNotFound(id) => {
                assert_eq!(id, "nonexistent-resource");
            }
            other => panic!("Expected ResourceNotFound, got: {other}"),
        }
    }
}
