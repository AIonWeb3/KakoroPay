# AgentPay TypeScript Payer Worker

Standalone TypeScript worker and CLI that consumes an x402 payment requirement and returns a payment payload authorized by the AgentPay smart account.

## Overview

In x402 on Stellar, the buyer signs a Soroban authorization entry for a token transfer (`transfer(from, to, amount)`), and a facilitator submits the transaction and pays the network transaction fees.

For an AgentPay smart account (contract account starting with `C...` deployed using OpenZeppelin `stellar-accounts` 0.7.x):
1. **Payee Context Rule**: Each allowlisted payee `(asset, pay_to)` is bound to an on-chain context rule ID in the smart account contract.
2. **Reverse Lookup**: The worker queries `get_payee_rule(asset, pay_to)` on-chain via Soroban RPC to retrieve the specific context rule ID.
3. **Soroban Preimage Signing**: The agent signs the Soroban authorization preimage hash:
   $$\text{digest} = \text{SHA256}(\text{preimage} \parallel \text{rule\_ids.to\_xdr}())$$
   using its Ed25519 secret key.
4. **AuthPayload Wrapper**: The authorization credentials wrap the signature into the OpenZeppelin `AuthPayload` structure:
   ```rust
   pub struct AuthPayload {
       pub context_rule_ids: Vec<u32>,
       pub signers: Map<Signer, BytesN<64>>,
   }
   ```
   where `Signer` is `Signer::Delegated(agent_address)`.
5. **Simulation Verification**: Before returning, the authorization is simulated against Soroban RPC to confirm `__check_auth` accepts the entry.

---

## CLI Specification

The CLI reads a single JSON object from `stdin` and outputs a single JSON object to `stdout`. All logs, diagnostics, and debugging messages are written strictly to `stderr`.

### Stdin Input Schema

```json
{
  "resource_url": "https://api.example.com/protected/data",
  "payment_requirements": {
    "scheme": "exact",
    "network": "stellar:testnet",
    "asset": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
    "payTo": "GB7B2YWT5ELTMVDQREHAQDMIFMTKOR3TOEDOYW3ONM2ZMZGIF6CS2BA3",
    "amount": "1000000",
    "extra": {
      "areFeesSponsored": true
    },
    "maxTimeoutSeconds": 60
  }
}
```

#### Fields

| Field | Type | Description |
|---|---|---|
| `resource_url` | `string` | The target resource URL. Must begin with `https://`. |
| `payment_requirements` | `object` | The x402 payment requirement parameters. |
| `payment_requirements.scheme` | `string` | Payment scheme (typically `"exact"`). |
| `payment_requirements.network` | `string` | Network identifier (e.g. `"stellar:testnet"`, `"stellar:pubnet"`). |
| `payment_requirements.asset` | `string` | Stellar contract address (`C...`) of the token asset. |
| `payment_requirements.payTo` | `string` | Stellar public key or contract address of recipient. |
| `payment_requirements.amount` | `string` | Amount in stroops / base units as a numeric string. |
| `payment_requirements.extra` | `object` | Optional extra scheme parameters (e.g. fee sponsorship). |

---

### Stdout Output Schema

#### Success Response (`ok: true`)

```json
{
  "ok": true,
  "payment_payload": {
    "transaction": "<base64_encoded_xdr_transaction_envelope>"
  },
  "rule_id": 42
}
```

#### Failure Response (`ok: false`)

```json
{
  "ok": false,
  "code": "PAYEE_NOT_ALLOWLISTED",
  "message": "Payee (CDLZ..., GB7B...) is not allowlisted by on-chain spend policy"
}
```

### Error Codes

| Code | Description |
|---|---|
| `NON_HTTPS_RESOURCE_URL` | Resource URL is missing or does not use HTTPS. |
| `NETWORK_MISMATCH` | Requirement network differs from payer configured network. |
| `INVALID_AMOUNT` | Amount is missing, non-numeric, or non-positive. |
| `AMOUNT_EXCEEDS_MAX_PER_CALL` | Amount exceeds configured `PAYER_MAX_PER_CALL` limit. |
| `PAYEE_NOT_ALLOWLISTED` | `(asset, pay_to)` has no matching rule ID returned by `get_payee_rule`. |
| `OverBudget` | Contract spend limit exceeded for current period. |
| `RateLimited` | Contract call rate limit exceeded for current period. |
| `InvalidContext` | Soroban authorization context rule check failed. |
| `AgentGuardExecutionDenied` | Guard denied execution of authorization entry. |
| `SIMULATION_FAILED` | Other Soroban RPC simulation failure. |
| `MISSING_CONFIG` | Required environment variables or configuration missing or malformed. |
| `INTERNAL_ERROR` | Unexpected internal exception. |

---

## Configuration & Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `ACCOUNT_CONTRACT_ID` | **Yes** | - | Smart account contract address (`C...`). |
| `AGENT_SIGNER_SECRET` | **Yes\*** | - | Ed25519 secret key (`S...`) of the agent. |
| `AGENT_SIGNER_SECRET_PATH`| **Yes\*** | - | Path to a file containing the agent secret key. |
| `SOROBAN_RPC_URL` | No | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint URL. |
| `STELLAR_NETWORK` | No | `stellar:testnet` | Target Stellar network. |
| `STELLAR_NETWORK_PASSPHRASE` | No | `Test SDF Network ; September 2015` | Network passphrase. |
| `PAYER_MAX_PER_CALL` | No | `10000000` (1 XLM) | Maximum stroops allowed per payment call. |
| `PAYER_EXPIRATION_SECONDS` | No | `60` | Valid auth entry lifetime in seconds. |
| `RUN_INTEGRATION_TESTS` | No | `false` | Run live testnet integration tests when `true`. |

\* Provide either `AGENT_SIGNER_SECRET` or `AGENT_SIGNER_SECRET_PATH`. Secrets are never logged or exposed in stdout or error responses.

---

## Standalone Usage

### Build
```bash
npm ci
npm run build
```

### Run CLI
```bash
node dist/cli.js < request.json
```
or via npm:
```bash
npm start < request.json
```

### Run Tests
```bash
npm test
```
