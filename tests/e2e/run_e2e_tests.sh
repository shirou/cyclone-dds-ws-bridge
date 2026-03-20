#!/bin/bash
set -euo pipefail

# Run bridge E2E tests via docker-compose
# Usage: ./tests/e2e/run_e2e_tests.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$PROJECT_DIR"

COMPOSE="docker compose -f docker-compose.test.yml --profile e2e"

cleanup() {
    echo "=== Cleaning up ==="
    $COMPOSE down -v 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Building E2E test images ==="
$COMPOSE build

echo "=== Starting bridge ==="
$COMPOSE up -d bridge

echo "=== Waiting for bridge to become healthy ==="
for i in $(seq 1 30); do
    if $COMPOSE ps bridge | grep -q "healthy"; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "FAIL: bridge did not become healthy within 60s"
        $COMPOSE logs bridge
        exit 1
    fi
    sleep 2
done

echo "=== Running Go E2E tests ==="
$COMPOSE run --rm go-e2e

echo "=== Running Python E2E tests ==="
$COMPOSE run --rm py-e2e

echo "PASS: All E2E tests passed"
