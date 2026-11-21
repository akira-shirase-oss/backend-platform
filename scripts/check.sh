#!/usr/bin/env bash
# The gate: what CI runs, run before every commit. Toolchain pin, formatting, Clippy with
# warnings denied (fault-injection build included), the architecture rules and proof that
# each can fail, every test against PostgreSQL (this starts the local stack when
# BP_TEST_DATABASE_URL is unset), the partial-provisioning drill (10,000 in-process
# schedules and the process-level kill -9 cases), a short load probe, cargo-deny, then the
# starter template's own gate (template/scripts/check.sh).
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/check-toolchain.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p platform-worker --features fault-injection --all-targets --locked -- -D warnings

# cargo tree reads dependency manifests: fetch the locked set for the real build targets
# (the architecture check no longer asks for `all`, so wasm-only deps are never needed).
cargo fetch --locked --quiet \
  --target aarch64-apple-darwin \
  --target x86_64-unknown-linux-gnu \
  --target x86_64-pc-windows-msvc
scripts/check-architecture.sh
# Each rule must be able to fail.
expect_failure() {
  if env "$@" scripts/check-architecture.sh >/dev/null 2>&1; then
    echo "error: the architecture check did not catch: $*" >&2
    exit 1
  fi
}
expect_failure ARCH_DOMAIN_BANNED=thiserror
expect_failure ARCH_DOMAIN_PATTERN='pub fn'
if cargo metadata --locked --format-version 1 --no-deps | grep -q '"name":"platform-server"'; then
  expect_failure ARCH_SERVER_BANNED=platform-db
fi
# ...including when an adapter is plain, optional behind a feature, target-specific or a
# dev-dependency of platform-server (F3).
scripts/test-architecture-check.sh
echo "architecture check proven able to fail"

scripts/check-pins.sh
# The pin check must be able to fail too: on an image and on an action, each alone (as
# CI), so a checker that ignored either one is caught.
unpinned="$(mktemp)"
for sample in 'services:\n  db:\n    image: postgres:18\n' 'steps:\n  - uses: actions/checkout@v7\n'; do
  printf '%b' "$sample" >"$unpinned"
  status=0
  scripts/check-pins.sh "$unpinned" >/dev/null 2>&1 || status=$?
  if [ "$status" -ne 1 ]; then
    echo "error: the pin check did not catch: $sample (exit $status)" >&2
    rm -f "$unpinned"
    exit 1
  fi
done
rm -f "$unpinned"
echo "pin check proven able to fail"

scripts/check-ports.sh
# The port check must be able to fail too: a mapped host port and a loopback literal,
# each alone outside the band (as CI proves for pins).
# The out-of-band values reach the sample file through printf %s, so this script itself
# holds no offending literal for the check to flag.
outofband="$(mktemp)"
for spec in 'ports:\n      - "%s:8000"\n|58000' 'curl http://127.0.0.1:%s/health\n|8080'; do
  printf "${spec%|*}" "${spec#*|}" >"$outofband"
  status=0
  scripts/check-ports.sh "$outofband" >/dev/null 2>&1 || status=$?
  if [ "$status" -ne 1 ]; then
    echo "error: the port check did not catch: $spec (exit $status)" >&2
    rm -f "$outofband"
    exit 1
  fi
done
rm -f "$outofband"
echo "port check proven able to fail"

scripts/check-workflows.sh
dup="$(mktemp)"
printf 'jobs:\n  build:\n    steps: []\n  build:\n    steps: []\n' >"$dup"
status=0
scripts/check-workflows.sh "$dup" >/dev/null 2>&1 || status=$?
if [ "$status" -ne 1 ]; then
  echo "error: the workflow check passed a duplicate key (exit $status)" >&2
  rm -f "$dup"
  exit 1
fi
rm -f "$dup"
echo "workflow check proven able to fail"

if [ -z "${BP_TEST_DATABASE_URL:-}" ]; then
  scripts/stack.sh up >/dev/null
  eval "$(scripts/stack.sh env)"
fi
export BP_TEST_DATABASE_URL
cargo test --workspace --locked
# Drill partial-provision, process level: the real worker binary killed with SIGKILL.
cargo test -p platform-worker --features fault-injection --test kill_drill --locked
# Every parser of outside input: seed corpus replay + a bounded proptest smoke.
scripts/fuzz-smoke.sh --cases "${PROPTEST_CASES:-512}"

# The control-API load scenario, as a short probe (never committed as evidence), run on
# non-default band ports so it also proves BP_SERVER_PORT/BP_FAKE_PORT parameterise a run.
# BP_SKIP_LOAD_PROBE=1 skips it while another load run owns the machine; CI never sets it.
if [ "${BP_SKIP_LOAD_PROBE:-0}" = 1 ]; then
  echo "note: BP_SKIP_LOAD_PROBE=1; load probe skipped locally (CI runs it)"
elif command -v k6 >/dev/null 2>&1; then
  TOTAL=2000 RATE=200 PROJECTS=100 BP_SERVER_PORT=61470 BP_FAKE_PORT=61471 scripts/load.sh >/dev/null
  echo "load probe ok"
else
  echo "note: k6 not installed (scripts/install-k6.sh); load probe skipped locally (CI runs it)"
fi

# cargo installs tools into ~/.cargo/bin, which may be missing from PATH (after rustup's).
[ -d "$HOME/.cargo/bin" ] && PATH="$PATH:$HOME/.cargo/bin"
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny --locked check
else
  echo "note: cargo-deny not installed; supply-chain check skipped locally (CI runs it)"
fi
# B1-T's evidence under perf/results names no host, user or home path (the template's check).
shopt -s nullglob
b1t=(perf/results/*-b1t-*.json)
if (( ${#b1t[@]} )); then template/scripts/check-results.sh "${b1t[@]}"; fi
shopt -u nullglob
# The starter template (B1-T): its own workspace and gate, run on this stack's PostgreSQL
# (its roles are starter_*, so it never touches this repository's databases). CI runs it as
# its own job on the template's own stack.
STARTER_TEST_DATABASE_URL="$BP_TEST_DATABASE_URL" \
  STARTER_PG_CONTAINER="$(docker compose -f deploy/compose.yaml ps -q postgres 2>/dev/null || true)" \
  template/scripts/check.sh
# A template copy must be fmt- and clippy-clean at every legal NAME length — the rename
# changes line widths, so a template edit stable only at "starter" length fails here.
scripts/test-copy-names.sh
echo "check ok"
