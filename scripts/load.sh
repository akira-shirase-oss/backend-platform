#!/usr/bin/env bash
# Runs the control-API load scenario against release builds and keeps the evidence
# (workspace load-test standard, D-018).
#
#   MODE=smoke scripts/load.sh    100,000 requests (the B0 baseline)
#   MODE=full  scripts/load.sh    1,000,000 requests — only on a quiet machine
#
# It starts, on 127.0.0.1, the fake provider (61410) with a seeded 0.1% fault schedule,
# the worker (no listener) and the server (61400), on a fresh control database in the
# local stack; offers load with k6 at RATE requests/s (default 2000); then waits for the
# worker to drain every operation and reads the provider side: duplicates must be zero and
# the provider must hold exactly PROJECTS × 5 resources. Every result records the machine,
# the commit (and whether the tree was dirty) and the load average before and after; a
# result taken with a load average above 6 is marked as a busy-machine observation. A
# threshold or invariant miss fails the run and is kept as evidence, never rerun until
# green. Results go to perf/results/ in both modes.
#
# Needs: k6 (scripts/install-k6.sh), jq, curl, and docker for the local stack (or
# BP_LOAD_DATABASE_URL naming an empty database).
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${MODE:-smoke}"
rate="${RATE:-2000}"
projects="${PROJECTS:-2000}"
server_port=61400
fake_port=61410
case "$mode" in smoke|full) ;; *) echo "MODE must be smoke or full" >&2; exit 2 ;; esac

cargo build --release --locked -p platform-server -p platform-worker -p platform-fake-provider >/dev/null
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
# A fresh control database per run in the local stack; CI passes its own (BP_LOAD_DATABASE_URL).
db=""
if [ -n "${BP_LOAD_DATABASE_URL:-}" ]; then
  db_url="$BP_LOAD_DATABASE_URL"
else
  scripts/stack.sh up >/dev/null
  db="bp_load_$(date -u +%Y%m%d%H%M%S)"
  docker compose -f deploy/compose.yaml exec -T postgres psql -q -U platform -d platform -c "CREATE DATABASE $db" >/dev/null
  db_url="postgres://platform:platform-dev@127.0.0.1:61432/$db"
fi
mkdir -p perf/results target/perf
summary="$(mktemp)"
token="load-$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"

load_before="$(uptime | sed 's/.*load averages*: *//')"
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
  curl -fsS "http://127.0.0.1:${server_port}/readyz" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://127.0.0.1:${server_port}/readyz" >/dev/null
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
if [ -n "$db" ]; then
  docker compose -f deploy/compose.yaml exec -T postgres psql -q -U platform -d platform -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" >/dev/null || true
fi
if [ "$k6_status" -ne 0 ] || [ "$invariants_ok" != "true" ]; then exit 1; fi
