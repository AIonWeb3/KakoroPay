#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "==> Building payer and e2e harness"
(cd "$ROOT/payer" && npm run build)
(cd "$ROOT/tests/e2e" && node ./node_modules/typescript/bin/tsc -p tsconfig.json)

echo "==> Executing AgentPay E2E Testnet Harness"
node "$ROOT/tests/e2e/dist/run_e2e.js"
