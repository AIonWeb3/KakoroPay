"""Pure policy decision helper shared by the demo engine and tests."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class PolicyDecision:
    ok: bool
    error: Optional[str]
    reason: str
    remaining: int


def evaluate_call(
    *,
    allowlisted: bool,
    amount: int,
    spent: int,
    max_spend: int,
    calls: int,
    max_calls: int,
    # Payee binding (x402 / SEP-41).  Both must be provided together.
    # When present, ``pay_to`` must equal ``bound_pay_to``; otherwise
    # the call is denied with reason ``wrong_payee`` (mirrors InvalidContext
    # from the Rust contract).
    pay_to: Optional[str] = None,
    bound_pay_to: Optional[str] = None,
) -> PolicyDecision:
    remaining = max_spend - spent
    if not allowlisted:
        return PolicyDecision(False, "PolicyDenied", "not_allowlisted", remaining)
    # Payee-binding check (mirrors spend.rs enforce() for payee rules)
    if bound_pay_to is not None and pay_to != bound_pay_to:
        return PolicyDecision(False, "PolicyDenied", "wrong_payee", remaining)
    if calls + 1 > max_calls:
        return PolicyDecision(
            False,
            "PolicyDenied",
            f"rate limited: {calls}/{max_calls} calls this period",
            remaining,
        )
    if spent + amount > max_spend:
        return PolicyDecision(
            False,
            "InsufficientBudget",
            f"required {amount} stroops, remaining {remaining} stroops",
            remaining,
        )
    return PolicyDecision(True, None, "under_cap", remaining - amount)
