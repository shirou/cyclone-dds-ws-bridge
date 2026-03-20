#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$PROJECT_ROOT"

echo "=== CDR Interop Tests (Phase 6-1) ==="
echo ""
echo "Building and running all interop test services..."
echo ""

docker compose -f docker-compose.test.yml up \
    --build \
    --abort-on-container-exit \
    --exit-code-from go-verify

rc=$?

docker compose -f docker-compose.test.yml down -v

exit $rc
