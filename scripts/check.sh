#!/usr/bin/env bash
# The gate: what CI runs, run before every commit. Toolchain pin, formatting, Clippy with
# warnings denied (fault-injection build included), the architecture rules and proof that
# each can fail, every test against PostgreSQL (this starts the local stack when
# BP_TEST_DATABASE_URL is unset), the partial-provisioning drill (10,000 in-process
# schedules and the process-level kill -9 cases), a short load probe, then cargo-deny.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/check-toolchain.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p platform-worker --features fault-injection --all-targets --locked -- -D warnings

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
# The pin check must be able to fail too.
unpinned="$(mktemp)"
printf 'services:\n  db:\n    image: postgres:18\nsteps:\n  - uses: actions/checkout@v7\n' >"$unpinned"
if scripts/check-pins.sh "$unpinned" >/dev/null 2>&1; then
  echo "error: the pin check did not catch an unpinned image and action" >&2
  rm -f "$unpinned"
  exit 1
fi
rm -f "$unpinned"
echo "pin check proven able to fail"

if [ -z "${BP_TEST_DATABASE_URL:-}" ]; then
  scripts/stack.sh up >/dev/null
  eval "$(scripts/stack.sh env)"
fi
export BP_TEST_DATABASE_URL
cargo test --workspace --locked
# Drill partial-provision, process level: the real worker binary killed with SIGKILL.
cargo test -p platform-worker --features fault-injection --test kill_drill --locked

# The control-API load scenario, as a short probe (never committed as evidence).
if command -v k6 >/dev/null 2>&1; then
  TOTAL=2000 RATE=200 PROJECTS=100 scripts/load.sh >/dev/null
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
echo "check ok"
