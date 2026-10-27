from datetime import datetime, timedelta, timezone
from typing import Any, Dict

import pytest

from generate_policy import PolicyInputError, score_transactions
from schema import PolicySpec


def _ts(base: datetime, minutes: int) -> str:
    return (base + timedelta(minutes=minutes)).isoformat()


BASE = datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)


def test_score_transactions_outlier():
    base_time = datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)

    # 20 steady volume transactions
    tx_log = []
    for i in range(20):
        tx_log.append(
            {
                "contract_id": "C_STEADY_VENDOR",
                "method": "buy",
                "amount": 10,
                "timestamp": (base_time + timedelta(minutes=i)).isoformat(),
            }
        )

    # 1 outlier spike
    tx_log.append(
        {
            "contract_id": "C_STEADY_VENDOR",
            "method": "buy",
            "amount": 1000,
            "timestamp": (base_time + timedelta(minutes=20)).isoformat(),
        }
    )

    policy = score_transactions(tx_log)

    assert len(policy.allowed_contracts) == 1
    contract_policy = policy.allowed_contracts[0]

    assert contract_policy.contract_id == "C_STEADY_VENDOR"

    # The p95 of [10]*20 + [1000] should be 10.
    # With a safety margin of 1.5, the cap should be 15.
    assert contract_policy.max_spend_per_period >= 10
    assert contract_policy.max_spend_per_period < 1000
    assert contract_policy.max_spend_per_period == 15

    # Rate limit check:
    # 21 calls in 20 minutes (1200 seconds).
    # span_ledgers = 1200 / 5 = 240
    # period_ledgers for 1200 seconds is 1200/5 * 1.2 = 288. max(720, 288) = 720
    # rate_multiplier = 720 / 240 = 3
    # base_calls = 21, max_calls = 21 * 3 * 1.5 = 94
    assert contract_policy.max_calls_per_period >= 21


def test_empty_log_rejected():
    with pytest.raises(PolicyInputError, match="empty"):
        score_transactions([])


def test_missing_fields_rejected():
    with pytest.raises(PolicyInputError, match="missing"):
        score_transactions([{"amount": 1}])


# ---------------------------------------------------------------------------
# Payee tests
# ---------------------------------------------------------------------------


def _payee_tx(asset: str, pay_to: str, amount: int, minutes: int) -> Dict[str, Any]:
    """Return a payee-style transaction record."""
    return {
        "contract_id": asset,  # kept for backward compat with validate_tx_log
        "method": "transfer",
        "asset": asset,
        "pay_to": pay_to,
        "amount": amount,
        "timestamp": _ts(BASE, minutes),
    }


ASSET_A = "CDEMO5TOKN1ASSET1ADDRESSXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
ASSET_B = "CDEMO5TOKN2ASSET2ADDRESSXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
PAYEE_1 = "GDEMO5VENDOR1PAYEEADDRESXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
PAYEE_2 = "GDEMO5VENDOR2PAYEEADDRESXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"


def test_payee_txs_produce_allowed_payees():
    """Txs with asset+pay_to go to allowed_payees, not allowed_contracts."""
    tx_log = [_payee_tx(ASSET_A, PAYEE_1, 100, i) for i in range(10)]
    policy = score_transactions(tx_log)

    assert len(policy.allowed_contracts) == 0, "No legacy contracts expected"
    assert len(policy.allowed_payees) == 1

    p = policy.allowed_payees[0]
    assert p.asset == ASSET_A
    assert p.pay_to == PAYEE_1
    assert p.max_spend_per_period >= 100  # p95 × 1.5
    assert p.max_calls_per_period >= 1


def test_two_payees_same_asset_independent():
    """Two payees on the same token asset get separate entries."""
    tx_log = [_payee_tx(ASSET_A, PAYEE_1, 100, i) for i in range(5)] + [
        _payee_tx(ASSET_A, PAYEE_2, 500, i + 5) for i in range(5)
    ]
    policy = score_transactions(tx_log)

    assert len(policy.allowed_payees) == 2

    by_payee = {p.pay_to: p for p in policy.allowed_payees}
    assert PAYEE_1 in by_payee
    assert PAYEE_2 in by_payee

    # Caps are independent
    assert by_payee[PAYEE_1].max_spend_per_period < by_payee[PAYEE_2].max_spend_per_period


def test_mixed_log_produces_both_lists():
    """A log with both legacy and payee txs populates both lists."""
    tx_log = [
        {
            "contract_id": "C_LEGACY",
            "method": "get_data",
            "amount": 50,
            "timestamp": _ts(BASE, i),
        }
        for i in range(10)
    ] + [_payee_tx(ASSET_A, PAYEE_1, 200, i + 10) for i in range(10)]

    policy = score_transactions(tx_log)

    assert len(policy.allowed_contracts) == 1
    assert policy.allowed_contracts[0].contract_id == "C_LEGACY"

    assert len(policy.allowed_payees) == 1
    assert policy.allowed_payees[0].asset == ASSET_A

    # Total tx count
    assert policy.source_tx_count == 20


def test_payee_cap_uses_p95_times_safety_margin():
    """Cap = floor(p95 × 1.5), outliers clipped at p95."""
    # 20 txs at 100, 1 outlier at 10_000 (21 total so (21-1)*0.95 = 19.0 integer index)
    tx_log = [_payee_tx(ASSET_A, PAYEE_1, 100, i) for i in range(20)] + [
        _payee_tx(ASSET_A, PAYEE_1, 10_000, 20)
    ]
    policy = score_transactions(tx_log)
    assert len(policy.allowed_payees) == 1
    p = policy.allowed_payees[0]
    # p95 of 19×100 + 1×10000 ≈ 100; cap = 150
    assert p.max_spend_per_period == 150


def test_policy_validates_against_schema():
    """The output of score_transactions is a valid PolicySpec."""
    tx_log = [_payee_tx(ASSET_A, PAYEE_1, 100, i) for i in range(5)]
    policy = score_transactions(tx_log)
    # Round-trip through JSON and back
    json_str = policy.model_dump_json()
    reparsed = PolicySpec.model_validate_json(json_str)
    assert len(reparsed.allowed_payees) == 1
    assert reparsed.allowed_payees[0].asset == ASSET_A


def test_simulate_produces_payee_txs():
    """simulate_agent.generate_transaction_log emits asset+pay_to entries."""
    import sys
    from pathlib import Path

    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "scripts"))
    from simulate_agent import generate_transaction_log

    tx_log = generate_transaction_log(num_transactions=75, seed=42)
    payee_txs = [tx for tx in tx_log if tx.get("asset") and tx.get("pay_to")]
    legacy_txs = [tx for tx in tx_log if not tx.get("asset")]

    assert len(payee_txs) > 0, "Expected some x402 payee transactions"
    assert len(legacy_txs) > 0, "Expected some legacy contract transactions"


def test_simulate_log_produces_payees_in_policy():
    """The full pipeline: simulate → score returns allowed_payees."""
    import sys
    from pathlib import Path

    sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "scripts"))
    from simulate_agent import generate_transaction_log

    tx_log = generate_transaction_log(num_transactions=75, seed=42)
    policy = score_transactions(tx_log)

    assert len(policy.allowed_payees) > 0, "Policy should contain allowed_payees from simulation"
    assert all(p.max_spend_per_period > 0 for p in policy.allowed_payees), (
        "Every payee cap must be positive"
    )
