#!/usr/bin/env bash
# Every attack test is proven to fail when the protection it guards is removed.
#
# Each protection in the source carries a marker, `// ATTACK(name)`, on the line that
# enforces it (or on a comment line just above it). For every marker this script removes
# or weakens the protection in a scratch copy — one substitution on that line — and runs
# the named attack test, which must then FAIL as a test (a build error is not a proof).
# The table must cover every test in tests/attacks/tests/: a new attack test that is not
# named here fails the script, so a test can never silently guard nothing. A marker
# without a mutation, or a mutation whose marker is gone, fails the script too.
#
#   scripts/prove-attacks.sh [--out FILE]   (FILE: a JSON record; never names the machine)
#
# Needs BP_TEST_DATABASE_URL (scripts/stack.sh env) for the database-backed tests.
# Builds into target/prove-attacks (this copy's own target directory); the scratch source
# is deleted at the end.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
cd "$(dirname "$0")/.."
here="$PWD"
out=""
case "${1:-}" in
  "") ;;
  --out) out="${2:?--out needs a file}" ;;
  *) echo "prove-attacks.sh: unknown argument $1 (see --help)" >&2; exit 2 ;;
esac
: "${BP_TEST_DATABASE_URL:?is not set. Start the database and export its address: scripts/stack.sh up; eval \"\$(scripts/stack.sh env)\"}"

# The proof runs over committed state: an uncommitted or untracked file would
# carry markers, tests or protections into the scratch copy that CI never sees.
if [ -n "$(git status --porcelain -- .)" ]; then
  echo "error: prove-attacks.sh needs a clean tree - commit or stash first:" >&2
  git status --porcelain -- . >&2
  exit 1
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/platform-prove-attacks.XXXXXX")"
trap 'rm -rf "$work"' EXIT
tar --exclude ./target --exclude ./.git -cf - . | tar -xf - -C "$work"
export CARGO_TARGET_DIR="$here/target/prove-attacks"

# name|file|pattern<TAB>replacement (a Python regex, applied near the marker)|package|test target|test
mutations=(
  # --- auth_bypass ---
  "anonymous-refused|crates/platform-server/src/auth.rs|_ => ApiError::Unauthorized\.into_response\(\),	_ => next.run(request).await,|platform-attacks|auth_bypass|no_authorization_header_is_refused_on_every_protected_route"
  "anonymous-refused|crates/platform-server/src/auth.rs|_ => ApiError::Unauthorized\.into_response\(\),	_ => next.run(request).await,|platform-attacks|auth_bypass|a_token_in_the_query_string_is_not_authentication"
  "anonymous-refused|crates/platform-server/src/auth.rs|_ => ApiError::Unauthorized\.into_response\(\),	_ => next.run(request).await,|platform-attacks|auth_bypass|empty_and_control_byte_authorization_values_are_refused"
  "require-admin|crates/platform-server/src/auth.rs|constant_time_eq\(token\.as_bytes\(\), state\.admin_token\.expose\(\)\.as_bytes\(\)\)	true|platform-attacks|auth_bypass|malformed_bearer_headers_are_refused"
  "dup-authorization|crates/platform-server/src/auth.rs|values\.next\(\)\.is_some\(\)	values.next().is_none()|platform-attacks|auth_bypass|duplicated_authorization_headers_are_refused"
  "livez-minimal|crates/platform-server/src/handlers.rs|\bok\b	ok — internals leaked: pool=PgPool(0x7f) admin_token=Secret(admin-…) xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx|platform-attacks|auth_bypass|health_and_readiness_stay_open_but_reveal_nothing"
  # --- bodies ---
  "body-limit|crates/platform-server/src/middleware.rs|config\.max_body_bytes	usize::MAX|platform-attacks|bodies|a_body_past_the_limit_is_413_whether_declared_or_streamed"
  "request-timeout|crates/platform-server/src/middleware.rs|config\.request_timeout	std::time::Duration::from_secs(3600)|platform-attacks|bodies|a_slow_dribbled_body_times_out_without_taking_the_server_down"
  "invalid-is-400|crates/platform-server/src/error.rs|StatusCode::BAD_REQUEST	StatusCode::INTERNAL_SERVER_ERROR|platform-attacks|bodies|malformed_json_and_wrong_content_type_are_4xx_never_5xx"
  # --- fuzz ---
  "invalid-is-400|crates/platform-server/src/error.rs|StatusCode::BAD_REQUEST	StatusCode::INTERNAL_SERVER_ERROR|platform-attacks|fuzz|request_bodies_through_the_router_never_5xx"
  "header-parse|crates/platform-server/src/auth.rs|value\.to_str\(\)\.ok\(\)\?	value.to_str().unwrap()|platform-attacks|fuzz|seed_corpus_replays_without_panics"
  "header-parse|crates/platform-server/src/auth.rs|value\.to_str\(\)\.ok\(\)\?	value.to_str().unwrap()|platform-attacks|fuzz|header_parsers_never_panic"
  "id-parse|crates/platform-domain/src/ids.rs|check_len\(value, WHAT, &PROJECT_ID_LEN\)\?	check_len(value, WHAT, &PROJECT_ID_LEN).unwrap()|platform-attacks|fuzz|ids_never_panic_on_arbitrary_text"
  "name-parse|crates/platform-domain/src/ids.rs|check_len\(value, WHAT, &LOGICAL_NAME_LEN\)\?	check_len(value, WHAT, &LOGICAL_NAME_LEN).unwrap()|platform-attacks|fuzz|desired_state_json_never_panics"
  "provider-id-parse|crates/platform-domain/src/ids.rs|check_len\(value, WHAT, &\(1\.\.=MAX_PROVIDER_ID_LEN\)\)\?	check_len(value, WHAT, &(1..=MAX_PROVIDER_ID_LEN)).unwrap()|platform-attacks|fuzz|provider_replies_never_panic"
  "config-parse|crates/platform-server/src/config.rs|Self::default\(\)	if true { panic!(\"config\") } else { Self::default() }|platform-attacks|fuzz|config_parsers_never_panic_or_accept_garbage"
  "admin-token-len|crates/platform-server/src/config.rs|admin_token\.len\(\) < MIN_ADMIN_TOKEN_LEN	admin_token.len() > MIN_ADMIN_TOKEN_LEN|platform-attacks|fuzz|config_refuses_garbage_values"
  # --- headers ---
  "anonymous-refused|crates/platform-server/src/auth.rs|_ => ApiError::Unauthorized\.into_response\(\),	_ => next.run(request).await,|platform-attacks|headers|forwarding_headers_do_not_elevate_an_anonymous_request"
  "request-id-check|crates/platform-server/src/middleware.rs|\(1\.\.=MAX_REQUEST_ID_LEN\)\.contains\(&value\.len\(\)\)	true|platform-attacks|headers|a_valid_shaped_spoofed_request_id_is_propagated_and_an_invalid_one_replaced"
  # --- ratelimit ---
  "shed-gate|crates/platform-server/src/middleware.rs|try_acquire\(\)	try_acquire_many(0)|platform-attacks|ratelimit|the_shed_gate_cannot_be_evaded_by_spoofed_client_headers"
  # --- replay ---
  "idempotency-replay|crates/platform-db/src/desired.rs|idempotency_key$	None::<&str>|platform-attacks|replay|the_same_key_replays_the_same_operation"
  "idempotency-conflict|crates/platform-db/src/desired.rs|Err\(AdmitError::IdempotencyConflict\)	Ok(Admitted { operation_id: OperationId(id), revision, replayed: true })|platform-attacks|replay|the_same_key_with_a_different_payload_is_refused"
  "idempotency-key-shape|crates/platform-server/src/handlers.rs|is_ascii_graphic	is_ascii|platform-attacks|replay|malformed_idempotency_keys_are_refused"
  # --- ssrf ---
  "known-fields|crates/platform-domain/src/plan.rs|deny_unknown_fields	default|platform-attacks|ssrf|desired_state_refuses_every_smuggled_url_shape"
  "name-charset|crates/platform-domain/src/ids.rs|b\.is_ascii_lowercase\(\) \x7c\x7c b\.is_ascii_digit\(\) \x7c\x7c b == b'_'	true|platform-attacks|ssrf|desired_state_refuses_every_smuggled_url_shape"
  "project-id-parse|crates/platform-server/src/handlers.rs|ProjectId::parse\(raw\)\.map_err\(\x7ce\x7c ApiError::Invalid\(e\.to_string\(\)\)\)	Ok(ProjectId::parse(raw).unwrap())|platform-attacks|ssrf|a_url_shaped_project_or_operation_is_never_a_target"
  # --- tenant ---
  "project-scope|crates/platform-db/src/desired.rs|WHERE project = \\\$1	WHERE project != \$1|platform-attacks|tenant|another_projects_state_is_not_visible"
  "operation-lookup|crates/platform-db/src/log.rs|WHERE o\.id = \\\$1	WHERE (o.id = $1 OR TRUE)|platform-attacks|tenant|operation_ids_are_not_guessable"
  "no-fallback|crates/platform-server/src/lib.rs|Router::new\(\)	Router::new().fallback(get(handlers::livez))|platform-attacks|tenant|path_traversal_and_encoding_tricks_are_refused"
)

# Every marker in the source has at least one mutation, and every mutation a marker.
found="$(grep -rhoE 'ATTACK\([a-z0-9-]+\)' crates/*/src | sed 's/ATTACK(\(.*\))/\1/' | sort -u)"
defined="$(printf '%s\n' "${mutations[@]}" | cut -d'|' -f1 | sort -u)"
if [ "$found" != "$defined" ]; then
  echo "error: markers and mutations differ:" >&2
  diff <(echo "$found") <(echo "$defined") >&2 || true
  exit 1
fi

# Tests whose protection lives in a dependency (hyper's HTTP/1 parser) cannot be flipped
# by a source edit. Listed with the reason; still must exist; still count as covered.
EXEMPTIONS=(
  "a_head_with_content_length_and_transfer_encoding_reads_chunked|hyper's HTTP/1 parser disambiguates to chunked (RFC 9112); no in-tree line to mutate"
  "conflicting_content_length_headers_are_refused|hyper's HTTP/1 parser refuses ambiguous framing; no in-tree line to mutate"
)

# Every attack test is named in the table — a test that guards nothing cannot land.
python3 -I - tests/attacks/tests <<'PY' > "$work/attack-tests"
import re, sys, os
for name in sorted(os.listdir(sys.argv[1])):
    if name == "common" or not name.endswith(".rs"):
        continue
    src = open(os.path.join(sys.argv[1], name)).read()
    for m in re.finditer(r'#(?:\[(?:tokio::)?test)[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:async\s+)?fn\s+([a-z_0-9]+)', src):
        print(m.group(1))
PY
covered="$(printf '%s\n' "${mutations[@]}" | cut -d'|' -f6 | sort -u)"
exempted="$(printf '%s\n' "${EXEMPTIONS[@]}" | cut -d'|' -f1 | sort -u)"
actual="$(sort -u "$work/attack-tests")"
if [ "$(printf '%s\n%s\n' "$covered" "$exempted" | sort -u)" != "$actual" ]; then
  echo "error: attack tests without a mutation or exemption (or stale rows):" >&2
  diff <(echo "$actual") <(printf '%s\n%s\n' "$covered" "$exempted" | sort -u) >&2 || true
  exit 1
fi
for row in "${EXEMPTIONS[@]}"; do
  test="${row%%|*}"
  grep -rln "fn ${test}\b" tests/attacks/tests >/dev/null || {
    echo "error: exempted test $test does not exist" >&2
    exit 1
  }
  echo "exempt: $test (${row#*|})"
done

mutate() { # file marker pattern replacement
  python3 -I - "$work/$1" "$2" "$3" "$4" <<'PY'
import re, sys
path, marker, pattern, replacement = sys.argv[1:]
lines = open(path).read().split("\n")
tag = f"ATTACK({marker})"
changed = 0
for i, line in enumerate(lines):
    if tag not in line:
        continue
    start = i if line.split("//")[0].strip() else i + 1
    for target in range(start, min(start + 8, len(lines))):
        new = re.sub(pattern, replacement, lines[target], count=1)
        if new != lines[target]:
            lines[target] = new
            changed += 1
            break
if not changed:
    sys.exit(f"the mutation for {marker} changed nothing")
open(path, "w").write("\n".join(lines))
PY
}

# Baseline: every test the table names must pass on the unmutated tree, or a
# later failure is a test that was already red - not the mutation's work.
for package in $(printf '%s\n' "${mutations[@]}" | cut -d'|' -f4 | sort -u); do
  echo "baseline: $package tests on the unmutated tree"
  if ! (cd "$work" && cargo test --locked -q -p "$package" --tests); then
    echo "error: baseline failed - the unmutated suite is already red" >&2
    exit 1
  fi
done

failures=0
ran=0
results=()
declare -a proven_runs
for row in "${mutations[@]}"; do
  IFS='|' read -r name file subst package target test <<<"$row"
  [ -n "${PROVE_ONLY:-}" ] && [ "$PROVE_ONLY" != "$name" ] && continue
  ran=$((ran + 1))
  pattern="${subst%%$'\t'*}"
  replacement="${subst#*$'\t'}"
  key="$name|$file|$subst"
  # A marker mutation already applied this round must be reapplied per row only when the
  # file was restored; rows sharing a marker each get their own run.
  cp "$here/$file" "$work/$file"
  mutate "$file" "$name" "$pattern" "$replacement"
  if [ "$target" = lib ]; then selector=(--lib); else selector=(--test "$target"); fi
  started=$(date +%s)
  set +e
  output="$(cd "$work" && cargo test --locked -q -p "$package" "${selector[@]}" -- --exact "$test" 2>&1)"
  status=$?
  set -e
  seconds=$(( $(date +%s) - started ))
  if [ "$status" -ne 0 ] && grep -q 'test result: FAILED' <<<"$output" \
      && grep -qF "test ${test} ... FAILED" <<<"$output"; then
    verdict=proven
    cp "$here/$file" "$work/$file"
    printf '  proven   %-24s %s fails without it (%ss)\n' "$name" "$test" "$seconds"
  else
    diff "$here/$file" "$work/$file" | head -20 >&2 || true
    cp "$here/$file" "$work/$file"
    if [ "$status" -ne 0 ]; then
      verdict=broken
      failures=$((failures + 1))
      printf '  BROKEN   %-24s the mutation did not build or the test did not run:\n%s\n' "$name" "$(tail -20 <<<"$output")"
    else
      verdict=unproven
      failures=$((failures + 1))
      printf '  UNPROVEN %-24s %s passes without the protection:\n%s\n' "$name" "$test" "$(tail -8 <<<"$output")"
    fi
  fi
  results+=("{\"protection\":\"$name\",\"test\":\"$test\",\"verdict\":\"$verdict\",\"seconds\":$seconds}")
done

if [ -n "$out" ]; then
  commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
  dirty=false; git diff --quiet HEAD -- . 2>/dev/null || dirty=true
  load="$(uptime | sed 's/.*load averages*: *//')"
  printf '{"proof":"attacks","commit":"%s","tree_dirty":%s,"load_average":"%s","os":"%s","results":[%s]}\n' \
    "$commit" "$dirty" "$load" "$(uname -sr)" "$(IFS=,; echo "${results[*]}")" >"$out"
  echo "record: $out"
fi
if [ "$ran" -eq 0 ]; then
  echo "error: no mutations ran (PROVE_ONLY='${PROVE_ONLY:-}' matched nothing)" >&2
  exit 1
fi
if (( failures )); then
  echo "error: $failures of $ran attack mutations are not proven" >&2
  exit 1
fi
echo "attacks ok: all $ran mutations are proven to fail their attack test"
