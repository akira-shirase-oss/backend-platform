#!/usr/bin/env bash
# The architecture rules (AGENTS.md non-negotiables 5 and the Rust rules):
#
#  1. platform-domain carries no I/O: no I/O crate anywhere in its dependency tree
#     (transitively), and its source names none of std's file, network, process, thread,
#     environment or clock APIs.
#  2. platform-server never links the provider adapters or the fake provider (D-013, T1),
#     through any normal, build or dev dependency, under any feature or target
#     (scripts/test-architecture-check.sh proves each form is caught).
#  3. platform-server reads no provider setting: its source never mentions BP_PROVIDER.
#
# Overrides exist only to prove each rule can fail (scripts/check.sh does that):
#   ARCH_DOMAIN_BANNED="a b"     crates banned from the domain's tree
#   ARCH_DOMAIN_PATTERN='regex'  source pattern banned in the domain
#   ARCH_SERVER_BANNED="a b"     crates banned from the server's tree
set -euo pipefail
cd "${ARCH_ROOT:-$(dirname "$0")/..}"
mode="${ARCH_CARGO_MODE:---locked}"

domain_banned="${ARCH_DOMAIN_BANNED:-tokio tokio-util axum hyper tower tower-http sqlx sqlx-core reqwest mio socket2 libc nix}"
domain_pattern="${ARCH_DOMAIN_PATTERN:-std::(fs|net|process|env|thread)|SystemTime|Instant::now|stdin\(|stdout\(|println!|eprintln!}"
server_banned="${ARCH_SERVER_BANNED:-platform-adapters platform-fake-provider platform-worker reqwest}"

# The resolved graph with every feature and every real build target (F3): an adapter
# hidden behind `optional = true` + a feature, or under `[target.'cfg(..)'.dependencies]`,
# is still linked in some build, so it counts. The targets are the three real ones —
# `all` would also demand the manifests of wasm-only deps (e.g. reqwest's
# wasm-bindgen-futures), which are never fetched and can be unavailable; a dep planted
# behind another cfg triple (cfg(unix), cfg(wasm32)) is not covered.
tree_of() {
  for t in aarch64-apple-darwin x86_64-unknown-linux-gnu x86_64-pc-windows-msvc; do
    cargo tree $mode -p "$1" -e "$2" --all-features --target "$t" --prefix none --format '{p}'
  done | awk '{print $1}' | sort -u
}

fail=0
tree=$(tree_of platform-domain normal,build)
for name in $domain_banned; do
  if grep -qx -- "$name" <<<"$tree"; then
    echo "error: platform-domain must not depend on $name" >&2
    fail=1
  fi
done
if grep -rnE -- "$domain_pattern" crates/platform-domain/src >/dev/null; then
  echo "error: platform-domain source uses an I/O or clock API:" >&2
  grep -rnE -- "$domain_pattern" crates/platform-domain/src >&2 || true
  fail=1
fi

if cargo metadata $mode --format-version 1 --no-deps | grep -q '"name":"platform-server"'; then
  server_tree=$(tree_of platform-server normal,build,dev)
  for name in $server_banned; do
    if grep -qx -- "$name" <<<"$server_tree"; then
      echo "error: platform-server must not depend on $name (D-013: provider credentials live only in the worker)" >&2
      fail=1
    fi
  done
  if grep -rn 'BP_PROVIDER' crates/platform-server >/dev/null; then
    echo "error: platform-server reads a provider setting (T1):" >&2
    grep -rn 'BP_PROVIDER' crates/platform-server >&2 || true
    fail=1
  fi
  server_note="platform-server links none of [$server_banned] and reads no BP_PROVIDER setting"
else
  server_note="platform-server not present yet"
fi

if (( fail )); then
  exit 1
fi
echo "architecture ok: platform-domain has none of [$domain_banned] ($(wc -l <<<"$tree" | tr -d ' ') crates) and no I/O in its source; $server_note"
