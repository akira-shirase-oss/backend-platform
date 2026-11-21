#!/usr/bin/env bash
# Runs the control-API load scenario against release builds and keeps the evidence
# (workspace load-test standard, D-018).
#
#   MODE=smoke scripts/load.sh    100,000 requests (the B0 baseline)
#   MODE=full  scripts/load.sh    1,000,000 requests — only on a quiet machine
#
# It starts, on 127.0.0.1, the fake provider (BP_FAKE_PORT, default 61410) with a seeded
# 0.1% fault schedule, the worker (no listener) and the server (BP_SERVER_PORT, default
# 61400), on a fresh control database in the
# local stack; offers load with k6 at RATE requests/s (default 2000); then waits for the
# worker to drain every operation and reads the provider side: duplicates must be zero and
# the provider must hold exactly PROJECTS × 5 resources. Every result records the machine,
# the commit (and whether the tree was dirty) and the load average before and after; a
# result taken with a load average above 6 is marked as a busy-machine observation. A
# threshold or invariant miss fails the run and is kept as evidence, never rerun until
# green. Results go to perf/results/ in both modes.
#
# Needs: k6 (scripts/install-k6.sh), jq, curl, and docker for the local stack (or
# BP_LOAD_ADMIN_URL naming an administrator that may create roles and databases). The
# run's database (bp_load_<timestamp>) is dropped at the end.
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${MODE:-smoke}"
rate="${RATE:-2000}"
projects="${PROJECTS:-2000}"
server_port="${BP_SERVER_PORT:-61400}"
fake_port="${BP_FAKE_PORT:-61410}"
case "$mode" in smoke|full) ;; *) echo "MODE must be smoke or full" >&2; exit 2 ;; esac
# A run binds ports only inside this repository's band (deploy/compose.yaml), so a second
# checkout or another project on the shared machine never collides with it.
for port in "$server_port" "$fake_port"; do
  case "$port" in ''|*[!0-9]*) echo "error: not a port number: $port" >&2; exit 2 ;; esac
  # [ -lt ] returns status 2 on an out-of-range integer, which `||` reads as in-band;
  # 10# keeps a leading zero decimal and a wrapped huge number still fails the range test.
  if ! { [[ $port =~ ^[1-9][0-9]{0,4}$ ]] && (( 10#$port >= 61400 && 10#$port <= 61499 )); }; then
    echo "error: $port is outside this project's port band 61400-61499 (BP_SERVER_PORT, BP_FAKE_PORT)" >&2
    exit 2
  fi
done
if [ "$server_port" = "$fake_port" ]; then
  echo "error: BP_SERVER_PORT and BP_FAKE_PORT must differ" >&2
  exit 2
fi

cargo build --release --locked -p platform-server -p platform-worker -p platform-fake-provider -p platform-db >/dev/null
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
# A fresh control database per run, set up as in production: the administrator creates the
# roles and the database, the owner migrates, the server and worker run as the runtime role.
# CI passes its own administrator URL (BP_LOAD_ADMIN_URL); locally it is the stack's.
if [ -z "${BP_LOAD_ADMIN_URL:-}" ]; then
  scripts/stack.sh up >/dev/null
fi
admin_url="${BP_LOAD_ADMIN_URL:-postgres://platform:platform-dev@127.0.0.1:${BP_PG_PORT:-61432}/platform}"
db="bp_load_$(date -u +%Y%m%d%H%M%S)"
# Roles are cluster-wide: use the stack's public development passwords, never per-run ones.
owner_pw=platform-owner-dev
app_pw=platform-app-dev
BP_ADMIN_DATABASE_URL="$admin_url" BP_DATABASE_NAME="$db" BP_OWNER_PASSWORD="$owner_pw" BP_APP_PASSWORD="$app_pw" \
  ./target/release/platform-dbctl bootstrap >/dev/null
host_part="${admin_url#*://}"; host_part="${host_part#*@}"; host_part="${host_part%%/*}"
BP_MIGRATION_DATABASE_URL="postgres://platform_owner:${owner_pw}@${host_part}/${db}" ./target/release/platform-dbctl migrate >/dev/null
db_url="postgres://platform_app:${app_pw}@${host_part}/${db}"
mkdir -p perf/results target/perf
summary="$(mktemp)"
token="load-$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"

load_before="$(uptime | sed 's/.*load averages*: *//')"
# A port that already listens belongs to another process — k6 would load-test
# that stranger. Refuse before launching anything (a connect probe: HTTP or not).
for p in "$fake_port" "$server_port"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then
    echo "error: 127.0.0.1:$p already has a listener - held by another process" >&2
    exit 1
  fi
done
BP_FAKE_LISTEN="127.0.0.1:${fake_port}" BP_FAKE_RANDOM_PER_MILLION=1000 BP_FAKE_RANDOM_SEED=20261116 BP_FAKE_HANG_MS=3000 \
  ./target/release/fake-provider >"target/perf/fake-${stamp}.log" 2>&1 &
fake=$!
BP_DATABASE_URL="$db_url" BP_LISTEN="127.0.0.1:${server_port}" BP_ADMIN_TOKEN="$token" BP_LOG=warn \
  ./target/release/platform-server >"target/perf/server-${stamp}.log" 2>&1 &
server=$!
cleanup() {
  kill "$server" "${worker:-}" "$fake" 2>/dev/null || true
  rm -f "$summary"
}
trap cleanup EXIT
for _ in $(seq 1 100); do
  # The liveness check comes first: a dead child can mean the port was already
  # another product's, whose /readyz would keep answering.
  kill -0 "$server" 2>/dev/null || {
    echo "error: the server died before becoming ready; its log follows" >&2
    tail -20 "target/perf/server-${stamp}.log" >&2
    exit 1
  }
  curl -fsS "http://127.0.0.1:${server_port}/readyz" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://127.0.0.1:${server_port}/readyz" >/dev/null
if command -v lsof >/dev/null 2>&1; then
  listener="$(lsof -nP -tiTCP:"$server_port" -sTCP:LISTEN | head -1 || true)"
  [ "$listener" = "$server" ] || {
    echo "error: port $server_port is held by pid ${listener:-?}, not our server $server" >&2
    exit 1
  }
fi
BP_DATABASE_URL="$db_url" BP_PROVIDER_URL="http://127.0.0.1:${fake_port}" BP_WORKER_CONCURRENCY=32 \
  BP_CALL_TIMEOUT_MS=1000 BP_LEASE_MS=20000 BP_LOG=warn \
  ./target/release/platform-worker >"target/perf/worker-${stamp}.log" 2>&1 &
worker=$!

set +e
BASE_URL="http://127.0.0.1:${server_port}" TOKEN="$token" MODE="$mode" RATE="$rate" PROJECTS="$projects" TOTAL="${TOTAL:-}" WRITE_RATIO="${WRITE_RATIO:-}" \
  k6 run --quiet --summary-export "$summary" perf/scenarios/control-api.js
k6_status=$?
set -e
load_after="$(uptime | sed 's/.*load averages*: *//')"
server_rss_kb="$(ps -o rss= -p "$server" | tr -d ' ' || echo 0)"
worker_rss_kb="$(ps -o rss= -p "$worker" | tr -d ' ' || echo 0)"

# Convergence: the worker drains every operation (bounded wait).
drain_started=$(date +%s)
counts='{}'
for _ in $(seq 1 360); do
  counts="$(curl -fsS -H "Authorization: Bearer $token" "http://127.0.0.1:${server_port}/v1/operation-counts" || echo '{}')"
  if [ "$(jq '(.pending // 1) + (.running // 1)' <<<"$counts")" = "0" ]; then break; fi
  sleep 0.5
done
drain_seconds=$(( $(date +%s) - drain_started ))
inventory="$(curl -fsS "http://127.0.0.1:${fake_port}/_fake/inventory")"
duplicates="$(jq '.duplicates | length' <<<"$inventory")"
held="$(jq '.resources' <<<"$inventory")"
expected=$(( projects * 5 ))
drained="$(jq '((.pending // 1) + (.running // 1)) == 0' <<<"$counts")"
invariants_ok=false
if [ "$duplicates" = "0" ] && [ "$held" = "$expected" ] && [ "$drained" = "true" ]; then invariants_ok=true; fi

if [ "$(uname -s)" = "Darwin" ]; then
  cpu="$(sysctl -n machdep.cpu.brand_string)"; cores="$(sysctl -n hw.ncpu)"
  mem_gb="$(( $(sysctl -n hw.memsize) / 1073741824 ))"; os="macOS $(sw_vers -productVersion)"
else
  cpu="$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"; cores="$(nproc)"
  mem_gb="$(( $(grep MemTotal /proc/meminfo | awk '{print $2}') / 1048576 ))"; os="$(uname -sr)"
fi
commit="$(git rev-parse HEAD)"
dirty=false; git diff --quiet HEAD -- . ':!perf/results' || dirty=true
busy="$(awk '{gsub(",", "", $1); print ($1 > 6) ? "true" : "false"}' <<<"$load_before")"

outdir=perf/results
# A plumbing probe (TOTAL set) is not evidence: it never lands in perf/results.
if [ -n "${TOTAL:-}" ]; then outdir=target/perf; mode="probe-${TOTAL}"; fi
out="${outdir}/${stamp}-control-api-${mode}.json"
jq -n \
  --arg mode "$mode" --arg stamp "$stamp" --arg commit "$commit" --argjson dirty "$dirty" \
  --arg cpu "$cpu" --arg cores "$cores" --arg mem_gb "$mem_gb" --arg os "$os" \
  --arg load_before "$load_before" --arg load_after "$load_after" --argjson busy "$busy" \
  --argjson rate "$rate" --argjson projects "$projects" \
  --arg server_rss_kb "$server_rss_kb" --arg worker_rss_kb "$worker_rss_kb" \
  --argjson k6_status "$k6_status" --argjson counts "$counts" --argjson inventory "$inventory" \
  --argjson drain_seconds "$drain_seconds" --argjson expected "$expected" --argjson invariants_ok "$invariants_ok" \
  --slurpfile summary "$summary" '
  {
    scenario: "control-api", mode: $mode, started_utc: $stamp, commit: $commit, tree_dirty: $dirty,
    machine: {cpu: $cpu, cores: ($cores|tonumber), memory_gb: ($mem_gb|tonumber), os: $os,
              load_average_before: $load_before, load_average_after: $load_after,
              busy_machine: $busy,
              note: "same-machine load generator: a development observation, not capacity"},
    offered_rate_per_second: $rate, projects: $projects,
    mix: "80% reads (70% GET operation, 10% GET project), 20% PUT desired (each creates an operation)",
    provider: "fake-provider over HTTP, seeded random faults at 1000 per million calls",
    thresholds_passed: ($k6_status == 0), k6_exit_status: $k6_status,
    requests: $summary[0].metrics.http_reqs.count,
    requests_per_second: $summary[0].metrics.http_reqs.rate,
    failed_rate: $summary[0].metrics.http_req_failed.value,
    admission_refused_rate: ($summary[0].metrics.admission_refused.value // 0),
    dropped_iterations: ($summary[0].metrics.dropped_iterations.count // 0),
    latency_ms: ($summary[0].metrics.http_req_duration | {min, med, avg, "p(90)", "p(95)", "p(99)", max}),
    latency_ms_by_kind: {
      read: ($summary[0].metrics["http_req_duration{kind:read}"] // null | if . then {med, "p(95)", "p(99)", max} else null end),
      write: ($summary[0].metrics["http_req_duration{kind:write}"] // null | if . then {med, "p(95)", "p(99)", max} else null end)
    },
    convergence: {
      operations: $counts, drain_seconds_after_load: $drain_seconds,
      provider_resources: $inventory.resources, expected_resources: $expected,
      provider_duplicates: ($inventory.duplicates | length),
      provider_lookups: $inventory.lookups, provider_creates: $inventory.creates,
      provider_faults_injected: $inventory.faults_injected,
      invariants_passed: $invariants_ok
    },
    rss_kb_at_end: {server: ($server_rss_kb|tonumber), worker: ($worker_rss_kb|tonumber)},
    k6_summary: $summary[0].metrics
  }' >"$out"
echo "result: $out"
jq '{mode, commit, thresholds_passed, requests, requests_per_second, failed_rate, admission_refused_rate, dropped_iterations, latency_ms_by_kind, convergence, machine: {load_average_before: .machine.load_average_before, busy_machine: .machine.busy_machine}}' "$out"
kill "$server" "$worker" "$fake" 2>/dev/null || true
wait "$server" "$worker" 2>/dev/null || true
BP_ADMIN_DATABASE_URL="$admin_url" BP_DATABASE_NAME="$db" ./target/release/platform-dbctl drop >/dev/null || true
if [ "$k6_status" -ne 0 ] || [ "$invariants_ok" != "true" ]; then exit 1; fi
