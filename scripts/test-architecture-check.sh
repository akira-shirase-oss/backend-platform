#!/usr/bin/env bash
# Proves scripts/check-architecture.sh catches the provider adapters linked into
# platform-server in every form a manifest allows (F3): a plain dependency, an optional one
# behind a feature, a target-specific one, and a dev-dependency. Each variant patches a
# throwaway copy of the workspace; the check must fail on it, naming platform-adapters.
set -euo pipefail
cd "$(dirname "$0")/.."

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
dep='path = "../platform-adapters"'
status=0
for variant in plain optional target dev; do
  ws="$work/$variant"
  mkdir -p "$ws"
  tar --exclude ./target --exclude ./.git -cf - . | tar -xf - -C "$ws"
  manifest="$ws/crates/platform-server/Cargo.toml"
  case "$variant" in
    plain) printf '\n[dependencies.platform-adapters]\n%s\n' "$dep" >>"$manifest" ;;
    optional) printf '\n[dependencies.platform-adapters]\n%s\noptional = true\n\n[features]\ncloud = ["dep:platform-adapters"]\n' "$dep" >>"$manifest" ;;
    target) printf "\n[target.'cfg(windows)'.dependencies.platform-adapters]\n%s\n" "$dep" >>"$manifest" ;;
    dev) printf '\n[dev-dependencies.platform-adapters]\n%s\n' "$dep" >>"$manifest" ;;
  esac
  # The planted edge is a path dep whose package is already in the committed lockfile:
  # record the edge in the copy's lockfile instead of re-resolving. A fresh resolution
  # fetches live, so an unavailable version breaks main on a scheduled run with no code
  # change (wasm-bindgen-futures v0.4.80, run 44763676119, 2026-11-19).
  python3 - "$ws/Cargo.lock" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
m = re.search(r'name = "platform-server"\nversion = "[^"]+"\ndependencies = \[\n(.*?)\]', s, re.S)
assert m, "platform-server block not found in the copy's Cargo.lock"
s = s[:m.start(1)] + m.group(1).replace(
    ' "platform-db",', ' "platform-adapters",\n "platform-db",', 1) + s[m.end(1):]
open(p, 'w').write(s)
PY
  # cargo tree reads dependency manifests, so the locked set must be fetched first —
  # but only for the real build targets: `all` would request reqwest's wasm-only deps
  # (the unavailable file that broke main). --locked pins exactly the committed set —
  # never a live re-resolution.
  (cd "$ws" && cargo fetch --locked --quiet \
    --target aarch64-apple-darwin \
    --target x86_64-unknown-linux-gnu \
    --target x86_64-pc-windows-msvc)
  # --locked --offline with CARGO_NET_OFFLINE: the lock stays valid for the patched
  # manifest, no package is ever resolved or fetched, and a network request — or a lock
  # that no longer matches — can only fail for the planted reason, never pass.
  if out="$(CARGO_NET_OFFLINE=1 ARCH_ROOT="$ws" ARCH_CARGO_MODE="--locked --offline" scripts/check-architecture.sh 2>&1)"; then
    echo "error: the architecture check missed a $variant adapter dependency" >&2
    status=1
  elif ! grep -q 'platform-server must not depend on platform-adapters' <<<"$out"; then
    echo "error: the $variant variant failed for another reason: $out" >&2
    status=1
  else
    echo "caught: $variant adapter dependency (no network touched)"
  fi
done
exit "$status"
