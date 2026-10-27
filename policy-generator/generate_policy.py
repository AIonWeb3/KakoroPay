"""
Policy Generator — Rule-Based Heuristic

Reads a JSON transaction log and produces a minimal least-privilege
PolicySpec (allowed contract/method pairs + per-period spend caps).

The heuristic:
1. Extract unique (contract_id, method) pairs from the log.
   If a tx has ``asset`` and ``pay_to`` fields it is treated as an x402
   SEP-41 transfer and grouped into ``allowed_payees`` instead.
2. For each group, compute the p95 of observed amounts × 1.5 safety
   margin as the spend cap.
3. Estimate the period from the timestamp range of the log.

# TODO: Swap heuristic for LLM summarizer
# The generate_policy() function has a clear seam: replace the body with
# an LLM call that takes the transaction log and returns a PolicySpec.
# The schema and CLI interface remain unchanged.
"""

from __future__ import annotations

import json
import math
import sys
from collections import defaultdict
from datetime import datetime
from typing import Any, Dict, List

from schema import AllowedContract, AllowedMethod, AllowedPayee, PolicySpec


class PolicyInputError(ValueError):
    """Raised when a transaction log cannot be turned into a PolicySpec."""


REQUIRED_TX_FIELDS = ("contract_id", "method")


def validate_tx_log(tx_log: List[Dict[str, Any]]) -> None:
    if not isinstance(tx_log, list):
        raise PolicyInputError("transaction log must be a JSON array")
    if not tx_log:
        raise PolicyInputError("empty transaction log")
    for i, tx in enumerate(tx_log):
        if not isinstance(tx, dict):
            raise PolicyInputError(f"entry {i} is not an object")
        missing = [f for f in REQUIRED_TX_FIELDS if not tx.get(f)]
        if missing:
            raise PolicyInputError(f"entry {i} missing fields: {', '.join(missing)}")


# ---------------------------------------------------------------------------
# Transaction log types
# ---------------------------------------------------------------------------

# Legacy format (list of):
# {
#   "contract_id": "C...",
#   "method": "transfer",
#   "amount": 1000,
#   "timestamp": "2025-01-15T10:30:00Z"
# }
#
# x402 / SEP-41 payee format (list of):
# {
#   "contract_id": "<vendor contract>",   (kept for backward compat)
#   "method": "transfer",
#   "asset": "C...",                      (token contract)
#   "pay_to": "G...",                     (recipient)
#   "amount": 1000,
#   "timestamp": "2025-01-15T10:30:00Z"
# }
#
# When both ``asset`` and ``pay_to`` are present the tx is routed to the
# payee grouping; otherwise it is treated as a legacy AllowedContract tx.


def percentile(data: List[float], p: float) -> float:
    """Compute the p-th percentile of a list of numbers."""
    if not data:
        return 0.0
    sorted_data = sorted(data)
    k = (len(sorted_data) - 1) * (p / 100.0)
    f = math.floor(k)
    c = math.ceil(k)
    if f == c:
        return sorted_data[int(k)]
    return sorted_data[f] * (c - k) + sorted_data[c] * (k - f)


def estimate_period_ledgers(timestamps: List[str]) -> int:
    """
    Estimate the period in ledgers from the timestamp range.

    Assumes ~5 seconds per ledger on Stellar.
    Default: 17280 ledgers ≈ 1 day.
    """
    if len(timestamps) < 2:
        return 17280  # Default: 1 day

    dts = sorted(datetime.fromisoformat(ts.replace("Z", "+00:00")) for ts in timestamps)
    span_seconds = (dts[-1] - dts[0]).total_seconds()

    # Convert to ledgers (5 seconds per ledger), add 20% buffer
    ledgers = int((span_seconds / 5.0) * 1.2)

    # Clamp to reasonable range: minimum 1 hour, maximum 7 days
    return max(720, min(ledgers, 120960))


def score_transactions(
    tx_log: List[Dict[str, Any]],
    safety_margin: float = 1.5,
    percentile_threshold: float = 95.0,
) -> PolicySpec:
    """
    Generate a minimal least-privilege PolicySpec from a transaction log.

    Txs with both ``asset`` and ``pay_to`` fields are emitted as
    ``AllowedPayee`` entries (keyed by (asset, pay_to)).  All other txs
    follow the legacy ``AllowedContract`` path (keyed by contract_id).
    """
    validate_tx_log(tx_log)

    # Legacy grouping: contract_id → {methods, amounts}
    by_contract: Dict[str, Dict[str, Any]] = defaultdict(lambda: {"methods": set(), "amounts": []})
    # Payee grouping: (asset, pay_to) → {amounts}
    by_payee: Dict[tuple, Dict[str, Any]] = defaultdict(lambda: {"amounts": []})

    timestamps = []

    for tx in tx_log:
        asset = tx.get("asset", "")
        pay_to = tx.get("pay_to", "")
        amount = tx.get("amount", 0)
        timestamp = tx.get("timestamp", "")

        if asset and pay_to:
            # x402 / SEP-41 payee tx
            key = (asset, pay_to)
            by_payee[key]["amounts"].append(float(amount))
        else:
            # Legacy contract tx
            contract_id = tx["contract_id"]
            method = tx["method"]
            by_contract[contract_id]["methods"].add(method)
            by_contract[contract_id]["amounts"].append(float(amount))

        if timestamp:
            timestamps.append(timestamp)

    # Estimate the period from timestamp range
    period_ledgers = estimate_period_ledgers(timestamps)

    # Determine rate limit scaling factor
    dts = (
        sorted(datetime.fromisoformat(ts.replace("Z", "+00:00")) for ts in timestamps)
        if timestamps
        else []
    )
    span_seconds = (dts[-1] - dts[0]).total_seconds() if len(dts) >= 2 else 0
    span_ledgers = max(1, int(span_seconds / 5.0))
    rate_multiplier = (period_ledgers / span_ledgers) if span_ledgers > 0 else 1.0

    # -----------------------------------------------------------------------
    # Build allowed_contracts (legacy)
    # -----------------------------------------------------------------------
    allowed_contracts: List[AllowedContract] = []

    for contract_id, data in by_contract.items():
        p95 = percentile(data["amounts"], percentile_threshold)
        cap = int(p95 * safety_margin)
        cap = max(cap, 1)
        base_calls = len(data["amounts"])
        max_calls = max(1, int(base_calls * rate_multiplier * safety_margin))

        allowed_contracts.append(
            AllowedContract(
                contract_id=contract_id,
                allowed_methods=[AllowedMethod(name=m) for m in sorted(data["methods"])],
                max_spend_per_period=cap,
                max_calls_per_period=max_calls,
            )
        )

    # Sort by contract_id for deterministic output
    allowed_contracts.sort(key=lambda c: c.contract_id)

    # -----------------------------------------------------------------------
    # Build allowed_payees (x402 / SEP-41)
    # -----------------------------------------------------------------------
    allowed_payees: List[AllowedPayee] = []

    for (asset, pay_to), data in by_payee.items():
        p95 = percentile(data["amounts"], percentile_threshold)
        cap = int(p95 * safety_margin)
        cap = max(cap, 1)
        base_calls = len(data["amounts"])
        max_calls = max(1, int(base_calls * rate_multiplier * safety_margin))

        allowed_payees.append(
            AllowedPayee(
                asset=asset,
                pay_to=pay_to,
                max_spend_per_period=cap,
                max_calls_per_period=max_calls,
            )
        )

    # Sort by (asset, pay_to) for deterministic output
    allowed_payees.sort(key=lambda p: (p.asset, p.pay_to))

    agent_signer = None
    for tx in tx_log:
        key = tx.get("agent_pubkey") or tx.get("agent_signer")
        if key:
            agent_signer = str(key)
            break

    return PolicySpec(
        allowed_contracts=allowed_contracts,
        allowed_payees=allowed_payees,
        period_ledgers=period_ledgers,
        source_tx_count=len(tx_log),
        agent_signer=agent_signer,
    )


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def main() -> None:
    """
    CLI: reads a JSON transaction log from stdin or a file argument,
    outputs a PolicySpec JSON to stdout.

    Usage:
        python generate_policy.py < transaction_log.json
        python generate_policy.py transaction_log.json
        python generate_policy.py --from-chain <ACCOUNT_ID> --events events.json
    """
    args = sys.argv[1:]
    if args and args[0] == "--from-chain":
        if len(args) < 2:
            raise SystemExit("usage: generate_policy.py --from-chain ACCOUNT_ID --events FILE")
        account_id = args[1]
        events_path = None
        if "--events" in args:
            events_path = args[args.index("--events") + 1]
        if not events_path:
            raise SystemExit(
                "--from-chain needs --events FILE (offline). Live RPC is not called from CI."
            )
        with open(events_path) as f:
            payload = json.load(f)
        if isinstance(payload, dict) and "account_id" not in payload:
            payload["account_id"] = account_id
        from chain import generate_policy_from_chain

        policy = generate_policy_from_chain(payload)
    elif args:
        with open(args[0]) as f:
            tx_log = json.load(f)
        policy = score_transactions(tx_log)
    else:
        tx_log = json.load(sys.stdin)
        policy = score_transactions(tx_log)

    print(policy.model_dump_json(indent=2))


if __name__ == "__main__":
    main()
