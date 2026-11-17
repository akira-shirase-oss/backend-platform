#!/usr/bin/env bash
# The local database: `up` starts PostgreSQL (compose project starter-template, 127.0.0.1 on
# STARTER_PG_PORT, default 61433) and waits until it is healthy; `down` stops it; `reset`
# also deletes its volume; `env` prints what the tests and drills read:
#
#   STARTER_TEST_DATABASE_URL   the administrator (creates roles and throwaway databases)
#   STARTER_PG_CONTAINER        where pg_dump and pg_restore run (they are not needed locally)
#
# Every password here is a public development value.
set -euo pipefail
cd "$(dirname "$0")/.."

compose=(docker compose -f deploy/compose.yaml)
port="${STARTER_PG_PORT:-61433}"

case "${1:-}" in
  up) "${compose[@]}" up -d --wait postgres >/dev/null; echo "stack ok: postgres on 127.0.0.1:${port}" ;;
  down) "${compose[@]}" down ;;
  reset) "${compose[@]}" down --volumes ;;
  env)
    echo "export STARTER_TEST_DATABASE_URL=postgres://starter:starter-admin-dev@127.0.0.1:${port}/starter"
    echo "export STARTER_PG_CONTAINER=$("${compose[@]}" ps -q postgres)"
    ;;
  *) echo "usage: scripts/stack.sh up|down|reset|env" >&2; exit 2 ;;
esac
