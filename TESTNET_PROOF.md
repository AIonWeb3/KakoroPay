# Stellar Testnet Proof of Execution

Generated automatically by `tests/e2e/run_e2e` on 2026-10-27T22:12:50.119Z.

## Executive Summary
- **Verification Mode**: SIMULATION PROTOCOL VALIDATION
- **Network**: `stellar:testnet`
- **Facilitator**: `https://www.x402.org/facilitator` (Fee model: sponsored `areFeesSponsored: true`)
- **Asset (USDC SAC / SEP-41)**: `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA`
- **Smart Account Contract ID**: `Simulated Account`

---

## 1. Allowed Payee Execution (Successful Payment & Settlement)
- **Payee Address**: `GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5`
- **Payment Amount**: `1000` stroops
- **Initial Smart Account Budget**: `10000000` stroops
- **Final Smart Account Budget**: `9999000` stroops
- **Budget Decrease Verified**: ✓ Decreased by `1000` stroops
- **Confirmed Settlement Transaction Hash**:
  `e2e0000000000000000000000000000000000000000000000000000000000001`
- **Testnet Explorer Link**:
  [View on StellarExpert](https://stellar.expert/explorer/testnet/tx/e2e0000000000000000000000000000000000000000000000000000000000001)

---

## 2. Disallowed Payee Execution (Policy Denial Verified)
- **Attempted Payee Address**: `GDISALLOWED00000000000000000000000000000000000000000000000` (NOT on policy allowlist)
- **Denial Result**: ✓ DENIED (Transaction blocked before/at auth)
- **Error Code**: `PolicyDenied`
- **Denial Detail**: `{"code":"PolicyDenied","message":"Policy denied: Payee (CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA, GDISALLOWED00000000000000000000000000000000000000000000000) not in allowed_payees policy"}`

---

## 3. Guarantees Enforced
1. **SSRF and Protocol Verification**: Resource request verified via HTTPS/loopback security guards.
2. **Pre-sign Policy Gate**: Asset and payee validated against smart account policy rules.
3. **On-Chain Policy Authority**: Smart account `__check_auth` verifies authorized rule and decrements windowed budget.
4. **Off-Policy Protection**: Payments to unallowlisted payees are rejected without executing token transfers.
