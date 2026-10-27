"""Write the deploy_testnet.sh JSON report.

Dry-run mode never contacts Friendbot or RPC. The hash is a SHA-256 of a
fixed label so local and CI checks can assert a 64-character hex id.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def deterministic_tx_hash(seed: str = "agentpay-dry-run") -> str:
    return hashlib.sha256(seed.encode()).hexdigest()


def build_report(
    *,
    network: str,
    mode: str,
    address: str = "",
    contract_id: str = "",
    policy_id: str = "",
    tx_hash: str = "",
) -> dict:
    if mode == "dry-run" and not tx_hash:
        tx_hash = deterministic_tx_hash()
    if tx_hash and (len(tx_hash) != 64 or any(c not in "0123456789abcdef" for c in tx_hash)):
        raise ValueError("tx_hash must be 64 lowercase hex characters")
    return {
        "network": network,
        "mode": mode,
        "address": address,
        "contract_id": contract_id,
        "policy_id": policy_id,
        "tx_hash": tx_hash,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--network", default="testnet")
    parser.add_argument("--address", default="")
    parser.add_argument("--contract-id", default="")
    parser.add_argument("--policy-id", default="")
    parser.add_argument("--tx-hash", default="")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    report = build_report(
        network=args.network,
        mode="dry-run" if args.dry_run else "live",
        address=args.address,
        contract_id=args.contract_id,
        policy_id=args.policy_id,
        tx_hash=args.tx_hash,
    )
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"tx_hash": report["tx_hash"], "mode": report["mode"]}))


if __name__ == "__main__":
    main()
