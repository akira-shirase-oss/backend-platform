#!/usr/bin/env bash
# The local stack: `up` starts PostgreSQL (compose project `backend-platform`, port 61432 on
# 127.0.0.1, a named volume and no bind mounts), waits until it is healthy, then creates the
# owner and runtime roles and the development database `bp_dev` and migrates it as the
# owner (platform-dbctl); `down` stops it; `reset` also deletes its volume; `env` prints the
# variables the processes and the tests read:
#
#   BP_TEST_DATABASE_URL       the administrator (creates roles and test databases)
#   BP_MIGRATION_DATABASE_URL  the owner role on bp_dev (migrations only)
#   BP_DATABASE_URL            the runtime role on bp_dev (server and worker)
#
# Every password here is a public development value.
set -euo pipefail
cd "$(dirname "$0")/.."

compose=(docker compose -f deploy/compose.yaml)
port="${BP_PG_PORT:-61432}"
# The stack binds a port inside this repository's band (deploy/compose.yaml), like every
# process here, so a second checkout or another project on the shared machine cannot collide.
case "$port" in ''|*[!0-9]*) echo "error: BP_PG_PORT is not a port number: $port" >&2; exit 2 ;; esac
# [ -lt ] returns status 2 on an out-of-range integer, which `||` reads as in-band;
# 10# keeps a leading zero decimal and a wrapped huge number still fails the range test.
if ! { [[ $port =~ ^[1-9][0-9]{0,4}$ ]] && (( 10#$port >= 61400 && 10#$port <= 61499 )); }; then
  echo "error: BP_PG_PORT $port is outside this project's port band 61400-61499" >&2
  exit 2
fi
admin="postgres://platform:platform-dev@127.0.0.1:${port}/platform"
owner="postgres://platform_owner:platform-owner-dev@127.0.0.1:${port}/bp_dev"
runtime="postgres://platform_app:platform-app-dev@127.0.0.1:${port}/bp_dev"

case "${1:-}" in
  up)
    "${compose[@]}" up -d --wait postgres
    BP_ADMIN_DATABASE_URL="$admin" BP_DATABASE_NAME=bp_dev \
      BP_OWNER_PASSWORD=platform-owner-dev BP_APP_PASSWORD=platform-app-dev \
      cargo run --quiet --locked -p platform-db --bin platform-dbctl -- bootstrap >/dev/null
    BP_MIGRATION_DATABASE_URL="$owner" cargo run --quiet --locked -p platform-db --bin platform-dbctl -- migrate >/dev/null
    echo "stack ok: postgres on 127.0.0.1:${port}, database bp_dev migrated"
    ;;
  down)
    "${compose[@]}" down
    ;;
  reset)
    "${compose[@]}" down --volumes
    ;;
  env)
    # eval'd by callers: %q so no value can run shell.
    printf 'export BP_TEST_DATABASE_URL=%q\n' "$admin"
    printf 'export BP_MIGRATION_DATABASE_URL=%q\n' "$owner"
    printf 'export BP_DATABASE_URL=%q\n' "$runtime"
    ;;
  *)
    echo "usage: scripts/stack.sh up|down|reset|env" >&2
    exit 2
    ;;
esac
