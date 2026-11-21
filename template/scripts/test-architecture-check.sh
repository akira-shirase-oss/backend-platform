#!/usr/bin/env bash
# Proves scripts/check-architecture.sh catches a database driver linked into starter-http in
# every form a manifest allows: plain, optional behind a feature, and target-specific; and
# test support enabled in the shipped build. Each variant patches a throwaway copy; the
# check must fail on it, for that reason.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
[ $# -eq 0 ] || { echo "test-architecture-check.sh: takes no arguments (see --help)" >&2; exit 2; }
cd "$(dirname "$0")/.."

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
status=0
for variant in plain optional target shipped; do
  ws="$work/$variant"
  mkdir -p "$ws"
  tar --exclude ./target --exclude ./.git -cf - . | tar -xf - -C "$ws"
  case "$variant" in
    plain) printf '\n[dependencies.sqlx]\nworkspace = true\n' >>"$ws/crates/starter-http/Cargo.toml"; expect='must not depend on sqlx' ;;
    optional) printf '\n[dependencies.sqlx]\nworkspace = true\noptional = true\n\n[features]\ndb = ["dep:sqlx"]\n' >>"$ws/crates/starter-http/Cargo.toml"; expect='must not depend on sqlx' ;;
    target) printf "\n[target.'cfg(windows)'.dependencies.sqlx]\nworkspace = true\n" >>"$ws/crates/starter-http/Cargo.toml"; expect='must not depend on sqlx' ;;
    shipped) sed -i.bak 's|^starter-db.workspace = true$|starter-db = { workspace = true, features = ["test-support"] }|' "$ws/crates/starter-server/Cargo.toml"; expect='enables the test-support feature' ;;
  esac
  # The planted edge is already in the committed lockfile's package set — record it in the
  # copy's lockfile instead of re-resolving. A fresh resolution fetches live, so an
  # unavailable version breaks the check on a scheduled run with no code change.
  case "$variant" in
    plain|optional|target)
      python3 - "$ws/Cargo.lock" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
m = re.search(r'name = "starter-http"\nversion = "[^"]+"\ndependencies = \[\n(.*?)\]', s, re.S)
assert m, "starter-http block not found in the copy's Cargo.lock"
block = m.group(1)
assert ' "sqlx",' not in block
s = s[:m.start(1)] + block + ' "sqlx",\n' + s[m.end(1):]
open(p, 'w').write(s)
PY
      ;;
  esac
  # cargo tree reads dependency manifests, so the locked set must be fetched first — but
  # only for the real build targets, so a wasm-only dep that is unavailable can never
  # break the check. --locked pins exactly the committed set — never a live re-resolution.
  (cd "$ws" && cargo fetch --locked --quiet \
    --target aarch64-apple-darwin \
    --target x86_64-unknown-linux-gnu \
    --target x86_64-pc-windows-msvc)
  # --locked --offline with CARGO_NET_OFFLINE: the lock stays valid for the patched
  # manifest, no package is ever resolved or fetched, and a network request — or a lock
  # that no longer matches — can only fail for the planted reason, never pass.
  if out="$(CARGO_NET_OFFLINE=1 ARCH_ROOT="$ws" ARCH_CARGO_MODE="--locked --offline" scripts/check-architecture.sh 2>&1)"; then
    echo "error: the architecture check missed the $variant variant" >&2
    status=1
  elif ! grep -q "$expect" <<<"$out"; then
    echo "error: the $variant variant failed for another reason: $out" >&2
    status=1
  else
    echo "caught: $variant (no network touched)"
  fi
done
exit "$status"
