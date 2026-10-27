//! Least-privilege checks shared by MCP pay_and_call.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyOutcome {
    Allow { remaining: i128 },
    Denied { reason: String },
    InsufficientBudget { required: i128, available: i128 },
}

pub fn evaluate_call(
    allowlisted: bool,
    amount: i128,
    spent: i128,
    max_spend: i128,
    calls: u32,
    max_calls: u32,
) -> PolicyOutcome {
    let remaining = max_spend - spent;
    if !allowlisted {
        return PolicyOutcome::Denied {
            reason: "not_allowlisted".to_string(),
        };
    }
    if calls.saturating_add(1) > max_calls {
        return PolicyOutcome::Denied {
            reason: format!("rate limited: {calls}/{max_calls} calls this period"),
        };
    }
    if spent + amount > max_spend {
        return PolicyOutcome::InsufficientBudget {
            required: amount,
            available: remaining,
        };
    }
    PolicyOutcome::Allow {
        remaining: remaining - amount,
    }
}

/// Payee-aware evaluation for x402 / SEP-41 transfer rules.
///
/// Identical to [`evaluate_call`] but additionally checks that `pay_to`
/// matches the rule's `bound_pay_to`.  Call this when the rule was
/// installed via an `AllowedPayee` entry.  Existing callers that use
/// [`evaluate_call`] for legacy `AllowedContract` rules are unaffected.
///
/// # Arguments
/// - `pay_to`: recipient address in the current transfer invocation.
/// - `bound_pay_to`: the address stored in the policy when the rule was installed.
/// - All other args: same semantics as [`evaluate_call`].
#[allow(dead_code)]
pub fn evaluate_payee_call(
    pay_to: &str,
    bound_pay_to: &str,
    amount: i128,
    spent: i128,
    max_spend: i128,
    calls: u32,
    max_calls: u32,
) -> PolicyOutcome {
    // Payee-binding check first (mirrors Rust contract enforce())
    if pay_to != bound_pay_to {
        return PolicyOutcome::Denied {
            reason: format!("wrong_payee: expected {bound_pay_to}, got {pay_to}"),
        };
    }
    evaluate_call(true, amount, spent, max_spend, calls, max_calls)
}

/// Policy denial and budget failure both skip envelope construction.
pub fn allows_submission(outcome: &PolicyOutcome) -> bool {
    matches!(outcome, PolicyOutcome::Allow { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_cap() {
        let out = evaluate_call(true, 50, 0, 1000, 0, 10);
        assert_eq!(out, PolicyOutcome::Allow { remaining: 950 });
    }

    #[test]
    fn over_cap() {
        let out = evaluate_call(true, 1500, 0, 1000, 0, 10);
        assert!(matches!(out, PolicyOutcome::InsufficientBudget { .. }));
        assert!(!allows_submission(&out));
    }

    #[test]
    fn denial_blocks_submission() {
        let out = evaluate_call(false, 50, 0, 1000, 0, 10);
        assert!(!allows_submission(&out));
    }

    // --- Payee-aware tests ---

    #[test]
    fn payee_correct_recipient_under_cap() {
        let out = evaluate_payee_call("GVENDOR", "GVENDOR", 100, 0, 1000, 0, 10);
        assert_eq!(out, PolicyOutcome::Allow { remaining: 900 });
    }

    #[test]
    fn payee_wrong_recipient_denied() {
        let out = evaluate_payee_call("GOTHER", "GVENDOR", 100, 0, 1000, 0, 10);
        assert!(matches!(out, PolicyOutcome::Denied { .. }));
        if let PolicyOutcome::Denied { reason } = out {
            assert!(reason.contains("wrong_payee"));
        }
    }

    #[test]
    fn payee_over_cap_still_denied() {
        let out = evaluate_payee_call("GVENDOR", "GVENDOR", 2000, 0, 1000, 0, 10);
        assert!(matches!(out, PolicyOutcome::InsufficientBudget { .. }));
    }

    #[test]
    fn payee_rate_limited() {
        let out = evaluate_payee_call("GVENDOR", "GVENDOR", 10, 0, 1000, 10, 10);
        assert!(matches!(out, PolicyOutcome::Denied { .. }));
        if let PolicyOutcome::Denied { reason } = out {
            assert!(reason.contains("rate limited"));
        }
    }
}
