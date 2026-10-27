#!/usr/bin/env bash
# deploy_testnet.sh — Simulate → generate policy → deploy → initialize → apply_policy
#
# Usage:
#   ./scripts/deploy_testnet.sh
#
# Optional env:
#   NETWORK=testnet|local
#   STELLAR_IDENTITY  (reuse an existing identity instead of generating one)
#
# No secrets are committed. Generated identities are ephemeral unless you set STELLAR_IDENTITY.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

NETWORK="${NETWORK:-testnet}"
REPORT="${DEPLOY_REPORT:-$ROOT/deploy_report.json}"
if [ "${AGENTPAY_DRY_RUN:-}" = "1" ]; then
    python "$ROOT/scripts/testnet_report.py" --dry-run --network "$NETWORK" --out "$REPORT"
    exit 0
fi
IDENTITY="${STELLAR_IDENTITY:-agentpay-deployer-$(date +%s)}"
TX_LOG="${TX_LOG:-$ROOT/tx_log.json}"
POLICY_JSON="${POLICY_JSON:-$ROOT/policy.json}"
POLICY_ONCHAIN="${POLICY_ONCHAIN:-$ROOT/policy.onchain.json}"

echo "=== AgentPay Testnet Deployment ==="
echo ""

# -----------------------------------------------------------------------
# 0. Simulate + generate PolicySpec
# -----------------------------------------------------------------------
echo "[0/7] simulate_agent.py → $TX_LOG"
python "$ROOT/scripts/simulate_agent.py" > "$TX_LOG"
echo "[0/7] generate_policy.py → $POLICY_JSON"
python "$ROOT/policy-generator/generate_policy.py" "$TX_LOG" > "$POLICY_JSON"

python - "$POLICY_JSON" "$POLICY_ONCHAIN" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
p = json.load(open(src))
# Convert allowed_contracts (legacy path)
contracts = [
    {
        "contract_id": c["contract_id"],
        "allowed_methods": [
            m["name"] if isinstance(m, dict) else m for m in c["allowed_methods"]
        ],
        "max_spend_per_period": c["max_spend_per_period"],
        "max_calls_per_period": c["max_calls_per_period"],
    }
    for c in p.get("allowed_contracts", [])
]
# Convert allowed_payees (x402 / SEP-41 payee rules)
# Storage layout change: each AllowedPayee creates a CallContract(asset)
# context rule with a PayTo binding stored at PolicyKey::PayTo(account, rule_id).
payees = [
    {
        "asset":                 ap["asset"],
        "pay_to":                ap["pay_to"],
        "max_spend_per_period":  ap["max_spend_per_period"],
        "max_calls_per_period":  ap["max_calls_per_period"],
    }
    for ap in p.get("allowed_payees", [])
]
out = {
    "allowed_contracts": contracts,
    "allowed_payees":    payees,
    "period_ledgers":    p["period_ledgers"],
}
json.dump(out, open(dst, "w"))
print(f"      on-chain spec: {dst}  ({len(contracts)} contracts, {len(payees)} payees)")
PY

# -----------------------------------------------------------------------
# 1. Generate a throwaway identity
# -----------------------------------------------------------------------
echo "[1/7] Identity: $IDENTITY"
if ! stellar keys address "$IDENTITY" >/dev/null 2>&1; then
    stellar keys generate "$IDENTITY" --network "$NETWORK"
fi
ADDRESS=$(stellar keys address "$IDENTITY")
echo "      Address: $ADDRESS"

# -----------------------------------------------------------------------
# 2. Fund via Friendbot (testnet)
# -----------------------------------------------------------------------
if [ "$NETWORK" = "testnet" ]; then
    echo "[2/7] Funding via Friendbot..."
    curl -s "https://friendbot.stellar.org?addr=$ADDRESS" > /dev/null
    echo "      Funded ✓"
else
    echo "[2/7] Skipping Friendbot (NETWORK=$NETWORK)"
fi

# -----------------------------------------------------------------------
# 3. Build the contract
# -----------------------------------------------------------------------
echo "[3/7] Building agent-account contract..."
stellar contract build --manifest-path contracts/agent-account/Cargo.toml
WASM_PATH="target/wasm32v1-none/release/agent_account.wasm"
if [ ! -f "$WASM_PATH" ]; then
    WASM_PATH=$(find target -name "agent_account.wasm" -type f | head -1)
    if [ -z "$WASM_PATH" ]; then
        echo "ERROR: Could not find compiled WASM. Aborting."
        exit 1
    fi
fi
echo "      WASM: $WASM_PATH"

# -----------------------------------------------------------------------
# 4. Deploy account + spend-policy instances (same WASM, two IDs)
# -----------------------------------------------------------------------
echo "[4/7] Deploying smart account and spend policy..."
CONTRACT_ID=$(stellar contract deploy \
    --wasm "$WASM_PATH" \
    --source "$IDENTITY" \
    --network "$NETWORK")
POLICY_ID=$(stellar contract deploy \
    --wasm "$WASM_PATH" \
    --source "$IDENTITY" \
    --network "$NETWORK")
echo "      Account: $CONTRACT_ID"
echo "      Policy:  $POLICY_ID"

# -----------------------------------------------------------------------
# 5. Initialize + bind policy
# -----------------------------------------------------------------------
echo "[5/7] initialize + set_spend_policy..."
stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source "$IDENTITY" \
    --network "$NETWORK" \
    -- \
    initialize \
    --admin "$ADDRESS"
stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source "$IDENTITY" \
    --network "$NETWORK" \
    -- \
    set_spend_policy \
    --admin "$ADDRESS" \
    --policy "$POLICY_ID"
echo "      Initialized ✓"

if [ -n "${AGENTGUARD_CONTRACT_ID:-}" ]; then
    echo "[5b/7] set_agent_guard (AGENTGUARD_CONTRACT_ID)..."
    stellar contract invoke \
        --id "$CONTRACT_ID" \
        --source "$IDENTITY" \
        --network "$NETWORK" \
        -- \
        set_agent_guard \
        --admin "$ADDRESS" \
        --guard "$AGENTGUARD_CONTRACT_ID" \
        --required_role "${AGENTGUARD_REQUIRED_ROLE:-Basic}"
    echo "      AgentGuard linked ✓"
fi

# -----------------------------------------------------------------------
# 6. apply_policy
# -----------------------------------------------------------------------
POLICY_ENTRY="apply_policy"
if [ "${AGENTPAY_REPLACE:-}" = "1" ]; then
    POLICY_ENTRY="replace_policy"
fi
echo "[6/7] $POLICY_ENTRY from generated spec..."
SPEC_ARG=$(cat "$POLICY_ONCHAIN")
INVOKE_OUT=$(stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source "$IDENTITY" \
    --network "$NETWORK" \
    -- \
    "$POLICY_ENTRY" \
    --admin "$ADDRESS" \
    --policy_spec "$SPEC_ARG")
echo "$INVOKE_OUT"
echo "      Policy applied ✓"

# -----------------------------------------------------------------------
# 7. Print operator commands
# -----------------------------------------------------------------------
ADMIN=$(stellar contract invoke --id "$CONTRACT_ID" --source "$IDENTITY" --network "$NETWORK" -- get_admin)
echo ""
echo "=== Deployment Complete ==="
echo "  Network:     $NETWORK"
echo "  Identity:    $IDENTITY"
echo "  Address:     $ADDRESS"
echo "  Contract ID: $CONTRACT_ID"
echo "  Policy ID:   $POLICY_ID"
echo "  AgentGuard:  ${AGENTGUARD_CONTRACT_ID:-<unset>}"
echo "  Admin:       $ADMIN"
echo "  Tx log:      $TX_LOG"
echo "  Policy JSON: $POLICY_JSON"
echo ""
echo "MCP env:"
echo "  export ACCOUNT_CONTRACT_ID=$CONTRACT_ID"
echo "  export STELLAR_IDENTITY=$IDENTITY"
echo "  export SOROBAN_RPC_URL=${SOROBAN_RPC_URL:-https://soroban-testnet.stellar.org}"
echo "  export STELLAR_NETWORK=$NETWORK"
echo "  cargo run -p mcp-server"
echo ""
echo "Tools (via any MCP client, or stellar CLI equivalent):"
echo "  discover_resources  — MCP tool (local registry)"
echo "  check_budget        — reads get_remaining_budget on-chain when ACCOUNT_CONTRACT_ID is set"
echo "  pay_and_call        — source-account invoke of the vendor through this smart account"
echo ""
echo "stellar contract invoke --id $CONTRACT_ID --source $IDENTITY --network $NETWORK -- get_remaining_budget --rule_id 0"
echo "stellar contract invoke --id $CONTRACT_ID --source $IDENTITY --network $NETWORK -- get_rule_count"
echo "stellar contract invoke --id $CONTRACT_ID --source $IDENTITY --network $NETWORK -- get_admin"
echo ""
echo "Authorized vendor call (triggers __check_auth + spend/rate policy):"
echo "  stellar contract invoke --id <VENDOR> --source-account $CONTRACT_ID --source $IDENTITY --network $NETWORK --send=yes -- <method> --amount <stroops>"
echo ""
echo "Read auth_decision / policy_applied events:"
echo "  stellar events --id $CONTRACT_ID --network $NETWORK"
echo "  stellar events --id $POLICY_ID --network $NETWORK"
echo ""
echo "Over-cap and non-allowlisted calls fail in __check_auth (contract errors OverBudget=5, RateLimited=6, InvalidContext=7, AgentGuardExecutionDenied=8)."

TX_HASH=$(python -c 'import re,sys; m=re.search(r"[0-9a-f]{64}", sys.stdin.read()); print(m.group(0) if m else "")' <<<"${INVOKE_OUT:-}")
python "$ROOT/scripts/testnet_report.py" \
    --out "$REPORT" \
    --network "$NETWORK" \
    --address "$ADDRESS" \
    --contract-id "$CONTRACT_ID" \
    --policy-id "$POLICY_ID" \
    --tx-hash "$TX_HASH"
echo "  Report:      $REPORT"
