//! # AgentPay Smart Account
//!
//! A Soroban smart account representing an AI agent's wallet, built on
//! OpenZeppelin's `stellar-accounts` framework. It uses context rules to
//! scope authorization per vendor contract, and a spend/rate policy
//! attached to each rule (this contract implements `install`/`enforce`
//! so the framework invokes it from `__check_auth`).

#![no_std]

pub mod agent_guard;
pub mod policy_spec;
mod spend;

#[cfg(test)]
mod test;

use soroban_sdk::{
    auth::{Context, CustomAccountInterface},
    contract, contracterror, contractevent, contractimpl, contracttype,
    crypto::Hash,
    panic_with_error, Address, Env, IntoVal, Map, String, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{
    add_context_rule, do_check_auth, get_context_rule, remove_context_rule, AuthPayload,
    ContextRuleType, Signer, SmartAccountError,
};

use crate::agent_guard::Role;
use crate::policy_spec::{AllowedContract, AllowedPayee, PolicySpec};
use crate::spend::SpendPolicyParams;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    RuleCount,
    SpendPolicy,
    AgentGuard,
    RequiredRole,
    /// Context-rule ids installed by apply/replace (framework ids, not a dense counter).
    ActiveRuleIds,
    LastRuleId,
    PayeeRule(Address, Address),
    PayeeKeys,
}

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum AgentAccountError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    EmptyPolicy = 4,
    OverBudget = 5,
    RateLimited = 6,
    InvalidContext = 7,
    /// AgentGuard `verify_agent` returned false (or is required and denied).
    AgentGuardExecutionDenied = 8,
}

#[contractevent]
#[derive(Clone)]
pub struct AuthDecision {
    #[topic]
    pub kind: Symbol,
    pub decision: Symbol,
    pub reason: Symbol,
    pub amount: i128,
    pub rule_id: u32,
    pub contract_id: Option<Address>,
    pub method: Option<Symbol>,
}

pub(crate) fn emit_auth_decision(
    env: &Env,
    decision: Symbol,
    reason: Symbol,
    amount: i128,
    rule_id: u32,
    contract_id: Option<Address>,
    method: Option<Symbol>,
) {
    AuthDecision {
        kind: Symbol::new(env, "auth_decision"),
        decision,
        reason,
        amount,
        rule_id,
        contract_id,
        method,
    }
    .publish(env);
}

#[contractevent]
#[derive(Clone)]
pub struct PolicyApplied {
    #[topic]
    pub kind: Symbol,
    pub contract_id: Address,
    pub method: Symbol,
    pub max_spend_per_period: i128,
    pub rule_id: Option<u32>,
    pub pay_to: Option<Address>,
}

#[contract]
pub struct AgentAccountContract;

#[contractimpl]
impl AgentAccountContract {
    pub fn initialize(env: Env, admin: Address) -> Result<(), AgentAccountError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(AgentAccountError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::RuleCount, &0u32);
        Ok(())
    }

    /// Bind the spend/rate policy contract the framework will invoke on auth.
    /// Deploy a second instance of this package's `SpendPolicyContract`.
    pub fn set_spend_policy(
        env: Env,
        admin: Address,
        policy: Address,
    ) -> Result<(), AgentAccountError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AgentAccountError::NotInitialized)?;
        if admin != stored_admin {
            return Err(AgentAccountError::Unauthorized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::SpendPolicy, &policy);
        Ok(())
    }

    /// Bind the AgentGuard registry used as a pre-flight check in `__check_auth`.
    pub fn set_agent_guard(
        env: Env,
        admin: Address,
        guard: Address,
        required_role: Role,
    ) -> Result<(), AgentAccountError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AgentAccountError::NotInitialized)?;
        if admin != stored_admin {
            return Err(AgentAccountError::Unauthorized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::AgentGuard, &guard);
        env.storage()
            .instance()
            .set(&DataKey::RequiredRole, &required_role);
        Ok(())
    }

    pub fn get_agent_guard(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::AgentGuard)
    }

    pub fn apply_policy(
        env: Env,
        admin: Address,
        policy_spec: PolicySpec,
    ) -> Result<(), AgentAccountError> {
        authorize_admin(&env, &admin)?;
        let added = install_spec(&env, &policy_spec)?;
        let mut active = active_rule_ids(&env);
        for i in 0..added.len() {
            active.push_back(added.get(i).unwrap());
        }
        env.storage()
            .instance()
            .set(&DataKey::ActiveRuleIds, &active);
        env.storage()
            .instance()
            .set(&DataKey::RuleCount, &active.len());
        Ok(())
    }

    /// Drop every rule installed by this account, then install `policy_spec`.
    pub fn replace_policy(
        env: Env,
        admin: Address,
        policy_spec: PolicySpec,
    ) -> Result<(), AgentAccountError> {
        authorize_admin(&env, &admin)?;
        clear_active_rules(&env);
        let added = install_spec(&env, &policy_spec)?;
        env.storage()
            .instance()
            .set(&DataKey::ActiveRuleIds, &added);
        env.storage()
            .instance()
            .set(&DataKey::RuleCount, &added.len());
        Ok(())
    }

    /// Remove installed rules and leave the account with an empty rule set.
    pub fn revoke_policy(env: Env, admin: Address) -> Result<(), AgentAccountError> {
        authorize_admin(&env, &admin)?;
        clear_active_rules(&env);
        let empty: Vec<u32> = Vec::new(&env);
        env.storage()
            .instance()
            .set(&DataKey::ActiveRuleIds, &empty);
        env.storage().instance().set(&DataKey::RuleCount, &0u32);
        Ok(())
    }

    pub fn get_remaining_budget(env: Env, rule_id: u32) -> i128 {
        let policy: Address = env
            .storage()
            .instance()
            .get(&DataKey::SpendPolicy)
            .unwrap_or_else(|| panic_with_error!(&env, AgentAccountError::NotInitialized));
        let account = env.current_contract_address();
        env.invoke_contract(
            &policy,
            &Symbol::new(&env, "remaining_budget"),
            soroban_sdk::vec![&env, account.into_val(&env), rule_id.into_val(&env)],
        )
    }

    pub fn get_admin(env: Env) -> Result<Address, AgentAccountError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AgentAccountError::NotInitialized)
    }

    pub fn get_rule_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::RuleCount)
            .unwrap_or(0u32)
    }

    pub fn get_last_rule_id(env: Env) -> Option<u32> {
        env.storage().instance().get(&DataKey::LastRuleId)
    }

    pub fn get_payee_rule(env: Env, asset: Address, pay_to: Address) -> Option<u32> {
        env.storage()
            .instance()
            .get(&DataKey::PayeeRule(asset, pay_to))
    }

    pub fn rule_signer_count(env: Env, rule_id: u32) -> u32 {
        get_context_rule(&env, rule_id).signers.len()
    }

    /// Policy interface (used when this wasm is deployed as the spend-policy instance).
    pub fn install(
        env: Env,
        install_params: SpendPolicyParams,
        context_rule: stellar_accounts::smart_account::ContextRule,
        smart_account: Address,
    ) {
        spend::install(&env, &install_params, &context_rule, &smart_account);
    }

    pub fn enforce(
        env: Env,
        context: Context,
        authenticated_signers: soroban_sdk::Vec<stellar_accounts::smart_account::Signer>,
        context_rule: stellar_accounts::smart_account::ContextRule,
        smart_account: Address,
    ) {
        spend::enforce(
            &env,
            &context,
            &authenticated_signers,
            &context_rule,
            &smart_account,
        );
    }

    pub fn uninstall(
        env: Env,
        context_rule: stellar_accounts::smart_account::ContextRule,
        smart_account: Address,
    ) {
        spend::uninstall(&env, &context_rule, &smart_account);
    }

    pub fn remaining_budget(env: Env, smart_account: Address, rule_id: u32) -> i128 {
        spend::budget_remaining(&env, &smart_account, rule_id)
    }
}

fn authorize_admin(env: &Env, admin: &Address) -> Result<(), AgentAccountError> {
    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(AgentAccountError::NotInitialized)?;
    if admin != &stored_admin {
        return Err(AgentAccountError::Unauthorized);
    }
    admin.require_auth();
    Ok(())
}

fn active_rule_ids(env: &Env) -> Vec<u32> {
    env.storage()
        .instance()
        .get(&DataKey::ActiveRuleIds)
        .unwrap_or_else(|| Vec::new(env))
}

fn clear_active_rules(env: &Env) {
    let active = active_rule_ids(env);
    for i in 0..active.len() {
        remove_context_rule(env, active.get(i).unwrap());
    }
    if let Some(keys) = env
        .storage()
        .instance()
        .get::<_, Vec<(Address, Address)>>(&DataKey::PayeeKeys)
    {
        for i in 0..keys.len() {
            let (asset, pay_to) = keys.get(i).unwrap();
            env.storage()
                .instance()
                .remove(&DataKey::PayeeRule(asset, pay_to));
        }
        env.storage().instance().remove(&DataKey::PayeeKeys);
    }
}

fn install_spec(env: &Env, policy_spec: &PolicySpec) -> Result<Vec<u32>, AgentAccountError> {
    if policy_spec.allowed_contracts.is_empty() && policy_spec.allowed_payees.is_empty() {
        return Err(AgentAccountError::EmptyPolicy);
    }
    let policy_contract: Address = env
        .storage()
        .instance()
        .get(&DataKey::SpendPolicy)
        .ok_or(AgentAccountError::NotInitialized)?;

    let mut added: Vec<u32> = Vec::new(env);

    // -----------------------------------------------------------------------
    // Legacy AllowedContract rules (unchanged behaviour)
    // -----------------------------------------------------------------------
    let transfer_sym = Symbol::new(env, "transfer");
    for i in 0..policy_spec.allowed_contracts.len() {
        let allowed: AllowedContract = policy_spec.allowed_contracts.get(i).unwrap();
        for j in 0..allowed.allowed_methods.len() {
            let method = allowed.allowed_methods.get(j).unwrap();
            if method == transfer_sym {
                return Err(AgentAccountError::InvalidContext);
            }
            let context_type = ContextRuleType::CallContract(allowed.contract_id.clone());
            let mut signers: Vec<Signer> = Vec::new(env);
            if let Some(agent) = policy_spec.agent_signer.clone() {
                signers.push_back(Signer::Delegated(agent));
            }
            let params = SpendPolicyParams {
                max_spend_per_period: allowed.max_spend_per_period,
                max_calls_per_period: allowed.max_calls_per_period,
                period_ledgers: policy_spec.period_ledgers,
                expected_method: method.clone(),
                pay_to: None,
            };
            let mut policies: Map<Address, Val> = Map::new(env);
            policies.set(policy_contract.clone(), params.into_val(env));
            let rule_name = String::from_str(env, "agent_rule");
            let rule = add_context_rule(env, &context_type, &rule_name, None, &signers, &policies);
            added.push_back(rule.id);
            env.storage().instance().set(&DataKey::LastRuleId, &rule.id);
            PolicyApplied {
                kind: Symbol::new(env, "policy_applied"),
                contract_id: allowed.contract_id.clone(),
                method,
                max_spend_per_period: allowed.max_spend_per_period,
                rule_id: Some(rule.id),
                pay_to: None,
            }
            .publish(env);
        }
    }

    // -----------------------------------------------------------------------
    // Payee rules: one rule per (asset, pay_to) pair, scoped to "transfer"
    // -----------------------------------------------------------------------
    let mut payee_keys: Vec<(Address, Address)> = env
        .storage()
        .instance()
        .get(&DataKey::PayeeKeys)
        .unwrap_or_else(|| Vec::new(env));
    for i in 0..policy_spec.allowed_payees.len() {
        let payee: AllowedPayee = policy_spec.allowed_payees.get(i).unwrap();
        let context_type = ContextRuleType::CallContract(payee.asset.clone());
        let mut signers: Vec<Signer> = Vec::new(env);
        if let Some(agent) = policy_spec.agent_signer.clone() {
            signers.push_back(Signer::Delegated(agent));
        }
        let params = SpendPolicyParams {
            max_spend_per_period: payee.max_spend_per_period,
            max_calls_per_period: payee.max_calls_per_period,
            period_ledgers: policy_spec.period_ledgers,
            expected_method: transfer_sym.clone(),
            pay_to: Some(payee.pay_to.clone()),
        };
        let mut policies: Map<Address, Val> = Map::new(env);
        policies.set(policy_contract.clone(), params.into_val(env));
        let rule_name = String::from_str(env, "payee_rule");
        let rule = add_context_rule(env, &context_type, &rule_name, None, &signers, &policies);
        added.push_back(rule.id);
        env.storage().instance().set(&DataKey::LastRuleId, &rule.id);
        env.storage().instance().set(
            &DataKey::PayeeRule(payee.asset.clone(), payee.pay_to.clone()),
            &rule.id,
        );
        payee_keys.push_back((payee.asset.clone(), payee.pay_to.clone()));
        PolicyApplied {
            kind: Symbol::new(env, "policy_applied"),
            contract_id: payee.asset.clone(),
            method: transfer_sym.clone(),
            max_spend_per_period: payee.max_spend_per_period,
            rule_id: Some(rule.id),
            pay_to: Some(payee.pay_to.clone()),
        }
        .publish(env);
    }
    env.storage()
        .instance()
        .set(&DataKey::PayeeKeys, &payee_keys);

    Ok(added)
}

/// Read-only AgentGuard check before spend-policy effects in `do_check_auth`.
fn require_agent_guard(env: &Env, auth_contexts: &Vec<Context>) {
    let Some(guard) = env
        .storage()
        .instance()
        .get::<_, Address>(&DataKey::AgentGuard)
    else {
        return;
    };
    let required_role: Role = env
        .storage()
        .instance()
        .get(&DataKey::RequiredRole)
        .unwrap_or(Role::Basic);
    let agent_id = env.current_contract_address();
    if !agent_guard::verify_agent(env, &guard, &agent_id, &required_role) {
        let (amount, contract_id, method) = first_call_meta(env, auth_contexts);
        emit_auth_decision(
            env,
            Symbol::new(env, "denied"),
            Symbol::new(env, "guard_denied"),
            amount,
            0,
            contract_id,
            method,
        );
        panic_with_error!(env, AgentAccountError::AgentGuardExecutionDenied);
    }
}

fn first_call_meta(
    env: &Env,
    auth_contexts: &Vec<Context>,
) -> (i128, Option<Address>, Option<Symbol>) {
    for i in 0..auth_contexts.len() {
        if let Context::Contract(cc) = auth_contexts.get(i).unwrap() {
            return (
                spend::extract_amount(env, &cc.args),
                Some(cc.contract.clone()),
                Some(cc.fn_name.clone()),
            );
        }
    }
    (0, None, None)
}

#[contractimpl]
impl CustomAccountInterface for AgentAccountContract {
    type Error = SmartAccountError;
    type Signature = AuthPayload;

    #[allow(non_snake_case)]
    fn __check_auth(
        env: Env,
        signature_payload: Hash<32>,
        signature: AuthPayload,
        auth_contexts: Vec<Context>,
    ) -> Result<(), SmartAccountError> {
        require_agent_guard(&env, &auth_contexts);
        if signature.context_rule_ids.len() != auth_contexts.len() {
            emit_auth_decision(
                &env,
                Symbol::new(&env, "denied"),
                Symbol::new(&env, "unauthorized"),
                0,
                0,
                None,
                None,
            );
        }
        do_check_auth(&env, &signature_payload, &signature, &auth_contexts)?;
        for i in 0..auth_contexts.len() {
            if let Context::Contract(cc) = auth_contexts.get(i).unwrap() {
                let amount = spend::extract_amount(&env, &cc.args);
                let rule_id = signature.context_rule_ids.get(i).unwrap_or(0);
                emit_auth_decision(
                    &env,
                    Symbol::new(&env, "approved"),
                    Symbol::new(&env, "approved"),
                    amount,
                    rule_id,
                    Some(cc.contract.clone()),
                    Some(cc.fn_name.clone()),
                );
            }
        }
        Ok(())
    }
}
