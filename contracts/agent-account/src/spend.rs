//! Spend and rate-limit policy invoked by the smart-account framework
//! during `__check_auth` (`install` / `enforce` / `uninstall`).
//!
//! ## Payee-aware enforcement (x402 / SEP-41)
//!
//! When `PolicyKey::PayTo(account, rule_id)` is present the rule is a
//! *payee rule* authorising only `transfer(from, to, amount)` on the
//! bound token contract where:
//!   - `fn_name == "transfer"`
//!   - `args.len() == 3`
//!   - `args[0]` is an `Address` equal to `smart_account`
//!   - `args[1]` is an `Address` equal to the stored `pay_to`
//!   - `args[2]` is an `i128 > 0`
//!
//! For legacy rules (no `PayTo` entry) the original first-i128 extraction
//! is preserved and no payee-binding check is performed.

use soroban_sdk::{
    auth::{Context, ContractContext},
    contracttype, panic_with_error, Address, Env, Symbol, TryFromVal, Vec,
};
#[cfg(test)]
use soroban_sdk::{contract, contractimpl};
use stellar_accounts::smart_account::{ContextRule, ContextRuleType, Signer};

use crate::{emit_auth_decision, AgentAccountError};

// ---------------------------------------------------------------------------
// Public params type (used by install)
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct SpendPolicyParams {
    pub max_spend_per_period: i128,
    pub max_calls_per_period: u32,
    pub period_ledgers: u32,
    pub expected_method: Symbol,
    pub pay_to: Option<Address>,
}

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum RuleKind {
    Legacy = 0,
    Payee = 1,
}

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub enum PolicyKey {
    SpendCap(Address, u32),
    CallCap(Address, u32),
    Period(Address, u32),
    LastReset(Address, u32),
    Spent(Address, u32),
    Calls(Address, u32),
    Contract(Address, u32),
    Method(Address, u32),
    /// Explicit rule kind (Legacy or Payee).
    Kind(Address, u32),
    /// Present only for payee rules. Stores the bound recipient address.
    PayTo(Address, u32),
}

// ---------------------------------------------------------------------------
// Budget query
// ---------------------------------------------------------------------------

pub fn budget_remaining(env: &Env, smart_account: &Address, rule_id: u32) -> i128 {
    let cap: i128 = env
        .storage()
        .persistent()
        .get(&PolicyKey::SpendCap(smart_account.clone(), rule_id))
        .unwrap_or(0);
    reset_if_needed(env, smart_account, rule_id);
    let spent: i128 = env
        .storage()
        .persistent()
        .get(&PolicyKey::Spent(smart_account.clone(), rule_id))
        .unwrap_or(0);
    cap - spent
}

// ---------------------------------------------------------------------------
// Rolling-window reset
// ---------------------------------------------------------------------------

fn reset_if_needed(env: &Env, account: &Address, rule_id: u32) {
    let last_reset: u32 = env
        .storage()
        .persistent()
        .get(&PolicyKey::LastReset(account.clone(), rule_id))
        .unwrap_or(env.ledger().sequence());
    let period: u32 = env
        .storage()
        .persistent()
        .get(&PolicyKey::Period(account.clone(), rule_id))
        .unwrap_or(u32::MAX);
    if env.ledger().sequence() >= last_reset.saturating_add(period) {
        env.storage()
            .persistent()
            .set(&PolicyKey::Spent(account.clone(), rule_id), &0i128);
        env.storage()
            .persistent()
            .set(&PolicyKey::Calls(account.clone(), rule_id), &0u32);
        env.storage().persistent().set(
            &PolicyKey::LastReset(account.clone(), rule_id),
            &env.ledger().sequence(),
        );
    }
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

pub fn install(
    env: &Env,
    params: &SpendPolicyParams,
    context_rule: &ContextRule,
    smart_account: &Address,
) {
    let rule_id = context_rule.id;
    let contract_id = match &context_rule.context_type {
        ContextRuleType::CallContract(addr) => addr.clone(),
        _ => {
            panic_with_error!(env, AgentAccountError::InvalidContext);
        }
    };

    env.storage().persistent().set(
        &PolicyKey::SpendCap(smart_account.clone(), rule_id),
        &params.max_spend_per_period,
    );
    env.storage().persistent().set(
        &PolicyKey::CallCap(smart_account.clone(), rule_id),
        &params.max_calls_per_period,
    );
    env.storage().persistent().set(
        &PolicyKey::Period(smart_account.clone(), rule_id),
        &params.period_ledgers,
    );
    env.storage().persistent().set(
        &PolicyKey::LastReset(smart_account.clone(), rule_id),
        &env.ledger().sequence(),
    );
    env.storage()
        .persistent()
        .set(&PolicyKey::Spent(smart_account.clone(), rule_id), &0i128);
    env.storage()
        .persistent()
        .set(&PolicyKey::Calls(smart_account.clone(), rule_id), &0u32);
    env.storage().persistent().set(
        &PolicyKey::Contract(smart_account.clone(), rule_id),
        &contract_id,
    );
    env.storage().persistent().set(
        &PolicyKey::Method(smart_account.clone(), rule_id),
        &params.expected_method,
    );
    let kind = if params.pay_to.is_some() {
        RuleKind::Payee
    } else {
        RuleKind::Legacy
    };
    env.storage()
        .persistent()
        .set(&PolicyKey::Kind(smart_account.clone(), rule_id), &kind);
    if let Some(ref to) = params.pay_to {
        env.storage()
            .persistent()
            .set(&PolicyKey::PayTo(smart_account.clone(), rule_id), to);
    }
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

pub fn uninstall(env: &Env, context_rule: &ContextRule, smart_account: &Address) {
    let rule_id = context_rule.id;
    let a = smart_account.clone();
    env.storage()
        .persistent()
        .remove(&PolicyKey::SpendCap(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::CallCap(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Period(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::LastReset(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Spent(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Calls(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Contract(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Method(a.clone(), rule_id));
    env.storage()
        .persistent()
        .remove(&PolicyKey::Kind(a.clone(), rule_id));
    // Remove payee binding if present (no-op for legacy rules).
    env.storage()
        .persistent()
        .remove(&PolicyKey::PayTo(a, rule_id));
}

// ---------------------------------------------------------------------------
// Amount extraction
// ---------------------------------------------------------------------------

/// Legacy: return the first i128 found anywhere in args.
/// Used by `__check_auth`'s approval event and by legacy rules.
pub fn extract_amount(env: &Env, args: &Vec<soroban_sdk::Val>) -> i128 {
    for i in 0..args.len() {
        if let Some(v) = args.get(i) {
            if let Ok(amount) = i128::try_from_val(env, &v) {
                return amount;
            }
        }
    }
    0
}

/// Parse `transfer(from: Address, to: Address, amount: i128)`.
///
/// Returns `Ok((to, amount))` when:
///   - exactly 3 args
///   - `args[0]` decodes as an `Address` equal to `smart_account`
///   - `args[1]` decodes as an `Address`
///   - `args[2]` decodes as `i128 > 0`
///
/// Returns `Err(InvalidContext)` for any violation.
fn parse_transfer_args(
    env: &Env,
    args: &Vec<soroban_sdk::Val>,
    smart_account: &Address,
) -> Result<(Address, i128), AgentAccountError> {
    if args.len() != 3 {
        return Err(AgentAccountError::InvalidContext);
    }
    let from = Address::try_from_val(env, &args.get(0).unwrap())
        .map_err(|_| AgentAccountError::InvalidContext)?;
    if &from != smart_account {
        return Err(AgentAccountError::InvalidContext);
    }
    let to = Address::try_from_val(env, &args.get(1).unwrap())
        .map_err(|_| AgentAccountError::InvalidContext)?;
    let amount = i128::try_from_val(env, &args.get(2).unwrap())
        .map_err(|_| AgentAccountError::InvalidContext)?;
    if amount <= 0 {
        return Err(AgentAccountError::InvalidContext);
    }
    Ok((to, amount))
}

// ---------------------------------------------------------------------------
// enforce
// ---------------------------------------------------------------------------

pub fn enforce(
    env: &Env,
    context: &Context,
    _authenticated_signers: &Vec<Signer>,
    context_rule: &ContextRule,
    smart_account: &Address,
) {
    let rule_id = context_rule.id;
    let Context::Contract(ContractContext {
        contract,
        fn_name,
        args,
    }) = context
    else {
        emit_auth_decision(
            env,
            Symbol::new(env, "denied"),
            Symbol::new(env, "invalid_context"),
            0,
            rule_id,
            None,
            None,
        );
        panic_with_error!(env, AgentAccountError::InvalidContext);
    };

    let expected_contract: Option<Address> = env
        .storage()
        .persistent()
        .get(&PolicyKey::Contract(smart_account.clone(), rule_id));
    let expected_method: Option<Symbol> = env
        .storage()
        .persistent()
        .get(&PolicyKey::Method(smart_account.clone(), rule_id));

    // Check explicit rule kind.
    let rule_kind: RuleKind = env
        .storage()
        .persistent()
        .get(&PolicyKey::Kind(smart_account.clone(), rule_id))
        .unwrap_or(RuleKind::Legacy);

    // Determine bound recipient if present.
    let bound_pay_to: Option<Address> = env
        .storage()
        .persistent()
        .get(&PolicyKey::PayTo(smart_account.clone(), rule_id));

    let amount;

    if rule_kind == RuleKind::Payee {
        let Some(ref expected_pay_to) = bound_pay_to else {
            emit_auth_decision(
                env,
                Symbol::new(env, "denied"),
                Symbol::new(env, "invalid_context"),
                0,
                rule_id,
                Some(contract.clone()),
                Some(fn_name.clone()),
            );
            panic_with_error!(env, AgentAccountError::InvalidContext);
        };
        // ---------------------------------------------------------------
        // Payee rule: enforce transfer(from, pay_to, amount) semantics
        // ---------------------------------------------------------------
        let transfer_sym = Symbol::new(env, "transfer");

        // fn_name must be "transfer"
        if fn_name != &transfer_sym {
            emit_auth_decision(
                env,
                Symbol::new(env, "denied"),
                Symbol::new(env, "invalid_context"),
                0,
                rule_id,
                Some(contract.clone()),
                Some(fn_name.clone()),
            );
            panic_with_error!(env, AgentAccountError::InvalidContext);
        }

        // Contract must match stored asset
        if expected_contract.as_ref() != Some(contract) {
            emit_auth_decision(
                env,
                Symbol::new(env, "denied"),
                Symbol::new(env, "invalid_context"),
                0,
                rule_id,
                Some(contract.clone()),
                Some(fn_name.clone()),
            );
            panic_with_error!(env, AgentAccountError::InvalidContext);
        }

        // Parse and validate transfer args
        let (to, parsed_amount) = match parse_transfer_args(env, args, smart_account) {
            Ok(v) => v,
            Err(_) => {
                emit_auth_decision(
                    env,
                    Symbol::new(env, "denied"),
                    Symbol::new(env, "invalid_context"),
                    0,
                    rule_id,
                    Some(contract.clone()),
                    Some(fn_name.clone()),
                );
                panic_with_error!(env, AgentAccountError::InvalidContext);
            }
        };

        // Verify recipient matches the bound pay_to
        if &to != expected_pay_to {
            emit_auth_decision(
                env,
                Symbol::new(env, "denied"),
                Symbol::new(env, "invalid_context"),
                0,
                rule_id,
                Some(contract.clone()),
                Some(fn_name.clone()),
            );
            panic_with_error!(env, AgentAccountError::InvalidContext);
        }

        amount = parsed_amount;
    } else {
        // ---------------------------------------------------------------
        // Legacy rule: contract + method check, first-i128 extraction
        // ---------------------------------------------------------------
        let legacy_amount = extract_amount(env, args);
        amount = legacy_amount;

        if expected_contract.as_ref() != Some(contract) || expected_method.as_ref() != Some(fn_name)
        {
            emit_auth_decision(
                env,
                Symbol::new(env, "denied"),
                Symbol::new(env, "invalid_context"),
                amount,
                rule_id,
                Some(contract.clone()),
                Some(fn_name.clone()),
            );
            panic_with_error!(env, AgentAccountError::InvalidContext);
        }
    }

    reset_if_needed(env, smart_account, rule_id);

    let cap: i128 = env
        .storage()
        .persistent()
        .get(&PolicyKey::SpendCap(smart_account.clone(), rule_id))
        .unwrap_or(0);
    let spent: i128 = env
        .storage()
        .persistent()
        .get(&PolicyKey::Spent(smart_account.clone(), rule_id))
        .unwrap_or(0);
    let call_cap: u32 = env
        .storage()
        .persistent()
        .get(&PolicyKey::CallCap(smart_account.clone(), rule_id))
        .unwrap_or(0);
    let calls: u32 = env
        .storage()
        .persistent()
        .get(&PolicyKey::Calls(smart_account.clone(), rule_id))
        .unwrap_or(0);

    if calls.saturating_add(1) > call_cap {
        emit_auth_decision(
            env,
            Symbol::new(env, "denied"),
            Symbol::new(env, "rate_limited"),
            amount,
            rule_id,
            Some(contract.clone()),
            Some(fn_name.clone()),
        );
        panic_with_error!(env, AgentAccountError::RateLimited);
    }

    if spent + amount > cap {
        emit_auth_decision(
            env,
            Symbol::new(env, "denied"),
            Symbol::new(env, "over_budget"),
            amount,
            rule_id,
            Some(contract.clone()),
            Some(fn_name.clone()),
        );
        panic_with_error!(env, AgentAccountError::OverBudget);
    }

    env.storage().persistent().set(
        &PolicyKey::Spent(smart_account.clone(), rule_id),
        &(spent + amount),
    );
    env.storage().persistent().set(
        &PolicyKey::Calls(smart_account.clone(), rule_id),
        &(calls + 1),
    );

    emit_auth_decision(
        env,
        Symbol::new(env, "approved"),
        Symbol::new(env, "approved"),
        amount,
        rule_id,
        Some(contract.clone()),
        Some(fn_name.clone()),
    );
}

// ---------------------------------------------------------------------------
// Separate SpendPolicyContract instance
// ---------------------------------------------------------------------------

/// Separate contract instance so `PolicyClient` can call `install`/`enforce`
/// without re-entering the smart account.
#[cfg(test)]
#[contract]
pub struct SpendPolicyContract;

#[cfg(test)]
#[contractimpl]
impl SpendPolicyContract {
    pub fn install(
        env: Env,
        install_params: SpendPolicyParams,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        crate::spend::install(&env, &install_params, &context_rule, &smart_account);
    }

    pub fn enforce(
        env: Env,
        context: Context,
        authenticated_signers: Vec<Signer>,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        crate::spend::enforce(
            &env,
            &context,
            &authenticated_signers,
            &context_rule,
            &smart_account,
        );
    }

    pub fn uninstall(env: Env, context_rule: ContextRule, smart_account: Address) {
        crate::spend::uninstall(&env, &context_rule, &smart_account);
    }

    pub fn remaining_budget(env: Env, smart_account: Address, rule_id: u32) -> i128 {
        crate::spend::budget_remaining(&env, &smart_account, rule_id)
    }
}
