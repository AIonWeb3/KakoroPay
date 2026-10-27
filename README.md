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
┌─────────────────────────────────────────────────────────────────┐
│                        AI Agent Runtime                         │
│                                                                 │
│  ┌──────────┐    MCP/stdio    ┌──────────────────────────────┐ │
│  │ AI Agent  │◄──────────────►│      MCP Server (rmcp)       │ │
│  │           │                │  • discover_resources(query)  │ │
│  └──────────┘                │  • check_budget()             │ │
│                               │  • pay_and_call(id, params)   │ │
│                               └──────────┬───────────────────┘ │
└──────────────────────────────────────────┼───────────────────┘
                                           │ Soroban RPC (when ACCOUNT_CONTRACT_ID is set)
                                           ▼
                              ┌────────────────────────┐
                              │  Stellar Testnet       │
                              │                        │
                              │  ┌──────────────────┐  │
                              │  │ Agent Account    │  │
                              │  │ (Smart Account)  │  │
                              │  │                  │  │
                              │  │ Context Rules    │  │
                              │  │ Spending Limits  │  │
                              │  │ AgentGuard check │  │
                              │  │ Audit Events     │  │
                              │  └──────────────────┘  │
                              └────────────────────────┘

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
| `contracts/agent-account/` | Rust (Soroban) | Smart account: context rules, spend/rate policy, optional AgentGuard `verify_agent` |
| `mcp-server/` | Rust | MCP server with 3 tools: discover, check_budget, pay_and_call |
| `policy-generator/` | Python | Rule-based policy generator (p95 × 1.5 caps from tx logs) |
| `demo/` | Python + HTML | Pitch console: HTTP API, SQLite store, in-memory policy engine |
| `registry/` | JSON | Seed data for paid resource discovery (3 vendors) |
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

The server starts on stdio. Tools take real arguments (`query`, `resource_id`,
`params`). Connect any MCP-compatible client to use them.

### 3. Generate a Policy (Python)

```bash
pip install -r policy-generator/requirements.txt
python scripts/simulate_agent.py > transaction_log.json
python policy-generator/generate_policy.py transaction_log.json > policy.json
```

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
├── .github/workflows/ci.yml
├── contracts/agent-account/          # Soroban smart account (+ AgentGuard client)
├── mcp-server/                       # MCP tools over stdio
├── policy-generator/                 # PolicySpec from tx logs
├── demo/                             # Pitch console
│   ├── server.py                     # FastAPI routes
│   ├── engine.py                     # Enforcement mirroring the contract
│   ├── store.py / schema.sql         # SQLite persistence
│   ├── auth.py                       # Operator vs reader keys
│   ├── policy_eval.py / txhash.py
│   ├── static/                       # Operator UI
│   └── tests/                        # API, engine, and UI smoke tests
├── registry/resources.json
├── scripts/
│   ├── run_pitch.sh
│   ├── seed_pitch.py
│   ├── ci.sh
│   ├── deploy_testnet.sh
│   └── simulate_agent.py
├── tests/
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
