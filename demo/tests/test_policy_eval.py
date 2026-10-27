"""Tests for payee-aware policy evaluation (mirrors Rust contract cases)."""

from __future__ import annotations

from policy_eval import evaluate_call

# ---------------------------------------------------------------------------
# Legacy cases (unchanged)
# ---------------------------------------------------------------------------


def test_evaluate_under_cap():
    d = evaluate_call(allowlisted=True, amount=50, spent=0, max_spend=1000, calls=0, max_calls=10)
    assert d.ok
    assert d.remaining == 950


def test_evaluate_not_allowlisted():
    d = evaluate_call(allowlisted=False, amount=50, spent=0, max_spend=1000, calls=0, max_calls=10)
    assert not d.ok
    assert d.error == "PolicyDenied"


# ---------------------------------------------------------------------------
# Payee-binding cases
# ---------------------------------------------------------------------------


def test_payee_correct_recipient_approved():
    d = evaluate_call(
        allowlisted=True,
        amount=100,
        spent=0,
        max_spend=1000,
        calls=0,
        max_calls=10,
        pay_to="GVENDOR",
        bound_pay_to="GVENDOR",
    )
    assert d.ok
    assert d.remaining == 900


def test_payee_wrong_recipient_denied():
    d = evaluate_call(
        allowlisted=True,
        amount=100,
        spent=0,
        max_spend=1000,
        calls=0,
        max_calls=10,
        pay_to="GOTHER",
        bound_pay_to="GVENDOR",
    )
    assert not d.ok
    assert d.error == "PolicyDenied"
    assert "wrong_payee" in d.reason


def test_payee_over_cap_denied():
    d = evaluate_call(
        allowlisted=True,
        amount=2000,
        spent=0,
        max_spend=1000,
        calls=0,
        max_calls=10,
        pay_to="GVENDOR",
        bound_pay_to="GVENDOR",
    )
    assert not d.ok
    assert d.error == "InsufficientBudget"


def test_payee_rate_limited():
    d = evaluate_call(
        allowlisted=True,
        amount=10,
        spent=0,
        max_spend=10000,
        calls=10,
        max_calls=10,
        pay_to="GVENDOR",
        bound_pay_to="GVENDOR",
    )
    assert not d.ok
    assert d.error == "PolicyDenied"
    assert "rate limited" in d.reason


def test_payee_no_bound_pay_to_treats_as_legacy():
    """When bound_pay_to is None the check is skipped (legacy path)."""
    d = evaluate_call(
        allowlisted=True,
        amount=50,
        spent=0,
        max_spend=1000,
        calls=0,
        max_calls=10,
        pay_to="GANYONE",
        bound_pay_to=None,
    )
    assert d.ok


def test_payee_remaining_unchanged_on_deny():
    """remaining must reflect pre-call state when denied."""
    d = evaluate_call(
        allowlisted=True,
        amount=100,
        spent=400,
        max_spend=1000,
        calls=0,
        max_calls=10,
        pay_to="GOTHER",
        bound_pay_to="GVENDOR",
    )
    assert not d.ok
    assert d.remaining == 600  # max_spend - spent, not modified
