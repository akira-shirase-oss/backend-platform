#!/usr/bin/env bash
# The gate: what CI runs, run before every push. Exits non-zero on the first failure; read
# its exit code directly, never through a pipe.
#
#   scripts/check.sh
#
# Toolchain pin, formatting, Clippy with warnings denied, the architecture, pin and results
# checks (each shown able to fail), the host layer's checks, every test against PostgreSQL
# (the local stack starts when STARTER_TEST_DATABASE_URL is unset), then cargo-deny. The limit proofs
# (scripts/prove-limits.sh) run as their own CI job.
#
# Needs: the pinned toolchain through rustup, docker (or STARTER_TEST_DATABASE_URL), and
# cargo-deny (skipped with a note when absent; CI always runs it).
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }

step "toolchain"
scripts/check-toolchain.sh

step "fmt"
cargo fmt --all -- --check

step "clippy (-D warnings)"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "architecture, and that each rule can fail"
scripts/check-architecture.sh
expect_failure() {
  if env "$@" scripts/check-architecture.sh >/dev/null 2>&1; then
    echo "error: the architecture check did not catch: $*" >&2
    exit 1
  fi
}
expect_failure ARCH_HTTP_BANNED=tokio
expect_failure ARCH_DOMAIN_PATTERN='pub fn'
expect_failure ARCH_SHIPPED_FEATURE=json
scripts/test-architecture-check.sh

step "pins, and that the check can fail on an image and on an action, each alone"
scripts/check-pins.sh
sample="$(mktemp)"
for unpinned in 'services:\n  db:\n    image: postgres:18\n' 'steps:\n  - uses: actions/checkout@v7\n'; do
  printf '%b' "$unpinned" >"$sample"
  if scripts/check-pins.sh "$sample" >/dev/null 2>&1; then
    echo "error: the pin check did not catch: $unpinned" >&2
    rm -f "$sample"
    exit 1
  fi
done
rm -f "$sample"

step "results name no host, and the check can fail"
scripts/check-results.sh
printf '{"machine":{"path":"/home/someone/x"}}\n' >"$sample"
if scripts/check-results.sh "$sample" >/dev/null 2>&1; then
  echo "error: the results check passed a home path" >&2
  rm -f "$sample"
  exit 1
fi
rm -f "$sample"

step "host layer: hardening check (proven both ways), Caddyfile, unit exposure"
scripts/test-harden-check.sh
scripts/check-caddy.sh
if command -v systemd-analyze >/dev/null 2>&1; then
  scripts/check-unit.sh
  if scripts/check-unit.sh deploy/starter.service 10 >/dev/null 2>&1; then
    echo "error: the unit check passed below the unit's own exposure" >&2
    exit 1
  fi
else
  echo "note: no systemd-analyze here; the unit's exposure is checked in CI (or: scripts/check-unit.sh, through docker)"
fi

step "tests against PostgreSQL"
if [ -z "${STARTER_TEST_DATABASE_URL:-}" ]; then
  scripts/stack.sh up
  eval "$(scripts/stack.sh env)"
fi
export STARTER_TEST_DATABASE_URL
cargo test --workspace --locked

step "cargo deny (advisories, licenses, bans, sources)"
[ -d "$HOME/.cargo/bin" ] && PATH="$PATH:$HOME/.cargo/bin"
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny --locked check
else
  echo "note: cargo-deny not installed; supply-chain check skipped locally (CI runs it)"
fi

step "check ok"
