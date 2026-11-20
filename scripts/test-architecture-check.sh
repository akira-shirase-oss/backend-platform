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
  # The planted edge re-resolves the adapters' subtree; --target all can pick versions the
  # committed lock never needed (e.g. reqwest's wasm deps), which the offline cache lacks.
  # Fetch what the patched manifest resolves to, so the check's own --offline run fails
  # only for the planted reason — "attempting to make an HTTP request" can never pass.
  (cd "$ws" && cargo fetch --quiet)
  # --offline, not --locked: the patched manifest changes the lockfile, which is the point.
  if out="$(ARCH_ROOT="$ws" ARCH_CARGO_MODE=--offline scripts/check-architecture.sh 2>&1)"; then
    echo "error: the architecture check missed a $variant adapter dependency" >&2
    status=1
  elif ! grep -q 'platform-server must not depend on platform-adapters' <<<"$out"; then
    echo "error: the $variant variant failed for another reason: $out" >&2
    status=1
  else
    echo "caught: $variant adapter dependency"
  fi
done
exit "$status"
