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
# Needs STARTER_TEST_DATABASE_URL (scripts/stack.sh env) for the database-backed tests.
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
: "${STARTER_TEST_DATABASE_URL:?is not set. Start the database and export its address: scripts/stack.sh up; eval \"\$(scripts/stack.sh env)\"}"

# The proof is of a commit and must run inside a work tree.
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "error: prove-attacks.sh must run inside a git work tree" >&2
  exit 1
}

# The proof runs over committed state: an uncommitted or untracked file would
# carry markers, tests or protections into the scratch copy that CI never sees.
if [ -n "$(git status --porcelain -- .)" ]; then
  echo "error: prove-attacks.sh needs a clean tree - commit or stash first:" >&2
  git status --porcelain -- . >&2
  exit 1
fi

# Both scratch copies come from `git archive HEAD`, so an editor save mid-run
# cannot leak in. The scratch path is FIXED: env!("CARGO_MANIFEST_DIR") bakes
# the manifest dir into test binaries and tar-preserved mtimes let cargo reuse
# a binary built for a since-deleted dir — the corpus read then panics with a
# phantom NotFound. Two concurrent runs would clobber that fixed path; the
# mkdir lock is the atomic exclusion.
mkdir -p target
lock="$here/target/prove-attacks.lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "error: another prove-attacks run holds $lock (a crashed run leaves it: rm -rf it)" >&2
  exit 1
fi
work="$here/target/prove-attacks-work"
pristine="$here/target/prove-attacks-pristine"
rm -rf "$work" "$pristine"
mkdir "$work" "$pristine"
trap 'rm -rf "$work" "$pristine" "$lock"' EXIT
commit="$(git rev-parse HEAD)"
git archive "$commit" | tar -x -C "$work"
git archive "$commit" | tar -x -C "$pristine"
export CARGO_TARGET_DIR="$here/target/prove-attacks"
# Binaries baked by earlier runs hold a since-renamed manifest path; start fresh.
rm -rf "$here/target/prove-attacks"
# A caller's RUSTFLAGS (a sanitizer, -D warnings) would skew the proof build.
export RUSTFLAGS=

# name|file|pattern<TAB>replacement (a Python regex, applied near the marker)|package|test target|test
mutations=(
  # --- auth_bypass ---
  "access-bypass|crates/starter-accounts/src/lib.rs|match self\.principal\(headers\)\.await\? \{	match (Principal::Session { user: Uuid::nil() }) {|starter-attacks|auth_bypass|protected_routes_refuse_anonymous_requests"
  "invite-required|crates/starter-accounts/src/store.rs|claimed\.rows_affected\(\) != 1	claimed.rows_affected() == 1|starter-attacks|auth_bypass|public_routes_stay_strict"
  "dup-authorization|crates/starter-accounts/src/lib.rs|authorization\.next\(\)\.is_some\(\)	authorization.next().is_none()|starter-attacks|auth_bypass|duplicate_authorization_headers_are_refused_in_either_order"
  "dup-session-cookie|crates/starter-accounts/src/lib.rs|pairs\.next\(\)\.is_some\(\)	pairs.next().is_none()|starter-attacks|auth_bypass|duplicate_session_cookies_are_refused"
  "token-lookup|crates/starter-accounts/src/lib.rs|ok_or\(AccountsError::Unauthenticated\)\?;	unwrap_or((Uuid::nil(), Uuid::nil()));|starter-attacks|auth_bypass|garbage_sessions_and_tokens_are_refused"
  "origin-check|crates/starter-accounts/src/routes.rs|!= Some\(accounts\.settings\(\)\.public_url\.as_str\(\)\)	== Some(accounts.settings().public_url.as_str())|starter-attacks|auth_bypass|cross_origin_cookie_posts_are_refused"
  "origin-check|crates/starter-accounts/src/routes.rs|!= Some\(accounts\.settings\(\)\.public_url\.as_str\(\)\)	== Some(accounts.settings().public_url.as_str())|starter-attacks|auth_bypass|a_product_post_is_csrf_guarded_too"
  "csrf-v1|crates/starter-server/src/lib.rs|let router = router\.layer\(csrf\);	let _ = router.clone().layer(csrf);|starter-attacks|auth_bypass|a_product_post_is_csrf_guarded_too"
  # --- bodies ---
  "body-limit|crates/starter-http/src/stack.rs|limits\.max_body_bytes	usize::MAX|starter-attacks|bodies|a_body_past_the_limit_is_413_whether_declared_or_streamed"
  "request-timeout|crates/starter-http/src/stack.rs|limits\.request_timeout,	Duration::from_secs(3600),|starter-attacks|bodies|a_slow_dribbled_body_times_out_and_the_server_stays_up"
  "invalid-is-422|crates/starter-accounts/src/error.rs|UNPROCESSABLE_ENTITY	INTERNAL_SERVER_ERROR|starter-attacks|replay|an_email_with_header_injection_is_refused"
  "known-fields|crates/starter-accounts/src/routes.rs|deny_unknown_fields	crate = \"serde\"|starter-attacks|bodies|malformed_bodies_are_4xx_never_5xx"
  # --- fuzz ---
  "cookie-parse|crates/starter-accounts/src/lib.rs|v\.to_str\(\)\.ok\(\)	Some(v.to_str().unwrap())|starter-attacks|fuzz|seed_corpus_replays_without_panics"
  "cookie-parse|crates/starter-accounts/src/lib.rs|v\.to_str\(\)\.ok\(\)	Some(v.to_str().unwrap())|starter-attacks|fuzz|session_cookie_never_panics"
  "auth-parse|crates/starter-accounts/src/lib.rs|value\.to_str\(\)\.ok\(\)\?	value.to_str().unwrap()|starter-attacks|fuzz|authorization_never_panics"
  "auth-parse|crates/starter-accounts/src/lib.rs|value\.to_str\(\)\.ok\(\)\?	value.to_str().unwrap()|starter-attacks|fuzz|seed_corpus_replays_without_panics"
  "token-parse|crates/starter-accounts/src/tokens.rs|secret\.len\(\) == 64	secret.bytes().nth(70).unwrap().is_ascii()|starter-attacks|fuzz|token_parsers_never_panic"
  "xff-parse|crates/starter-http/src/client.rs|hop\.trim\(\)\.parse::<IpAddr>\(\)	Ok::<IpAddr, std::net::AddrParseError>(hop.trim().parse::<IpAddr>().unwrap())|starter-attacks|fuzz|xff_parsing_never_panic_or_reads_past_the_rightmost_entry"
  "email-parse|crates/starter-accounts/src/domain.rs|split_once\('@'\)\?	split_once('@').unwrap()|starter-attacks|fuzz|validators_never_panic"
  "public-url-loopback|crates/starter-server/src/config.rs|if !loopback	if false|starter-attacks|fuzz|config_refuses_garbage_values_and_names_the_variable"
  "config-parse|crates/starter-server/src/config.rs|default_of\(name\)\.to_owned\(\)	panic!(\"config\")|starter-attacks|fuzz|config_never_panics_or_accepts_garbage"
  # --- headers ---
  "header-cap|crates/starter-http/src/serve.rs|max_buf_size\(64 \* 1024\)	max_buf_size(usize::MAX)|starter-attacks|headers|oversized_headers_are_431"
  "header-read|crates/starter-http/src/serve.rs|header_read_timeout\(limits\.header_read_timeout\)	header_read_timeout(Duration::from_secs(3600))|starter-attacks|headers|dribbled_headers_are_closed_by_the_deadline"
  "xff-trust|crates/starter-http/src/client.rs|trusted\.contains\(&peer\.ip\(\)\)	true|starter-attacks|headers|xff_from_an_untrusted_peer_does_not_evasion_the_signin_budget"
  "xff-hop|crates/starter-http/src/client.rs|line\.rsplit\(','\)\.next\(\)	line.split(',').next()|starter-attacks|headers|from_a_trusted_proxy_only_the_rightmost_xff_entry_is_believed"
  # --- ratelimit ---
  "signin-budget|crates/starter-accounts/src/signin.rs|self\.per_address\.take\(address\)\?;	let _ = self.per_address.take(address);|starter-attacks|ratelimit|the_per_address_budget_refuses_a_flood_and_says_nothing"
  "mail-budget|crates/starter-accounts/src/signin.rs|self\.per_address\.take\(address\)\?;	let _ = self.per_address.take(address);|starter-attacks|ratelimit|mail_routes_share_the_per_address_budget"
  # --- replay ---
  "invite-single-use|crates/starter-accounts/src/store.rs|used_at IS NULL AND	|starter-attacks|replay|an_invite_is_spent_once"
  "verify-single-use|crates/starter-accounts/src/store.rs|used_at IS NULL AND	|starter-attacks|replay|a_verify_token_cannot_be_replayed"
  "reset-single-use|crates/starter-accounts/src/store.rs|used_at IS NULL AND	|starter-attacks|replay|a_reset_token_is_spent_once"
  "reset-expiry|crates/starter-accounts/src/store.rs|AND expires_at > now\(\)	|starter-attacks|replay|an_expired_reset_token_is_refused"
  "sign-out-revokes|crates/starter-accounts/src/store.rs|DELETE FROM sessions WHERE digest = \\\$1	UPDATE sessions SET user_id = user_id WHERE digest = \$1|starter-attacks|replay|a_signed_out_session_is_dead"
  "signin-indistinct|crates/starter-accounts/src/routes.rs|found\.map\(\x7cu\x7c \(u\.id, u\.password_hash\)\)	match found { Some(u) => Some((u.id, u.password_hash)), None => return Err(AccountsError::NotFound) }|starter-attacks|replay|sign_in_does_not_disclose_which_side_failed"
  "email-shape|crates/starter-accounts/src/routes.rs|domain::normalize_email\(raw\)	Some(raw.to_owned())|starter-attacks|replay|an_email_with_header_injection_is_refused"
  "signup-no-disclosure|crates/starter-accounts/src/store.rs|return Err\(AccountsError::EmailTaken\)	return Ok(())|starter-attacks|replay|a_duplicate_signup_is_refused_without_disclosure"
  "session-rotation|crates/starter-accounts/src/routes.rs|let session = tokens::new_hex_secret\(\)\.map_err\(internal\)\?;	let session = \"0\".repeat(64);|starter-attacks|replay|every_sign_in_rotates_the_session_id"
  # --- ssrf ---
  "reset-link-host|crates/starter-accounts/src/routes.rs|accounts\.settings\(\)\.public_url	\"http://attacker.example\".to_owned()|starter-attacks|ssrf|reset_links_come_from_the_configured_url_not_the_request"
  "email-shape|crates/starter-accounts/src/routes.rs|domain::normalize_email\(raw\)	Some(raw.to_owned())|starter-attacks|ssrf|user_controlled_fields_can_not_make_the_server_fetch"
  # --- tenant ---
  "membership|crates/starter-accounts/src/lib.rs|ok_or\(AccountsError::NotFound\)\?	unwrap_or(domain::Role::Member)|starter-attacks|tenant|a_workspace_is_invisible_to_a_non_member"
  "membership|crates/starter-accounts/src/lib.rs|ok_or\(AccountsError::NotFound\)\?	unwrap_or(domain::Role::Member)|starter-attacks|tenant|resource_ids_of_other_users_are_not_guessable"
  "token-scope|crates/starter-accounts/src/lib.rs|own != workspace	false|starter-attacks|tenant|an_api_token_sees_only_its_own_workspace_and_only_reads"
  "token-read-only|crates/starter-accounts/src/lib.rs|action != Action::Read	false|starter-attacks|tenant|an_api_token_is_refused_every_write_at_the_access_layer"
  "me-scoping|crates/starter-accounts/src/lib.rs|Principal::Session \{ user \} => Ok\(user\)	Principal::Session { .. } => Ok(Uuid::nil())|starter-attacks|tenant|me_only_ever_returns_the_callers_own_account"
  "role-allows|crates/starter-accounts/src/lib.rs|!allows\(role, action\)	false|starter-attacks|tenant|a_member_cannot_perform_owner_actions"
)

# Tests whose protection lives in a dependency (hyper's HTTP/1 parser) or is structural
# (parameterised queries) cannot be flipped by a source edit. They are listed here with
# the reason, still must exist, and still count toward coverage — but carry no mutation.
EXEMPTIONS=(
  "a_head_with_content_length_and_transfer_encoding_reads_chunked|hyper's HTTP/1 parser disambiguates to chunked (RFC 9112); no in-tree line to mutate"
  "conflicting_content_length_headers_are_refused|hyper's HTTP/1 parser refuses ambiguous framing; no in-tree line to mutate"
  "sql_metacharacters_round_trip_as_data|parameterised queries are structural across store.rs, not a flippable line"
  "request_bodies_through_the_router_never_5xx|axum's Json extractor refuses malformed bodies before a route runs; no in-tree line to mutate"
)

# Every marker in the source has at least one mutation, and every mutation a marker.
found="$(grep -rhoE 'ATTACK\([a-z0-9-]+\)' crates/*/src | sed 's/ATTACK(\(.*\))/\1/' | sort -u)"
defined="$(printf '%s\n' "${mutations[@]}" | cut -d'|' -f1 | sort -u)"
if [ "$found" != "$defined" ]; then
  echo "error: markers and mutations differ:" >&2
  diff <(echo "$found") <(echo "$defined") >&2 || true
  exit 1
fi

# Every attack test is named in the table — a test that guards nothing cannot land.
python3 -I - "$pristine/tests/attacks/tests" <<'PY' > "$work/attack-tests"
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
  grep -q "^${test}$" "$work/attack-tests" || {
    echo "error: exempted test $test does not exist" >&2
    exit 1
  }
  echo "exempt: $test (${row#*|})"
done

mutate() { # file marker pattern replacement -> the 1-based line it changed
  python3 -I - "$work/$1" "$2" "$3" "$4" <<'PY'
import re, sys
path, marker, pattern, replacement = sys.argv[1:]
lines = open(path).read().split("\n")
tag = f"ATTACK({marker})"
first = None
for i, line in enumerate(lines):
    if tag not in line:
        continue
    start = i if line.split("//")[0].strip() else i + 1
    for target in range(start, min(start + 8, len(lines))):
        new = re.sub(pattern, replacement, lines[target], count=1)
        if new != lines[target]:
            lines[target] = new
            if first is None:
                first = target
            break
if first is None:
    sys.exit(f"the mutation for {marker} changed nothing")
open(path, "w").write("\n".join(lines))
print(first + 1)
PY
}

# bounded <seconds> <cmd...>: run cmd under a watchdog so a hung mutant cannot
# park the proof. GNU timeout where present (its child runs in a new group and
# the whole group is signalled); elsewhere set -m gives the job its own group
# for the same effect.
bounded() {
  local limit=$1; shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "$limit" "$@"
    return
  fi
  set -m
  "$@" &
  local pid=$!
  ( sleep "$limit"; kill -KILL "-$pid" 2>/dev/null ) &
  local dog=$!
  wait "$pid" 2>/dev/null
  local rc=$?
  kill "$dog" 2>/dev/null; wait "$dog" 2>/dev/null
  set +m
  return "$rc"
}

# Baseline: every test the table names must pass on the unmutated tree, or a
# later failure is a test that was already red - not the mutation's work.
for package in $(printf '%s\n' "${mutations[@]}" | cut -d'|' -f4 | sort -u); do
  echo "baseline: $package tests on the unmutated tree"
  if ! (cd "$work" && bounded 1200 cargo test --locked -q -p "$package" --tests); then
    echo "error: baseline failed - the unmutated suite is already red" >&2
    exit 1
  fi
done

failures=0
ran=0
results=()
for row in "${mutations[@]}"; do
  IFS='|' read -r name file subst package target test <<<"$row"
  [ -n "${PROVE_ONLY:-}" ] && [ "$PROVE_ONLY" != "$name" ] && continue
  ran=$((ran + 1))
  pattern="${subst%%$'\t'*}"
  replacement="${subst#*$'\t'}"
  if [ "$target" = lib ]; then selector=(--lib); else selector=(--test "$target"); fi
  # Restore from the pristine copy of the commit — never the live tree, which a
  # mid-run edit could have changed since the baseline.
  cp "$pristine/$file" "$work/$file"
  mutated_line="$(mutate "$file" "$name" "$pattern" "$replacement")"
  # The kill must land on the protection's own decisive evidence: a panic
  # mutant's mutated line, or the named test file for an assertion mutant —
  # never a boot failure or a panic in a shared helper.
  case "$replacement" in
    *.unwrap\(\)*|*panic!*) pin="$(basename -- "$file"):$mutated_line:" ;;
    *) pin="" ;;
  esac
  started=$(date +%s)
  set +e
  output="$(cd "$work" && bounded 900 cargo test --locked -q -p "$package" "${selector[@]}" -- --exact "$test" 2>&1)"
  status=$?
  set -e
  seconds=$(( $(date +%s) - started ))
  named=0
  kills=0
  if [ "$status" -ne 0 ] && grep -q 'test result: FAILED' <<<"$output" \
      && grep -qE "${test} [-.]+ FAILED" <<<"$output"; then
    named=1
    if [ -n "$pin" ]; then
      grep -qF "panicked at ${pin}" <<<"$output" \
        || grep -qE "panicked at .*tests/${target}\\.rs:" <<<"$output" && kills=1
    else
      grep -qE "panicked at .*tests/${target}\\.rs:" <<<"$output" && kills=1
    fi
  fi
  cp "$pristine/$file" "$work/$file"
  if [ "$kills" -eq 1 ]; then
    # A flake during the mutant run is not a kill: the same test must pass
    # again on the restored file.
    set +e
    green="$(cd "$work" && bounded 900 cargo test --locked -q -p "$package" "${selector[@]}" -- --exact "$test" 2>&1)"
    green_status=$?
    set -e
    if [ "$green_status" -eq 0 ]; then
      verdict=proven
      printf '  proven   %-24s %s fails without it (%ss)\n' "$name" "$test" "$seconds"
    else
      verdict=flaky
      failures=$((failures + 1))
      printf '  FLAKY    %-24s stayed red on the restored tree - the kill may have been a flake:\n%s\n' "$name" "$(tail -8 <<<"$green")"
    fi
  else
    diff "$pristine/$file" "$work/$file" | head -20 >&2 || true
    if [ "$named" -eq 1 ]; then
      verdict=inconclusive
      failures=$((failures + 1))
      printf '  INCONCLUSIVE %-22s failed, but not on the decisive line:\n%s\n' "$name" "$(tail -12 <<<"$output")"
    elif [ "$status" -ne 0 ]; then
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
