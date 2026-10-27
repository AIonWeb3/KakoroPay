# AgentPay

[![CI](https://github.com/AIonWeb3/AgentPay/actions/workflows/ci.yml/badge.svg)](https://github.com/AIonWeb3/AgentPay/actions/workflows/ci.yml)

> An AI agent discovers, authorizes, pays for, and calls on-chain resources — with
> spending policies generated from data and enforced by a Soroban smart account.

Repo: [github.com/AIonWeb3/AgentPay](https://github.com/AIonWeb3/AgentPay)

**AgentPay** targets two Stellar Community Fund RFPs:

1. **AI-Assisted Policy Toolkit** — Turns observed/simulated transactions into a
   minimal least-privilege account policy (spending caps + contract allowlists).
2. **MCP Discovery & Paid-Call Server** — Lets an AI agent find and pay for a
   resource from inside its own runtime via the Model Context Protocol.

## Pitch demo

A local operator console walks the full story on one screen: synthetic traffic →
policy → MCP discover → budget check → approved pay → policy deny → per-vendor
scope.

```bash
./scripts/run_pitch.sh
# or:
pip install -r demo/requirements.txt -r policy-generator/requirements.txt
python scripts/seed_pitch.py
python demo/server.py
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080), go fullscreen, click **Run pitch demo**.

The console is a FastAPI app plus a vanilla HTML/CSS/JS UI. It persists session,
policy, rules, audit events, and the marketplace registry in SQLite. Mutating
routes require `DEMO_API_KEY`; read routes accept that key or `DEMO_READ_KEY`.

Operator UI covers:

- Stepped pitch API (`POST /api/pitch/step`) with a live narration strip
- MCP-style tools: discover, check_budget, pay_and_call (weather / price / inference)
- Policy and marketplace cards, per-vendor remaining spend and call budget
- `auth_decision` audit timeline and loading / error toasts
- Empty and reset confirmation states, keyboard and screen-reader labels
- Responsive layout; export of the simulation log (`GET /api/tx-log`)

**Simulated vs on-chain:** the console enforces the same allowlist, spend cap,
and rate-limit rules as `contracts/agent-account`, with deterministic local tx
hashes. The MCP server talks to live Soroban RPC / `stellar contract invoke`
when `ACCOUNT_CONTRACT_ID` and `STELLAR_IDENTITY` are set (see `.env.example`).
Optional AgentGuard: set `AGENTGUARD_CONTRACT_ID` and `AGENTGUARD_REQUIRED_ROLE`
(`Basic` / `Premium` / `Admin`) so `__check_auth` calls `verify_agent` before
spend effects.
Without those, `check_budget` falls back to `AGENTPAY_STATE` and `pay_and_call`
returns a `local_` hash — never a stub success against a configured contract.

Copy `.env.example` if you change keys (`DEMO_API_KEY`, `DEMO_READ_KEY`,
`DATABASE_URL`, `AGENTGUARD_CONTRACT_ID`). The demo store uses `DATABASE_URL`
(default `sqlite:///demo/data/agentpay.db`). The MCP server can read a JSON
budget snapshot from `AGENTPAY_STATE` when set.

## Local CI

```bash
./scripts/ci.sh
```

Gates: `cargo fmt`, `clippy -p mcp-server`, `cargo test --workspace`, `ruff`, `pytest`.

GitHub Actions (`.github/workflows/ci.yml`) runs the same Rust and Python jobs
on every push and pull request.

Python tests live in `tests/`, `policy-generator/`, and `demo/tests/` (engine,
auth, persistence, pitch API, UI smoke, accessibility, and full agent-pay flow).

## Architecture

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                                   AI Agent Runtime                                     │
│                                                                                        │
│  ┌──────────┐     MCP/stdio      ┌──────────────────────────────────────────────────┐  │
│  │ AI Agent │◄──────────────────►│                MCP Server (rmcp)                 │  │
│  │          │                    │  • discover_resources(query)                     │  │
│  └──────────┘                    │  • check_budget()                                │  │
│                                  │  • pay_and_call(url | id, params)                │  │
│                                  └───────┬───────────────────────────┬──────────────┘  │
│                                          │                           │                 │
│                                          ▼                           ▼                 │
│                         ┌─────────────────────────────┐   ┌─────────────────────────┐  │
│                         │   DiscoverySource Adapter   │   │  TypeScript Payer Worker│  │
│                         │  • RegistrySource (local)   │   │  (payer/src/cli.ts)     │  │
│                         │  • RemoteCatalogSource (x402│   │  • guards & SSRF check  │  │
│                         │    Bazaar with 60s cache)   │   │  • signs XDR / Soroban  │  │
│                         └─────────────────────────────┘   └──────────┬──────────────┘  │
└──────────────────────────────────────────────────────────────────────┼─────────────────┘
                                                                       │
                         ┌─────────────────────────────────────────────┼─────────────────┐
                         │ HTTP 402 Flow                               │                 │
                         ▼                                             ▼                 │
              ┌─────────────────────┐                     ┌───────────────────────────┐  │
              │  x402 Resource /    │                     │     x402 Facilitator      │  │
              │  Local Seller Server│                     │ (x402.org / OpenZeppelin) │  │
              │  (@x402/express)    │◄─── Settlement ────►│ • verifies signature      │  │
              └─────────────────────┘                     │ • submits tx / sponsors   │  │
                                                          └────────────┬──────────────┘  │
                                                                       │                 │
                                                                       ▼                 │
                                                          ┌───────────────────────────┐  │
                                                          │      Stellar Testnet      │  │
                                                          │                           │  │
                                                          │  ┌─────────────────────┐  │  │
                                                          │  │ Agent Smart Account │  │  │
                                                          │  │ • (asset, pay_to)   │  │  │
                                                          │  │   payee policy      │  │  │
                                                          │  │ • windowed budget   │  │  │
                                                          │  │ • AgentGuard auth   │  │  │
                                                          │  │ • on-chain audit    │  │  │
                                                          │  └─────────────────────┘  │  │
                                                          └───────────────────────────┘  │

┌─────────────────────────────────────────────────────────────────┐
│                     Policy Generation Pipeline                   │
│                                                                  │
│  Transaction Log ──► generate_policy.py ──► PolicySpec JSON     │
│  (observed data)      (p95 caps + allowlist)   (apply on-chain) │
│                                                                  │
│  simulate_agent.py ──► synthetic log for demos                  │
│  seed_pitch.py       ──► deterministic 75-tx seed + policy JSON │
│  demo/server.py      ──► visual operator console                │
└─────────────────────────────────────────────────────────────────┘
```

## Components

| Component | Language | Description |
|-----------|----------|-------------|
| `contracts/agent-account/` | Rust (Soroban) | Smart account: context rules, spend/rate policy, payee-scoped rules `(asset, pay_to)`, optional AgentGuard `verify_agent` |
| `mcp-server/` | Rust | MCP server with 3 tools: discover (composite adapter), check_budget, pay_and_call (x402) |
| `payer/` | TypeScript | Isolated payer worker: sign & assemble Soroban payment transactions with guards |
| `policy-generator/` | Python | Rule-based policy generator (p95 × 1.5 caps from tx logs) |
| `demo/` | Python + HTML | Pitch console: HTTP API, SQLite store, in-memory policy engine, testnet explorer links |
| `registry/` | JSON | Seed data for paid resource discovery (3 vendors) |
| `tests/e2e/` | TypeScript (Node) | End-to-end test harness with `@x402/express` seller and testnet proof runner |
| `scripts/` | Bash/Python | Pitch seed, local CI, testnet deploy, synthetic tx logs |
| `tests/` | Python | Repo-level checks (CI config, env, README, rustfmt) |

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.84+ with `wasm32v1-none` target
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/install-cli)
- Python 3.10+ (CI uses 3.12)

### 1. Build & Test the Smart Account

```bash
cargo test -p agent-account
```

### 2. Run the MCP Server

```bash
cargo run -p mcp-server
```

The server starts on stdio. Tools take real arguments:
- `discover_resources(query)`: Queries resources across both local static registry and remote x402 catalogs via a pluggable `DiscoverySource` abstraction.
  - **Composite Sources**:
    - `RegistrySource`: Loads curated services from `registry/resources.json`.
    - `RemoteCatalogSource`: Fetches dynamic Bazaar-style catalogs over HTTPS (via `X402_CATALOG_URL`), applying SSRF defenses (blocking RFC 1918 private IPs, loopback, link-local, cloud metadata), 5s timeouts, 512KB size limits, and 60-second in-memory caching.
  - **Normalized Schema**: Normalizes items into `{ name, description, url, price, asset, pay_to, network, source, payable, policy_status }`.
  - **Policy Allowance Flag**: Evaluates against the active smart account policy and flags whether each resource is currently payable or *"discoverable but not payable until policy allows"*.
- `check_budget()`: Queries the smart account's remaining spending allowance in stroops and XLM, rolling period ledger window, total configured policy rules, and authorized `allowed_payees: [{ asset, pay_to, rule_id }]`.
- `pay_and_call({ url, resource_id, params })`: Invoke either an x402 paid HTTPS URL or a registry resource ID.
  - **URL mode (x402 flow)**: Fetches the URL with SSRF protection (HTTPS-only, blocks loopback/private/link-local/metadata IPs, redirects disabled, optional `ALLOWED_HOSTS`). On HTTP 402, validates payment requirements and policy prechecks (`get_payee_rule`, `PAYMENT_MAX_PER_CALL`, budget), executes the payer worker via `PAYER_CMD` (`PAYER_TIMEOUT_MS`), retries once with the worker's payment header, decodes settlement transaction hash, and returns `{ status, body, tx_hash, amount, asset, pay_to }`.
  - **Registry mode (legacy)**: Invokes catalog vendor contract with parameter validation and smart account policy checks.
  - **Structured error codes**: `InvalidUrl`, `BlockedTarget`, `PaymentRequirementsInvalid`, `NetworkMismatch`, `OverCeiling`, `PolicyDenied`, `BudgetExceeded`, `PayerFailed`, `PayerTimeout`, `FacilitatorRejected`, `SettlementMissing`, `InsufficientBudget`, `ResourceNotFound`, `ResourceCallFailed`.

### 3. Generate a Policy (Python)

```bash
pip install -r policy-generator/requirements.txt
python scripts/simulate_agent.py > transaction_log.json
python policy-generator/generate_policy.py transaction_log.json > policy.json
```

## End-to-End Testnet Harness (tests/e2e)

The repository includes a comprehensive, automated test harness in `tests/e2e/` that validates the full payment and policy chain against real or simulated x402 endpoints:

1. **Local x402 Seller Service (`tests/e2e/src/seller.ts`)**:
   - Built with official `@x402/express`, `@x402/core`, and `@x402/stellar`.
   - Initializes an `x402ResourceServer` configured with `ExactStellarScheme` for `stellar:testnet`.
   - Connects to an x402 facilitator (`FACILITATOR_URL`, default `https://www.x402.org/facilitator`).
   - Serves an allowed endpoint (`/api/data`) charging `1000` stroops of testnet stablecoin (`TESTNET_ASSET`) to `ALLOWED_PAYEE`.
   - Serves a disallowed endpoint (`/api/disallowed`) charging to `DISALLOWED_PAYEE`.

2. **Harness Runner (`tests/e2e/src/run_e2e.ts`)**:
   - Spawns the MCP server over stdio.
   - Deploys or configures the agent account with policy allowlisting `(TESTNET_ASSET, ALLOWED_PAYEE)`.
   - Calls `check_budget` to verify initial allowance.
   - Executes `pay_and_call` against the allowed payee; asserts a 64-character transaction hash and verifies testnet settlement.
   - Calls `check_budget` again and asserts that the remaining budget decreased by the exact price.
   - Executes `pay_and_call` against the disallowed payee; asserts immediate denial with error code `PolicyDenied` or `PAYEE_NOT_ALLOWLISTED`.
   - Automatically writes execution results and contract IDs to `TESTNET_PROOF.md`.

### Running the Harness

```bash
# Build TypeScript components
npm --prefix payer run build
npm --prefix tests/e2e run build

# Run the test harness in local simulation mode (no testnet funds needed)
node tests/e2e/dist/run_e2e.js
# or via bash:
./tests/e2e/run.sh
```

To run against live on-chain testnet:
```bash
ACCOUNT_CONTRACT_ID=C... \
AGENT_SIGNER_SECRET=S... \
STELLAR_IDENTITY=agent-identity \
FACILITATOR_URL=https://www.x402.org/facilitator \
./tests/e2e/run.sh
```

## Nightly Testnet Workflow

`.github/workflows/e2e-testnet.yml` runs automatically every night at 03:00 UTC and supports manual dispatch (`workflow_dispatch`).

Key properties:
- **Zero Impact on Regular CI**: Fast local `./scripts/ci.sh` and push/PR CI (`.github/workflows/ci.yml`) remain testnet-free.
- **Graceful Degradation**: If repository secrets (`ACCOUNT_CONTRACT_ID`, `AGENT_SIGNER_SECRET`) are not configured, the workflow logs a clear notice and runs the protocol validation flow safely.
- **Security**: Never logs secrets or private keys; testnet secrets are injected only as environment variables.
- **Proof Artifacts**: Automatically uploads `TESTNET_PROOF.md` and harness execution logs as downloadable workflow artifacts.

## Facilitator & x402 Ecosystem

AgentPay supports any standard x402 testnet facilitator. Configurable via `FACILITATOR_URL`:
- Default: `https://www.x402.org/facilitator` (public testnet facilitator supporting `stellar:testnet` scheme `exact` with sponsored network fees `areFeesSponsored: true`).
- OpenZeppelin Channels: `https://channels.openzeppelin.com/x402/testnet` (pass API key via `FACILITATOR_API_KEY`).

## Environment Variables & Configuration

Copy `.env.example` to `.env` to customize settings:

| Variable | Default | Purpose |
|----------|---------|---------|
| `DEMO_API_KEY` | `dev-operator-key` | Operator authentication key for pitch console |
| `DEMO_READ_KEY` | `dev-read-key` | Read-only authentication key for pitch console |
| `DATABASE_URL` | `sqlite:///demo/data/agentpay.db` | Pitch console SQLite database |
| `ACCOUNT_CONTRACT_ID` | *(empty)* | On-chain Soroban Smart Account contract ID |
| `STELLAR_IDENTITY` | *(empty)* | Stellar CLI identity name for contract invocation |
| `SOROBAN_RPC_URL` | `https://soroban-testnet.stellar.org` | Soroban testnet RPC endpoint |
| `STELLAR_NETWORK` | `testnet` | Target Stellar network |
| `AGENT_SIGNER_SECRET` | *(empty)* | Stellar Ed25519 secret key for signing agent transactions |
| `FACILITATOR_URL` | `https://www.x402.org/facilitator` | x402 facilitator endpoint URL |
| `FACILITATOR_API_KEY` | *(empty)* | Optional API bearer token for paid facilitators |
| `X402_CATALOG_URL` | *(empty)* | Remote x402 Bazaar catalog URL for dynamic resource discovery |
| `TESTNET_ASSET` | `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA` | Testnet USDC SAC / SEP-41 token contract ID |
| `ALLOWED_PAYEE` | *(random)* | Testnet payee public key allowlisted in account policy |
| `DISALLOWED_PAYEE` | *(random)* | Testnet payee public key NOT allowlisted in account policy |
| `PAYMENT_MAX_PER_CALL` | `10000000` | Maximum stroops per single payment request (10 XLM) |
| `ALLOWED_HOSTS` | *(empty)* | Optional comma-separated host allowlist for `pay_and_call` |
| `AGENTPAY_ALLOW_TEST_LOOPBACK`| `false` | Allows loopback HTTP connections during e2e testing |

## Note for Windows Contributors (WSL2 Recommendation)

When building Soroban smart account contracts (`cargo build --target wasm32v1-none` or native contract tests) on Windows using the GNU toolchain (MinGW), the linker may fail with:
```
ld.exe: error: export ordinal too large: 75316
```
This is caused by the Windows PE binary format limit of 65,535 exported symbol ordinals when compiling Rust cdylibs with full Soroban SDK symbols.

**Recommendations:**
- **Local Testing**: The MCP server (`cargo test -p mcp-server`), TypeScript payer (`npm --prefix payer test`), and Python test suite run cleanly and natively on Windows.
- **Contract Compilation**: Use **WSL2** (Windows Subsystem for Linux) or a Linux container for building and deploying Soroban contracts (`contracts/agent-account/`).

## End-to-End Demo Flow

```
1. SIMULATE  →  python scripts/simulate_agent.py > tx_log.json
                 Generates 75 synthetic transactions across 3 resources.

2. GENERATE  →  python policy-generator/generate_policy.py tx_log.json > policy.json
                 Produces a PolicySpec with per-contract spend caps derived
                 from the p95 of observed amounts.

3. DEPLOY    →  ./scripts/deploy_testnet.sh
                 Simulates a tx log, generates PolicySpec, deploys the account
                 and spend-policy instances, initialize, set_spend_policy,
                 optional set_agent_guard (if AGENTGUARD_CONTRACT_ID is set),
                 apply_policy. Prints MCP env and stellar event commands.

4. DISCOVER  →  Agent calls discover_resources("weather") via MCP
                 Gets back matching resources with pricing info.

5. CHECK     →  Agent calls check_budget() via MCP
                 Sees remaining allowance under current policy.

6. PAY+CALL  →  Agent calls pay_and_call("weather-oracle", "{}") via MCP
                 Transaction authorized by smart account, spend recorded,
                 resource invoked, real tx hash when ACCOUNT_CONTRACT_ID is set.

7. AUDIT     →  stellar events --id <CONTRACT>  (auth_decision)
                 Approved decisions persist on-chain; denied auths fail the tx
                 with OverBudget / RateLimited / InvalidContext /
                 AgentGuardExecutionDenied.
```

Steps 1–2 and 4–6 can be shown in one take via `python demo/server.py`
(or `./scripts/run_pitch.sh`, which seeds `demo/data/` first).

## Project Structure

```
AgentPay/
├── Cargo.toml
├── pyproject.toml                    # ruff + pytest
├── .github/workflows/
│   ├── ci.yml                        # Fast local-equivalent CI (no testnet)
│   └── e2e-testnet.yml               # Nightly + manual testnet e2e workflow
├── contracts/agent-account/          # Soroban smart account (+ AgentGuard client)
├── mcp-server/                       # MCP tools over stdio (DiscoverySource, pay_and_call)
├── payer/                            # TypeScript payer worker (CLI, guards, Soroban auth)
├── policy-generator/                 # PolicySpec from tx logs
├── demo/                             # Pitch console with explorer tx links
│   ├── server.py                     # FastAPI routes
│   ├── engine.py                     # Enforcement mirroring the contract
│   ├── store.py / schema.sql         # SQLite persistence
│   ├── auth.py                       # Operator vs reader keys
│   ├── policy_eval.py / txhash.py
│   ├── static/                       # Operator UI
│   └── tests/                        # API, engine, and UI smoke tests
├── registry/resources.json           # Curated resource catalog
├── tests/
│   ├── e2e/                          # End-to-end testnet harness (@x402/express seller)
│   │   ├── src/seller.ts             # Express server with x402ResourceServer
│   │   ├── src/run_e2e.ts            # Harness runner & TESTNET_PROOF generator
│   │   └── run.sh                    # E2E test runner script
│   └── test_*.py                     # Repo-level checks (CI, README, env, rules)
├── scripts/
│   ├── run_pitch.sh
│   ├── seed_pitch.py
│   ├── ci.sh
│   ├── deploy_testnet.sh
│   └── simulate_agent.py
├── TESTNET_PROOF.md                  # Generated proof artifact from e2e harness
└── README.md
```

## Tech Stack

- **Soroban Contracts**: `soroban-sdk`, `#![no_std]`
- **Smart Account Framework**: OpenZeppelin `stellar-accounts` 0.7.x
- **MCP Server**: `rmcp` 3.1.x, stdio transport
- **Policy Generator**: Python 3.10+, `pydantic` 2.x
- **Pitch console**: FastAPI + SQLite + vanilla HTML/CSS/JS

## Current Status

- Smart account: initialize, set_spend_policy, apply_policy (per-vendor `CallContract` rules + spend/rate policy on `__check_auth`, plus payee-scoped SEP-41 token transfer rules keyed by `(asset, pay_to)`), optional `set_agent_guard` (`verify_agent` before spend), get_remaining_budget, rolling window, events
- MCP: discover; check_budget from on-chain account when `ACCOUNT_CONTRACT_ID` is set (else `AGENTPAY_STATE`); pay_and_call submits via Stellar CLI + polls `getTransaction` (no stub hash on the live path); bounded retry on `TransientError` only
- Policy generator: p95 × 1.5 caps + allowlist (supporting both legacy vendor contracts and `(asset, pay_to)` payee rules); empty logs rejected
- Demo console: SQLite persistence, operator/reader keys, stepped pitch API, per-vendor budget, deterministic tx hashes, tx-log export, polished operator UI (responsive, a11y, toasts, loading, empty/reset)
- Deploy: `./scripts/deploy_testnet.sh` runs simulate → generate → deploy two instances → initialize → apply_policy; links AgentGuard when `AGENTGUARD_CONTRACT_ID` is set (`AGENTGUARD_REQUIRED_ROLE`, default Basic)
- CI: local `scripts/ci.sh` and GitHub Actions (fmt, clippy, cargo test, ruff, pytest)

**Out of scope for this MVP (follow-up)**

- Funded testnet deploy inside CI
- LLM-based policy generation
- Integration tests against public testnet in CI

## License

See [LICENSE](LICENSE).
