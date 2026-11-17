#!/usr/bin/env bash
# Every limit is proven by a test that fails when the limit is removed.
#
# Each bound in the source carries a marker, `// LIMIT(name)`, on the line that sets it (or
# on a comment line just above it). For every marker this script removes the limit in a
# scratch copy — one substitution on that line — and runs the one test that proves it,
# which must then FAIL as a test (a build error is not a proof). A marker without a
# mutation below, or a mutation whose marker is gone, fails the script too, so a new limit
# cannot be added without its proof.
#
#   scripts/prove-limits.sh [--out FILE]   (FILE: a JSON record; never names the machine)
#
# Needs STARTER_TEST_DATABASE_URL (scripts/stack.sh env) for the database-backed tests.
# Builds into target/prove (this copy's own target directory); the scratch source is
# deleted at the end.
set -euo pipefail
cd "$(dirname "$0")/.."
here="$PWD"
out=""
if [ "${1:-}" = "--out" ]; then out="$2"; fi
: "${STARTER_TEST_DATABASE_URL:?set it (scripts/stack.sh env)}"

work="$(mktemp -d "${TMPDIR:-/tmp}/starter-prove.XXXXXX")"
trap 'rm -rf "$work"' EXIT
tar --exclude ./target --exclude ./.git -cf - . | tar -xf - -C "$work"
export CARGO_TARGET_DIR="$here/target/prove"

# name|file|pattern=>replacement (a Python regex, applied to the marked line)|package|test target|test
mutations=(
  "connection-cap|crates/starter-http/src/serve.rs|limits\.max_connections=>Semaphore::MAX_PERMITS|starter-http|limits|connections_past_the_cap_wait_in_the_backlog_until_a_slot_frees"
  "header-read|crates/starter-http/src/serve.rs|^.*$=>|starter-http|limits|a_trickled_header_is_cut_off_and_never_answered"
  "requests-per-connection|crates/starter-http/src/serve.rs|nth >= cap=>false|starter-http|limits|pipelined_requests_past_the_cap_close_the_connection"
  "write-stall|crates/starter-http/src/serve.rs|limits\.write_stall=>Duration::from_secs(3600)|starter-http|limits|non_reading_clients_cannot_hold_connection_slots"
  "lifetime|crates/starter-http/src/serve.rs|limits\.max_age=>Duration::from_secs(3600)|starter-http|limits|a_busy_connection_is_closed_after_its_lifetime"
  "drain|crates/starter-http/src/serve.rs|limits\.drain_timeout=>Duration::from_secs(3600)|starter-http|limits|shutdown_lets_a_request_finish_and_cuts_at_its_deadline"
  "in-flight|crates/starter-http/src/stack.rs|max_in_flight\.max\(1\)=>Semaphore::MAX_PERMITS|starter-http|limits|requests_past_the_in_flight_gate_are_shed_at_once"
  "request-timeout|crates/starter-http/src/stack.rs|limits\.request_timeout=>Duration::from_secs(3600)|starter-http|limits|a_stalled_body_is_answered_by_the_request_deadline"
  "body|crates/starter-http/src/stack.rs|limits\.max_body_bytes=>usize::MAX|starter-http|limits|an_oversized_body_is_refused_with_the_envelope"
  "per-key-table|crates/starter-http/src/budget.rs|^.*$=>|starter-http|lib|budget::tests::the_table_never_grows_past_its_capacity_and_keeps_the_throttled"
  "sign-in-per-address|crates/starter-accounts/src/signin.rs|self\.per_address\.take\(address\)\?;=>let _ = self.per_address.take(address);|starter-accounts|budgets|one_address_flood_bounds_only_that_address"
  "sign-in-global|crates/starter-accounts/src/signin.rs|self\.global\.take\(\)=>Ok::<(), RetryAfter>(())|starter-accounts|budgets|a_distributed_flood_cannot_lock_out_a_trusted_owner"
  "mail-per-address|crates/starter-accounts/src/signin.rs|self\.per_address\.take\(address\)\?;=>let _ = self.per_address.take(address);|starter-accounts|budgets|mail_a_client_can_cause_is_bounded"
  "password-concurrency|crates/starter-accounts/src/password.rs|concurrency\.max\(1\)=>Semaphore::MAX_PERMITS|starter-accounts|lib|password::tests::at_most_the_concurrency_hashes_at_once"
  "plan|crates/starter-accounts/src/domain.rs|current < i64::from\(limit\)=>true|starter-accounts|limits|token_and_workspace_limits_hold_under_concurrency"
  "ready-deadline|crates/starter-server/src/health.rs|self\.deadline=>Duration::from_secs(3600)|starter-server|health|readyz_reports_a_silent_database_within_its_deadline"
)

# Every marker in the source has exactly one mutation, and every mutation a marker.
found="$(grep -rhoE 'LIMIT\([a-z-]+\)' crates/*/src | sed 's/LIMIT(\(.*\))/\1/' | sort -u)"
defined="$(printf '%s\n' "${mutations[@]}" | cut -d'|' -f1 | sort -u)"
if [ "$found" != "$defined" ]; then
  echo "error: markers and mutations differ:" >&2
  diff <(echo "$found") <(echo "$defined") >&2 || true
  exit 1
fi

mutate() { # file marker pattern replacement
  python3 -I - "$work/$1" "$2" "$3" "$4" <<'PY'
import re, sys
path, marker, pattern, replacement = sys.argv[1:]
lines = open(path).read().split("\n")
tag = f"LIMIT({marker})"
changed = 0
for i, line in enumerate(lines):
    if tag not in line:
        continue
    target = i if line.split("//")[0].strip() else i + 1
    new = re.sub(pattern, replacement, lines[target], count=1)
    if new != lines[target]:
        lines[target] = new
        changed += 1
if not changed:
    sys.exit(f"the mutation for {marker} changed nothing")
open(path, "w").write("\n".join(lines))
PY
}

failures=0
results=()
for row in "${mutations[@]}"; do
  IFS='|' read -r name file subst package target test <<<"$row"
  pattern="${subst%%=>*}"
  replacement="${subst#*=>}"
  cp "$here/$file" "$work/$file"
  mutate "$file" "$name" "$pattern" "$replacement"
  if [ "$target" = lib ]; then selector=(--lib); else selector=(--test "$target"); fi
  started=$(date +%s)
  set +e
  output="$(cd "$work" && cargo test --locked -q -p "$package" "${selector[@]}" -- --exact "$test" 2>&1)"
  status=$?
  set -e
  seconds=$(( $(date +%s) - started ))
  cp "$here/$file" "$work/$file"
  if [ "$status" -ne 0 ] && grep -q 'test result: FAILED' <<<"$output"; then
    verdict=proven
    printf '  proven   %-24s %s fails without it (%ss)\n' "$name" "$test" "$seconds"
  elif [ "$status" -ne 0 ]; then
    verdict=broken
    failures=$((failures + 1))
    printf '  BROKEN   %-24s the mutation did not build or the test did not run:\n%s\n' "$name" "$(tail -20 <<<"$output")"
  else
    verdict=unproven
    failures=$((failures + 1))
    printf '  UNPROVEN %-24s %s passes without the limit\n' "$name" "$test"
  fi
  results+=("{\"limit\":\"$name\",\"test\":\"$test\",\"verdict\":\"$verdict\",\"seconds\":$seconds}")
done

if [ -n "$out" ]; then
  commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
  dirty=false; git diff --quiet HEAD -- . 2>/dev/null || dirty=true
  load="$(uptime | sed 's/.*load averages*: *//')"
  printf '{"proof":"limits","commit":"%s","tree_dirty":%s,"load_average":"%s","os":"%s","results":[%s]}\n' \
    "$commit" "$dirty" "$load" "$(uname -sr)" "$(IFS=,; echo "${results[*]}")" >"$out"
  echo "record: $out"
fi
if (( failures )); then
  echo "error: $failures of ${#mutations[@]} limits are not proven" >&2
  exit 1
fi
echo "limits ok: all ${#mutations[@]} limits are proven by a test that fails without them"
