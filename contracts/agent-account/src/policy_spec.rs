//! # PolicySpec Types
//!
//! Soroban-native types representing the spending policy specification.
//! These match the JSON schema produced by `policy-generator/schema.py`,
//! but use `#[contracttype]` for on-chain XDR serialization.
//!
//! The Python generator produces JSON; the deploy script or MCP server
//! converts that JSON into these Soroban-native types before calling
//! `apply_policy`.

use soroban_sdk::{contracttype, Address, Symbol, Vec};

/// A single allowed contract and the methods + spend cap for it.
/// **Legacy path** — vendor identity = contract address.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct AllowedContract {
    /// The Soroban contract address of the vendor/resource.
    pub contract_id: Address,
    /// Which methods on that contract the agent is allowed to call.
    pub allowed_methods: Vec<Symbol>,
    /// Maximum spend (in stroops) allowed per period for this contract.
    pub max_spend_per_period: i128,
    /// Maximum number of calls allowed per period for this contract.
    pub max_calls_per_period: u32,
}

/// An x402-style SEP-41 payee rule.
///
/// Vendor identity is `(asset, pay_to)` rather than a vendor contract.
/// The rule authorises *only* `transfer(from, pay_to, amount)` calls on
/// `asset`, where `from` must equal the smart-account address.
/// Every `(asset, pay_to)` pair gets an **independent** budget + call cap.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct AllowedPayee {
    /// The SEP-41 token contract address (e.g. USDC).
    pub asset: Address,
    /// The recipient that must appear as `args[1]` in every `transfer`.
    pub pay_to: Address,
    /// Maximum spend (in stroops / token-units) allowed per period.
    pub max_spend_per_period: i128,
    /// Maximum number of transfer calls allowed per period.
    pub max_calls_per_period: u32,
}

/// Top-level policy specification produced by the policy generator
/// and consumed by `apply_policy`.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PolicySpec {
    /// List of vendor contracts the agent is allowed to interact with.
    /// **Legacy** — kept for backward compatibility.
    pub allowed_contracts: Vec<AllowedContract>,
    /// x402 / SEP-41 payee rules keyed by (asset, pay_to).
    /// Each entry creates one `CallContract(asset)` context rule scoped to
    /// `transfer` with a per-payee spend + call cap.
    pub allowed_payees: Vec<AllowedPayee>,
    /// Rolling window size in ledger sequences for spend tracking.
    /// ~17280 ledgers ≈ 1 day on Stellar.
    pub period_ledgers: u32,
    /// When set, each new `CallContract` rule lists this key as a delegated signer.
    /// `None` keeps the historical empty signer list.
    pub agent_signer: Option<Address>,
}
