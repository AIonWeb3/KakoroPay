//! Soroban RPC + Stellar CLI client for the AgentPay smart account.

use anyhow::{Result, anyhow};
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

use crate::tools::pay_and_call::PayAndCallError;

const DEFAULT_RPC: &str = "https://soroban-testnet.stellar.org";
const DEFAULT_PASSPHRASE: &str = "Test SDF Network ; September 2015";

#[derive(Debug, Clone)]
pub struct SorobanConfig {
    pub rpc_url: String,
    /// Network passphrase (documented for RPC tx building).
    #[allow(dead_code)]
    pub network_passphrase: String,
    pub account_contract_id: String,
    pub network: String,
    pub identity: String,
    pub rule_id: u32,
    /// G-address of the agent key. Read from the environment; never a secret.
    #[allow(dead_code)]
    pub agent_pubkey: String,
}

impl Default for SorobanConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SorobanConfig {
    pub fn from_env() -> Self {
        Self {
            rpc_url: std::env::var("SOROBAN_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.into()),
            network_passphrase: std::env::var("NETWORK_PASSPHRASE")
                .unwrap_or_else(|_| DEFAULT_PASSPHRASE.into()),
            account_contract_id: std::env::var("ACCOUNT_CONTRACT_ID").unwrap_or_default(),
            network: std::env::var("STELLAR_NETWORK").unwrap_or_else(|_| "testnet".into()),
            identity: std::env::var("STELLAR_IDENTITY").unwrap_or_default(),
            rule_id: std::env::var("AGENTPAY_RULE_ID")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            agent_pubkey: std::env::var("AGENT_SIGNER_PUBKEY").unwrap_or_default(),
        }
    }

    pub fn is_live(&self) -> bool {
        !self.account_contract_id.trim().is_empty()
    }
}

pub fn rpc_post(rpc_url: &str, method: &str, params: Value) -> Result<Value, PayAndCallError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;
    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PayAndCallError::TransientError(format!(
            "rpc http {}",
            resp.status()
        )));
    }
    let v: Value = resp
        .json()
        .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;
    if let Some(err) = v.get("error") {
        return Err(PayAndCallError::TransientError(err.to_string()));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// Poll `getTransaction` until SUCCESS / FAILED or attempts exhausted.
pub fn poll_get_transaction(
    rpc_url: &str,
    tx_hash: &str,
    attempts: u32,
) -> Result<(String, u32), PayAndCallError> {
    for _ in 0..attempts.max(1) {
        let result = rpc_post(
            rpc_url,
            "getTransaction",
            serde_json::json!({ "hash": tx_hash }),
        )?;
        let status = result
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("NOT_FOUND");
        match status {
            "SUCCESS" => {
                let ledger = result.get("ledger").and_then(Value::as_u64).unwrap_or(0) as u32;
                return Ok((tx_hash.to_string(), ledger));
            }
            "FAILED" => {
                return Err(PayAndCallError::PolicyDenied(
                    result
                        .get("resultXdr")
                        .and_then(Value::as_str)
                        .unwrap_or("transaction failed")
                        .to_string(),
                ));
            }
            _ => std::thread::sleep(Duration::from_millis(400)),
        }
    }
    Err(PayAndCallError::TransientError(
        "getTransaction poll timeout".into(),
    ))
}

fn stellar_invoke(cfg: &SorobanConfig, extra: &[&str]) -> Result<String, PayAndCallError> {
    if cfg.identity.is_empty() {
        return Err(PayAndCallError::TransientError(
            "STELLAR_IDENTITY is not set".into(),
        ));
    }
    let mut cmd = Command::new("stellar");
    cmd.arg("contract").arg("invoke");
    cmd.args(extra);
    cmd.arg("--source").arg(&cfg.identity);
    cmd.arg("--network").arg(&cfg.network);
    let output = cmd
        .output()
        .map_err(|e| PayAndCallError::TransientError(format!("stellar cli: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let combined = format!("{stdout}{stderr}");
        if combined.contains("over_budget")
            || combined.contains("OverBudget")
            || combined.contains("Error(Contract, #5)")
        {
            return Err(PayAndCallError::InsufficientBudget {
                required: 0,
                available: 0,
            });
        }
        if combined.contains("rate")
            || combined.contains("RateLimited")
            || combined.contains("Error(Contract, #6)")
            || combined.contains("Error(Contract, #7)")
            || combined.contains("UnvalidatedContext")
        {
            return Err(PayAndCallError::PolicyDenied(combined));
        }
        if combined.contains("connection")
            || combined.contains("timeout")
            || combined.contains("429")
            || combined.contains("unavailable")
        {
            return Err(PayAndCallError::TransientError(combined));
        }
        return Err(PayAndCallError::ResourceCallFailed(combined));
    }
    Ok(stdout)
}

fn parse_tx_hash(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|tok| {
        let t = tok.trim_matches(|c| c == '"' || c == '\'' || c == ',');
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(t.to_string())
        } else {
            None
        }
    })
}

pub fn query_budget(config: &SorobanConfig, rule_id: u32) -> Result<i128> {
    if !config.is_live() {
        return Err(anyhow!("ACCOUNT_CONTRACT_ID is not set"));
    }
    let out = stellar_invoke(
        config,
        &[
            "--id",
            &config.account_contract_id,
            "--",
            "get_remaining_budget",
            "--rule_id",
            &rule_id.to_string(),
        ],
    )
    .map_err(|e| anyhow!("{e}"))?;
    let trimmed = out.trim();
    trimmed
        .lines()
        .rev()
        .find_map(|l: &str| l.trim().parse::<i128>().ok())
        .ok_or_else(|| anyhow!("could not parse remaining budget from: {out}"))
}

pub fn query_rule_count(config: &SorobanConfig) -> Result<u32> {
    if !config.is_live() {
        return Err(anyhow!("ACCOUNT_CONTRACT_ID is not set"));
    }
    let out = stellar_invoke(
        config,
        &["--id", &config.account_contract_id, "--", "get_rule_count"],
    )
    .map_err(|e| anyhow!("{e}"))?;
    out.trim()
        .lines()
        .rev()
        .find_map(|l: &str| l.trim().parse::<u32>().ok())
        .ok_or_else(|| anyhow!("could not parse rule count"))
}

pub fn query_payee_rule(config: &SorobanConfig, asset: &str, pay_to: &str) -> Result<Option<u32>> {
    if !config.is_live() {
        return Err(anyhow!("ACCOUNT_CONTRACT_ID is not set"));
    }
    let out = stellar_invoke(
        config,
        &[
            "--id",
            &config.account_contract_id,
            "--",
            "get_payee_rule",
            "--asset",
            asset,
            "--pay_to",
            pay_to,
        ],
    )
    .map_err(|e| anyhow!("{e}"))?;
    let trimmed = out.trim();
    if trimmed.is_empty() || trimmed == "null" || trimmed == "None" {
        return Ok(None);
    }
    if let Some(rule_id) = trimmed
        .lines()
        .rev()
        .find_map(|l: &str| l.trim().parse::<u32>().ok())
    {
        Ok(Some(rule_id))
    } else {
        Ok(None)
    }
}

/// One Soroban transaction: the smart account authorizes the vendor call,
/// and the vendor contract collects its fee inside that same invocation.
pub struct PayAndCallTx {
    pub tx_hash: String,
    pub ledger: u32,
    pub response: String,
}

pub fn pay_and_call_tx(
    config: &SorobanConfig,
    resource_contract: &str,
    method: &str,
    amount: i128,
) -> Result<PayAndCallTx, PayAndCallError> {
    if !config.is_live() {
        return Err(PayAndCallError::TransientError(
            "ACCOUNT_CONTRACT_ID is not set".into(),
        ));
    }
    let amount_s = amount.to_string();
    let out = stellar_invoke(
        config,
        &[
            "--id",
            resource_contract,
            "--source-account",
            &config.account_contract_id,
            "--send=yes",
            "--",
            method,
            "--amount",
            &amount_s,
        ],
    )?;
    let hash = parse_tx_hash(&out).unwrap_or_default();
    if hash.is_empty() {
        return Err(PayAndCallError::AuthFailed(format!(
            "no tx hash in stellar output: {out}"
        )));
    }
    let (tx_hash, ledger) = poll_get_transaction(&config.rpc_url, &hash, 20)?;
    Ok(PayAndCallTx {
        tx_hash,
        ledger,
        response: out.trim().to_string(),
    })
}

/// Submit a vendor call authorized by the smart account (`--source-account`).
#[allow(dead_code)]
pub fn submit_transaction(
    config: &SorobanConfig,
    resource_contract: &str,
    method: &str,
    amount: i128,
) -> Result<(String, u32), PayAndCallError> {
    if !config.is_live() {
        return Err(PayAndCallError::TransientError(
            "ACCOUNT_CONTRACT_ID is not set".into(),
        ));
    }
    let amount_s = amount.to_string();
    let out = stellar_invoke(
        config,
        &[
            "--id",
            resource_contract,
            "--source-account",
            &config.account_contract_id,
            "--send=yes",
            "--",
            method,
            "--amount",
            &amount_s,
        ],
    )?;
    let hash = parse_tx_hash(&out).unwrap_or_else(|| {
        // stellar may print the return value only; still poll if hash found in stderr mix
        parse_tx_hash(&out).unwrap_or_default()
    });
    if hash.is_empty() {
        return Err(PayAndCallError::TransientError(format!(
            "no tx hash in stellar output: {out}"
        )));
    }
    poll_get_transaction(&config.rpc_url, &hash, 20)
}

#[allow(dead_code)]
pub fn send_transaction_xdr(
    config: &SorobanConfig,
    xdr: &str,
) -> Result<(String, u32), PayAndCallError> {
    let sent = rpc_post(
        &config.rpc_url,
        "sendTransaction",
        serde_json::json!({ "transaction": xdr }),
    )?;
    let hash = sent
        .get("hash")
        .and_then(Value::as_str)
        .ok_or_else(|| PayAndCallError::TransientError("sendTransaction missing hash".into()))?
        .to_string();
    poll_get_transaction(&config.rpc_url, &hash, 20)
}

#[allow(dead_code)]
pub fn call_resource(
    config: &SorobanConfig,
    resource_contract: &str,
    method: &str,
    params: &str,
) -> Result<String, PayAndCallError> {
    if params.trim_start().starts_with("http://") || params.trim_start().starts_with("https://") {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;
        let resp = client
            .get(params.trim())
            .send()
            .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;
        return resp
            .text()
            .map_err(|e| PayAndCallError::ResourceCallFailed(e.to_string()));
    }
    if config.is_live() && !config.identity.is_empty() {
        let out = stellar_invoke(
            config,
            &[
                "--id",
                resource_contract,
                "--source-account",
                &config.account_contract_id,
                "--",
                method,
            ],
        )?;
        return Ok(out.trim().to_string());
    }
    Ok(format!(
        "{{\"status\":\"ok\",\"contract\":\"{resource_contract}\",\"method\":\"{method}\",\"params\":{params}}}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_flag_requires_contract_id() {
        let mut cfg = SorobanConfig::from_env();
        cfg.account_contract_id.clear();
        assert!(!cfg.is_live());
        cfg.account_contract_id = "CABC".into();
        assert!(cfg.is_live());
    }

    #[test]
    fn parse_hash_from_cli_output() {
        let out = "ok\n9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\n";
        assert_eq!(parse_tx_hash(out).unwrap().len(), 64);
    }

    #[test]
    fn query_budget_without_contract_errors() {
        let cfg = SorobanConfig {
            account_contract_id: String::new(),
            ..SorobanConfig::from_env()
        };
        assert!(query_budget(&cfg, 0).is_err());
    }
}
