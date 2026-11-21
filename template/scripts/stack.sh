#!/usr/bin/env bash
# The local database: `up` starts PostgreSQL (compose project starter-template, 127.0.0.1 on
# STARTER_PG_PORT, default 61433) and waits until it is healthy; `down` stops it; `reset`
# also deletes its volume; `env` prints what the tests and drills read:
#
#   STARTER_TEST_DATABASE_URL   the administrator (creates roles and throwaway databases)
#   STARTER_PG_CONTAINER        where the drills run pg_dump and pg_restore (so they need not be installed)
#
# With STARTER_TEST_DATABASE_URL already set (another PostgreSQL 18 you administer), `up`
# starts nothing and `env` repeats it.
#
# Every password here is a public development value.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
cd "$(dirname "$0")/.."

compose=(docker compose -f deploy/compose.yaml)
# The band lives in one file (COPYING.md); validate before the port reaches a URL.
band="$(cat scripts/port-band)"
[[ "$band" =~ ^[1-9][0-9]{1,4}-[1-9][0-9]{1,4}$ ]] || { echo "stack: scripts/port-band must be <low>-<high>, got '$band'" >&2; exit 2; }
port="${STARTER_PG_PORT:-61433}"
if ! { [[ $port =~ ^[1-9][0-9]{0,4}$ ]] && (( 10#$port >= 10#${band%%-*} && 10#$port <= 10#${band##*-} )); }; then
  echo "stack: STARTER_PG_PORT $port is outside this project's port band $band" >&2
  exit 2
fi

case "${1:-}" in
  up)
    if [ -n "${STARTER_TEST_DATABASE_URL:-}" ]; then
      echo "stack: STARTER_TEST_DATABASE_URL is set; using that database, starting none"
      exit 0
    fi
    command -v docker >/dev/null || { echo "stack: docker is needed (or set STARTER_TEST_DATABASE_URL to an administrator's URL)" >&2; exit 1; }
    "${compose[@]}" up -d --wait postgres >/dev/null
    echo "stack ok: postgres on 127.0.0.1:${port}"
    ;;
  down) "${compose[@]}" down ;;
  reset) "${compose[@]}" down --volumes ;;
  env)
    # eval'd by callers: %q so a URL query or container id cannot run shell.
    if [ -n "${STARTER_TEST_DATABASE_URL:-}" ]; then
      printf 'export STARTER_TEST_DATABASE_URL=%q\n' "$STARTER_TEST_DATABASE_URL"
      printf 'export STARTER_PG_CONTAINER=%q\n' "${STARTER_PG_CONTAINER:-}"
    else
      printf 'export STARTER_TEST_DATABASE_URL=%q\n' "postgres://starter:starter-admin-dev@127.0.0.1:${port}/starter"
      printf 'export STARTER_PG_CONTAINER=%q\n' "$("${compose[@]}" ps -q postgres)"
    fi
    ;;
  *) echo "stack: unknown command '${1:-}'. Commands: up, down, reset, env (see --help)" >&2; exit 2 ;;
esac
