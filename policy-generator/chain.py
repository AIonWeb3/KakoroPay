"""Turn Horizon/RPC-shaped event pages into a PolicySpec.

Live HTTP is optional. Tests and CI pass already-fetched pages into
`generate_policy_from_chain`. `fetch_event_pages` retries a caller-supplied
getter so pagination stays deterministic.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, List, Optional

from generate_policy import score_transactions
from schema import PolicySpec


def events_to_tx_log(payload: Any) -> List[Dict[str, Any]]:
    if isinstance(payload, dict):
        events = payload.get("events") or payload.get("records") or []
        account_id = payload.get("account_id")
    elif isinstance(payload, list):
        events = payload
        account_id = None
    else:
        raise ValueError("event payload must be a list or an object with events")

    tx_log: List[Dict[str, Any]] = []
    for ev in events:
        if not isinstance(ev, dict):
            continue
        contract_id = ev.get("contract_id") or ev.get("contractId")
        method = ev.get("method")
        topics = ev.get("topic") or ev.get("topics") or []
        if not method and topics:
            last = topics[-1]
            if isinstance(last, str):
                method = last
        if not contract_id or not method:
            continue
        tx_log.append(
            {
                "contract_id": contract_id,
                "method": method,
                "amount": ev.get("amount", 0),
                "timestamp": ev.get("timestamp") or ev.get("ledgerClosedAt") or "",
                "agent_pubkey": ev.get("source_account") or account_id,
            }
        )
    return tx_log


def generate_policy_from_chain(payload: Any) -> PolicySpec:
    return score_transactions(events_to_tx_log(payload))


def fetch_event_pages(
    account_id: str,
    getter: Callable[[str, Optional[str]], Dict[str, Any]],
    max_pages: int = 5,
) -> Dict[str, Any]:
    """Follow `cursor` until the getter returns none or `max_pages` is hit."""
    events: List[Any] = []
    cursor: Optional[str] = None
    for _ in range(max_pages):
        page = getter(account_id, cursor)
        events.extend(page.get("events") or [])
        cursor = page.get("cursor")
        if not cursor:
            break
    return {"account_id": account_id, "events": events}
