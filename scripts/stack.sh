#!/usr/bin/env bash
# The local stack: `up` starts PostgreSQL (compose project `backend-platform`, port 61432 on
# 127.0.0.1) and waits until it is healthy; `down` stops it; `reset` also deletes its
# volume; `env` prints the variables the processes and the tests read.
set -euo pipefail
cd "$(dirname "$0")/.."

compose=(docker compose -f deploy/compose.yaml)
port="${BP_PG_PORT:-61432}"

case "${1:-}" in
  up)
    "${compose[@]}" up -d --wait postgres
    echo "stack ok: postgres on 127.0.0.1:${port}"
    ;;
  down)
    "${compose[@]}" down
    ;;
  reset)
    "${compose[@]}" down --volumes
    ;;
  env)
    echo "export BP_DATABASE_URL=postgres://platform:platform-dev@127.0.0.1:${port}/platform"
    echo "export BP_TEST_DATABASE_URL=postgres://platform:platform-dev@127.0.0.1:${port}/platform"
    ;;
  *)
    echo "usage: scripts/stack.sh up|down|reset|env" >&2
    exit 2
    ;;
esac
