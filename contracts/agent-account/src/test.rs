//! Unit tests: auth-path deny/approve, per-vendor isolation, rolling window,
//! rate limits, payee-binding checks, and `auth_decision` events.

#![cfg(test)]

extern crate std;

use soroban_sdk::{
    auth::{Context, ContractContext},
    testutils::{Address as _, Events, Ledger},
    vec, Address, Bytes, BytesN, Env, IntoVal, Map, Symbol, Val, Vec,
};
use stellar_accounts::smart_account::{AuthPayload, Signer};

use crate::{
    agent_guard::Role,
    policy_spec::{AllowedContract, AllowedPayee, PolicySpec},
    spend::SpendPolicyContract,
    AgentAccountContract, AgentAccountContractClient,
};

#[soroban_sdk::contract]
pub struct MockAgentGuard;

#[soroban_sdk::contractimpl]
impl MockAgentGuard {
    pub fn verify_agent(_env: soroban_sdk::Env, _agent_id: Address, _required_role: Role) -> bool {
        true
    }
}

#[soroban_sdk::contract]
pub struct DenyingAgentGuard;

#[soroban_sdk::contractimpl]
impl DenyingAgentGuard {
    pub fn verify_agent(_env: soroban_sdk::Env, _agent_id: Address, _required_role: Role) -> bool {
        false
    }
}

fn setup() -> (Env, AgentAccountContractClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let policy = env.register(SpendPolicyContract, ());
    let account = env.register(AgentAccountContract, ());
    let client = AgentAccountContractClient::new(&env, &account);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    client.set_spend_policy(&admin, &policy);
    (env, client, admin, account)
}

/// Empty PolicySpec with no payees — only used by legacy tests.
fn empty_payees(env: &Env) -> Vec<AllowedPayee> {
    Vec::new(env)
}

fn sample_policy(env: &Env, vendor: &Address, cap: i128, period: u32) -> PolicySpec {
    sample_policy_calls(env, vendor, cap, period, 100)
}

fn sample_policy_calls(
    env: &Env,
    vendor: &Address,
    cap: i128,
    period: u32,
    max_calls: u32,
) -> PolicySpec {
    let mut methods: Vec<Symbol> = Vec::new(env);
    methods.push_back(Symbol::new(env, "get_data"));
    let allowed = AllowedContract {
        contract_id: vendor.clone(),
        allowed_methods: methods,
        max_spend_per_period: cap,
        max_calls_per_period: max_calls,
    };
    let mut contracts: Vec<AllowedContract> = Vec::new(env);
    contracts.push_back(allowed);
    PolicySpec {
        allowed_contracts: contracts,
        allowed_payees: empty_payees(env),
        period_ledgers: period,
        agent_signer: None,
    }
}

/// Build a transfer context: transfer(from=account, to=payee, amount)
fn transfer_ctx(
    env: &Env,
    asset: &Address,
    account: &Address,
    payee: &Address,
    amount: i128,
) -> Context {
    let mut args: Vec<Val> = Vec::new(env);
    args.push_back(account.into_val(env));
    args.push_back(payee.into_val(env));
    args.push_back(amount.into_val(env));
    Context::Contract(ContractContext {
        contract: asset.clone(),
        fn_name: Symbol::new(env, "transfer"),
        args,
    })
}

fn try_auth(
    env: &Env,
    account: &Address,
    vendor: &Address,
    method: Symbol,
    amount: i128,
    rule_id: u32,
) -> Result<(), u32> {
    let mut args: Vec<Val> = Vec::new(env);
    args.push_back(amount.into_val(env));
    let ctx = Context::Contract(ContractContext {
        contract: vendor.clone(),
        fn_name: method,
        args,
    });
    let payload = AuthPayload {
        signers: Map::new(env),
        context_rule_ids: vec![env, rule_id],
    };
    let hash = BytesN::<32>::from_array(env, &[7u8; 32]);
    match env.try_invoke_contract_check_auth::<stellar_accounts::smart_account::SmartAccountError>(
        account,
        &hash,
        payload.into_val(env),
        &vec![env, ctx],
    ) {
        Ok(()) => Ok(()),
        Err(Ok(_)) => Err(1),
        Err(Err(soroban_sdk::InvokeError::Contract(code))) => Err(code),
        Err(Err(_)) => Err(0),
    }
}

/// Invoke check_auth with a raw Context and rule_id (for payee tests).
fn try_auth_ctx(env: &Env, account: &Address, ctx: Context, rule_id: u32) -> Result<(), u32> {
    let payload = AuthPayload {
        signers: Map::new(env),
        context_rule_ids: vec![env, rule_id],
    };
    let hash = BytesN::<32>::from_array(env, &[7u8; 32]);
    match env.try_invoke_contract_check_auth::<stellar_accounts::smart_account::SmartAccountError>(
        account,
        &hash,
        payload.into_val(env),
        &vec![env, ctx],
    ) {
        Ok(()) => Ok(()),
        Err(Ok(_)) => Err(1),
        Err(Err(soroban_sdk::InvokeError::Contract(code))) => Err(code),
        Err(Err(_)) => Err(0),
    }
}

fn has_auth_reason(env: &Env, reason: &str) -> bool {
    use soroban_sdk::xdr::ContractEventBody;
    env.events().all().events().iter().any(|ev| {
        let ContractEventBody::V0(body) = &ev.body;
        std::format!("{body:?}").contains(reason)
    })
}

// ---------------------------------------------------------------------------
// Legacy tests (unchanged behaviour)
// ---------------------------------------------------------------------------

#[test]
fn test_apply_policy() {
    let (env, client, admin, _account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    assert_eq!(client.get_rule_count(), 1);
    assert_eq!(client.get_remaining_budget(&0u32), 10_000_000);
}

#[test]
fn test_authorized_call_under_cap() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method, 5_000_000, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 5_000_000);
}

#[test]
fn test_denied_call_over_cap() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert_eq!(
        try_auth(&env, &account, &vendor, method, 15_000_000, 0).unwrap_err(),
        5
    );
    assert_eq!(client.get_remaining_budget(&0u32), 10_000_000);
}

#[test]
fn test_double_initialize_fails() {
    let (_env, client, admin, _account) = setup();
    assert!(client.try_initialize(&admin).is_err());
}

#[test]
fn test_empty_policy_fails() {
    let (env, client, admin, _account) = setup();
    // Both lists empty → EmptyPolicy error
    let spec = PolicySpec {
        allowed_contracts: Vec::new(&env),
        allowed_payees: Vec::new(&env),
        period_ledgers: 17280,
        agent_signer: None,
    };
    assert!(client.try_apply_policy(&admin, &spec).is_err());
}

#[test]
fn test_cumulative_spending() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method.clone(), 3_000_000, 0).is_ok());
    assert!(try_auth(&env, &account, &vendor, method.clone(), 4_000_000, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 3_000_000);
    assert!(try_auth(&env, &account, &vendor, method, 4_000_000, 0).is_err());
    assert_eq!(client.get_remaining_budget(&0u32), 3_000_000);
}

#[test]
fn test_rolling_window_reset() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_sequence_number(100);
    let policy = env.register(SpendPolicyContract, ());
    let account = env.register(AgentAccountContract, ());
    let client = AgentAccountContractClient::new(&env, &account);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    client.set_spend_policy(&admin, &policy);
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 100, 100));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method.clone(), 60, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 40);
    assert!(try_auth(&env, &account, &vendor, method.clone(), 50, 0).is_err());
    env.ledger().set_sequence_number(201);
    assert_eq!(client.get_remaining_budget(&0u32), 100);
    assert!(try_auth(&env, &account, &vendor, method, 80, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 20);
}

#[test]
fn test_denied_call_over_rate_limit() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(
        &admin,
        &sample_policy_calls(&env, &vendor, 10_000_000, 17280, 2),
    );
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method.clone(), 100, 0).is_ok());
    assert!(try_auth(&env, &account, &vendor, method.clone(), 100, 0).is_ok());
    assert_eq!(
        try_auth(&env, &account, &vendor, method, 100, 0).unwrap_err(),
        6
    );
}

#[test]
fn test_scoping_per_vendor() {
    let (env, client, admin, account) = setup();
    let vendor_a = Address::generate(&env);
    let vendor_b = Address::generate(&env);
    let method_a = Symbol::new(&env, "get_data_a");
    let method_b = Symbol::new(&env, "get_data_b");

    let mut contracts: Vec<AllowedContract> = Vec::new(&env);
    contracts.push_back(AllowedContract {
        contract_id: vendor_a.clone(),
        allowed_methods: vec![&env, method_a.clone()],
        max_spend_per_period: 10_000_000,
        max_calls_per_period: 100,
    });
    contracts.push_back(AllowedContract {
        contract_id: vendor_b.clone(),
        allowed_methods: vec![&env, method_b.clone()],
        max_spend_per_period: 5_000_000,
        max_calls_per_period: 100,
    });
    client.apply_policy(
        &admin,
        &PolicySpec {
            allowed_contracts: contracts,
            allowed_payees: empty_payees(&env),
            period_ledgers: 17280,
            agent_signer: None,
        },
    );
    assert_eq!(client.get_rule_count(), 2);

    // Vendor B with vendor A's rule → context type / policy deny
    assert!(try_auth(&env, &account, &vendor_b, method_a.clone(), 100, 0).is_err());
    // Vendor A contract with B's method on A's rule
    assert!(try_auth(&env, &account, &vendor_a, method_b.clone(), 100, 0).is_err());
    assert!(try_auth(&env, &account, &vendor_a, method_a, 100, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 9_999_900);
    assert_eq!(client.get_remaining_budget(&1u32), 5_000_000);
}

#[test]
fn test_auth_decision_events_approved_and_denied() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 100, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method.clone(), 10, 0).is_ok());
    assert!(has_auth_reason(&env, "approved"));
    assert_eq!(
        try_auth(&env, &account, &vendor, method, 200, 0).unwrap_err(),
        5
    );
}

#[test]
fn test_agent_guard_allows_then_spend() {
    let (env, client, admin, account) = setup();
    let guard = env.register(MockAgentGuard, ());
    client.set_agent_guard(&admin, &guard, &Role::Basic);
    assert!(client.get_agent_guard().is_some());
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method, 1_000_000, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 9_000_000);
}

#[test]
fn test_agent_guard_denies_before_spend() {
    let (env, client, admin, account) = setup();
    let guard = env.register(DenyingAgentGuard, ());
    client.set_agent_guard(&admin, &guard, &Role::Premium);
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert_eq!(
        try_auth(&env, &account, &vendor, method, 1_000_000, 0).unwrap_err(),
        8
    );
    assert_eq!(client.get_remaining_budget(&0u32), 10_000_000);
}

#[test]
fn test_replace_policy_resets_budget_to_new_spec() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 100, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method.clone(), 40, 0).is_ok());
    client.replace_policy(&admin, &sample_policy(&env, &vendor, 50, 17280));
    assert_eq!(client.get_rule_count(), 1);
    let rule_id = client.get_last_rule_id().unwrap();
    assert_eq!(client.get_remaining_budget(&rule_id), 50);
    assert!(try_auth(&env, &account, &vendor, method, 40, rule_id).is_ok());
    assert_eq!(client.get_remaining_budget(&rule_id), 10);
}

#[test]
fn test_revoke_policy_rejects_stale_rule() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 100, 17280));
    client.revoke_policy(&admin);
    assert_eq!(client.get_rule_count(), 0);
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method, 1, 0).is_err());
}

#[test]
fn test_agent_signer_is_on_the_rule_and_can_auth() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    let agent = Address::generate(&env);
    let mut spec = sample_policy(&env, &vendor, 1_000, 17280);
    spec.agent_signer = Some(agent.clone());
    client.apply_policy(&admin, &spec);
    let rule_id = client.get_last_rule_id().unwrap();
    assert_eq!(client.rule_signer_count(&rule_id), 1);

    let method = Symbol::new(&env, "get_data");
    let mut args: Vec<Val> = Vec::new(&env);
    args.push_back(10i128.into_val(&env));
    let ctx = Context::Contract(ContractContext {
        contract: vendor.clone(),
        fn_name: method.clone(),
        args,
    });
    let mut proofs: Map<Signer, Bytes> = Map::new(&env);
    proofs.set(Signer::Delegated(agent.clone()), Bytes::new(&env));
    let payload = AuthPayload {
        signers: proofs,
        context_rule_ids: vec![&env, rule_id],
    };
    let hash = BytesN::<32>::from_array(&env, &[9u8; 32]);
    env.try_invoke_contract_check_auth::<stellar_accounts::smart_account::SmartAccountError>(
        &account,
        &hash,
        payload.into_val(&env),
        &vec![&env, ctx],
    )
    .unwrap();

    let other = Address::generate(&env);
    let mut args: Vec<Val> = Vec::new(&env);
    args.push_back(10i128.into_val(&env));
    let ctx = Context::Contract(ContractContext {
        contract: vendor,
        fn_name: method,
        args,
    });
    let mut wrong: Map<Signer, Bytes> = Map::new(&env);
    wrong.set(Signer::Delegated(other), Bytes::new(&env));
    let payload = AuthPayload {
        signers: wrong,
        context_rule_ids: vec![&env, rule_id],
    };
    assert!(env
        .try_invoke_contract_check_auth::<stellar_accounts::smart_account::SmartAccountError>(
            &account,
            &hash,
            payload.into_val(&env),
            &vec![&env, ctx],
        )
        .is_err());
}

// ---------------------------------------------------------------------------
// Payee rule tests
// ---------------------------------------------------------------------------

/// Build a PolicySpec with a single payee rule and no legacy contracts.
fn payee_policy(
    env: &Env,
    asset: &Address,
    pay_to: &Address,
    cap: i128,
    period: u32,
    max_calls: u32,
) -> PolicySpec {
    let mut payees: Vec<AllowedPayee> = Vec::new(env);
    payees.push_back(AllowedPayee {
        asset: asset.clone(),
        pay_to: pay_to.clone(),
        max_spend_per_period: cap,
        max_calls_per_period: max_calls,
    });
    PolicySpec {
        allowed_contracts: Vec::new(env),
        allowed_payees: payees,
        period_ledgers: period,
        agent_signer: None,
    }
}

#[test]
fn test_payee_allow_under_cap() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    let ctx = transfer_ctx(&env, &asset, &account, &payee, 500);
    assert!(try_auth_ctx(&env, &account, ctx, rule_id).is_ok());
    assert_eq!(client.get_remaining_budget(&rule_id), 9_500);
}

#[test]
fn test_payee_deny_wrong_recipient_same_asset() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);
    let other = Address::generate(&env); // wrong recipient

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    // Transfer to `other` must be denied even though the asset matches
    let ctx = transfer_ctx(&env, &asset, &account, &other, 500);
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
                        // Budget must be unchanged
    assert_eq!(client.get_remaining_budget(&rule_id), 10_000);
}

#[test]
fn test_payee_deny_wrong_function_approve() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    // `approve` is not `transfer` → InvalidContext
    let mut args: Vec<Val> = Vec::new(&env);
    args.push_back(account.into_val(&env));
    args.push_back(payee.into_val(&env));
    args.push_back(500i128.into_val(&env));
    let ctx = Context::Contract(ContractContext {
        contract: asset.clone(),
        fn_name: Symbol::new(&env, "approve"),
        args,
    });
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
}

#[test]
fn test_payee_deny_wrong_function_transfer_from() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    let mut args: Vec<Val> = Vec::new(&env);
    args.push_back(account.into_val(&env));
    args.push_back(payee.into_val(&env));
    args.push_back(500i128.into_val(&env));
    let ctx = Context::Contract(ContractContext {
        contract: asset.clone(),
        fn_name: Symbol::new(&env, "transfer_from"),
        args,
    });
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
}

#[test]
fn test_payee_deny_over_cap() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 1_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    let ctx = transfer_ctx(&env, &asset, &account, &payee, 2_000);
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 5); // OverBudget
    assert_eq!(client.get_remaining_budget(&rule_id), 1_000);
}

#[test]
fn test_payee_deny_rate_limited() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 100_000, 17280, 2),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 100),
        rule_id
    )
    .is_ok());
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 100),
        rule_id
    )
    .is_ok());
    let err = try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 100),
        rule_id,
    )
    .unwrap_err();
    assert_eq!(err, 6); // RateLimited
}

#[test]
fn test_payee_deny_zero_amount() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    let ctx = transfer_ctx(&env, &asset, &account, &payee, 0);
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
}

#[test]
fn test_payee_deny_negative_amount() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    let ctx = transfer_ctx(&env, &asset, &account, &payee, -1);
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
}

#[test]
fn test_payee_deny_malformed_args_too_few() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 10_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    // Only 2 args instead of 3
    let mut args: Vec<Val> = Vec::new(&env);
    args.push_back(account.into_val(&env));
    args.push_back(payee.into_val(&env));
    let ctx = Context::Contract(ContractContext {
        contract: asset.clone(),
        fn_name: Symbol::new(&env, "transfer"),
        args,
    });
    let err = try_auth_ctx(&env, &account, ctx, rule_id).unwrap_err();
    assert_eq!(err, 7); // InvalidContext
}

#[test]
fn test_payee_rolling_window_reset() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_sequence_number(100);
    let policy = env.register(SpendPolicyContract, ());
    let account = env.register(AgentAccountContract, ());
    let client = AgentAccountContractClient::new(&env, &account);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    client.set_spend_policy(&admin, &policy);

    let asset = Address::generate(&env);
    let payee = Address::generate(&env);
    client.apply_policy(&admin, &payee_policy(&env, &asset, &payee, 1_000, 100, 100));
    let rule_id = client.get_last_rule_id().unwrap();

    // First transfer — OK
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 700),
        rule_id
    )
    .is_ok());
    assert_eq!(client.get_remaining_budget(&rule_id), 300);

    // Over budget before reset
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 500),
        rule_id
    )
    .is_err());

    // Advance past the window
    env.ledger().set_sequence_number(201);
    assert_eq!(client.get_remaining_budget(&rule_id), 1_000);

    // After reset — succeeds again
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 800),
        rule_id
    )
    .is_ok());
    assert_eq!(client.get_remaining_budget(&rule_id), 200);
}

#[test]
fn test_two_payees_same_asset_independent_budgets() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee_a = Address::generate(&env);
    let payee_b = Address::generate(&env);

    let mut payees: Vec<AllowedPayee> = Vec::new(&env);
    payees.push_back(AllowedPayee {
        asset: asset.clone(),
        pay_to: payee_a.clone(),
        max_spend_per_period: 1_000,
        max_calls_per_period: 10,
    });
    payees.push_back(AllowedPayee {
        asset: asset.clone(),
        pay_to: payee_b.clone(),
        max_spend_per_period: 5_000,
        max_calls_per_period: 10,
    });
    client.apply_policy(
        &admin,
        &PolicySpec {
            allowed_contracts: Vec::new(&env),
            allowed_payees: payees,
            period_ledgers: 17280,
            agent_signer: None,
        },
    );
    assert_eq!(client.get_rule_count(), 2);
    // rule 0 = payee_a, rule 1 = payee_b
    let rule_a = 0u32;
    let rule_b = 1u32;

    // Spend from payee_a budget
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee_a, 600),
        rule_a
    )
    .is_ok());
    assert_eq!(client.get_remaining_budget(&rule_a), 400);
    // payee_b budget is untouched
    assert_eq!(client.get_remaining_budget(&rule_b), 5_000);

    // payee_a over cap
    assert_eq!(
        try_auth_ctx(
            &env,
            &account,
            transfer_ctx(&env, &asset, &account, &payee_a, 600),
            rule_a
        )
        .unwrap_err(),
        5 // OverBudget
    );

    // payee_b still works
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee_b, 3_000),
        rule_b
    )
    .is_ok());
    assert_eq!(client.get_remaining_budget(&rule_b), 2_000);

    // Cross-recipient: payee_b address on rule_a → InvalidContext
    assert_eq!(
        try_auth_ctx(
            &env,
            &account,
            transfer_ctx(&env, &asset, &account, &payee_b, 100),
            rule_a
        )
        .unwrap_err(),
        7 // InvalidContext
    );
}

#[test]
fn test_legacy_allowed_contract_still_works() {
    let (env, client, admin, account) = setup();
    let vendor = Address::generate(&env);
    // Pure legacy policy — no allowed_payees
    client.apply_policy(&admin, &sample_policy(&env, &vendor, 10_000_000, 17280));
    let method = Symbol::new(&env, "get_data");
    assert!(try_auth(&env, &account, &vendor, method, 5_000_000, 0).is_ok());
    assert_eq!(client.get_remaining_budget(&0u32), 5_000_000);
}

#[test]
fn test_auth_decision_events_payee_approve_and_deny() {
    let (env, client, admin, account) = setup();
    let asset = Address::generate(&env);
    let payee = Address::generate(&env);

    client.apply_policy(
        &admin,
        &payee_policy(&env, &asset, &payee, 1_000, 17280, 100),
    );
    let rule_id = client.get_last_rule_id().unwrap();

    // Approved event
    assert!(try_auth_ctx(
        &env,
        &account,
        transfer_ctx(&env, &asset, &account, &payee, 100),
        rule_id
    )
    .is_ok());
    assert!(has_auth_reason(&env, "approved"));

    // Denied (over cap) event
    assert_eq!(
        try_auth_ctx(
            &env,
            &account,
            transfer_ctx(&env, &asset, &account, &payee, 2_000),
            rule_id
        )
        .unwrap_err(),
        5
    );
    assert!(has_auth_reason(&env, "over_budget"));

    // Denied (wrong payee) → invalid_context event
    let other = Address::generate(&env);
    assert_eq!(
        try_auth_ctx(
            &env,
            &account,
            transfer_ctx(&env, &asset, &account, &other, 100),
            rule_id
        )
        .unwrap_err(),
        7
    );
    assert!(has_auth_reason(&env, "invalid_context"));
}
