#!/usr/bin/env bash
# Port band (deploy/compose.yaml): every port this repository binds on the shared machine
# stays inside 61400-61499, so a second checkout or another project never collides with it.
# Scanned: deploy/, the workflows, the shell scripts and the k6 scenarios — a port mapping,
# a loopback address with a literal port, a *PORT variable's default, or a port= assignment
# outside the band fails. Rust sources are not scanned: tests bind 127.0.0.1:0 (ephemeral)
# and name deliberately unreachable ports.
#
# Usage: scripts/check-ports.sh [file ...]   (default: the scanned set)
# Passing files is how scripts/check.sh proves the check can fail.
set -euo pipefail
cd "$(dirname "$0")/.."

if (( $# )); then
  files=("$@")
else
  shopt -s nullglob
  files=(
    deploy/* .github/workflows/*.yml scripts/*.sh perf/scenarios/*.js
    template/deploy/* template/.github/workflows/*.yml
    template/scripts/*.sh template/scripts/drills/*.sh template/perf/scenarios/*.js
  )
fi

in_band() { [ "$1" -ge 61400 ] && [ "$1" -le 61499 ]; }
fail=0
report() {
  echo "error: $file:$1: port $2 outside the port band 61400-61499: $3" >&2
  fail=1
}

scanned=0
for file in "${files[@]}"; do
  [ -f "$file" ] || continue
  scanned=$((scanned + 1))

  # HOST:NNNN literals (two digits or more; :0 is ephemeral) — loopback, a wildcard
  # bind, and the host side of a three-part mapping like 0.0.0.0:NNNN:MMMM.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit##*:}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEo '(127\.0\.0\.1|localhost|0\.0\.0\.0|\[::\]):[0-9]{2,5}' "$file" || true)

  # A port mapping: - "NNNN:MMMM" — the host side. No end anchor, so a trailing comment
  # or a flow-sequence comma can't hide it; a host-prefixed three-part mapping is caught
  # by the literal rule above.
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    port="$(sed -n "${line}p" "$file" | sed -nE 's/^[[:space:]]*-[[:space:]]*"?([0-9]{2,5}):.*/\1/p')"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nE '^[[:space:]]*-[[:space:]]*"?[0-9]{2,5}:[0-9]+' "$file" | cut -d: -f1 || true)

  # A quoted mapping anywhere — the flow form `ports: ["NNNN:MMMM", ...]`.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit#*:}"
    port="${port#\"}"; port="${port%%:*}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEo '"[0-9]{2,5}:[0-9]+"' "$file" || true)

  # The compose long form: `published: NNNN` is the host port.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit##*:}"
    port="${port##*[!0-9]}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEio 'published:[[:space:]]*"?[0-9]{2,5}' "$file" || true)

  # A *PORT variable's default: ${...PORT...:-NNNN}.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit##*:-}"; port="${port%\}}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEo '\$\{[A-Za-z_][A-Za-z0-9_]*[Pp][Oo][Rr][Tt][A-Za-z0-9_]*:-[0-9]{2,5}\}' "$file" || true)

  # A port assignment: port=NNNN or a name ending _port=NNNN.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit##*=}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -niEo '(^|[[:space:]])(port|[a-z0-9_]+_port)=[0-9]{2,5}' "$file" || true)
done

if (( fail )); then exit 1; fi
# A check that inspects nothing proves nothing — the gate must fail, not pass.
if (( scanned == 0 )); then
  echo "error: check-ports.sh checked no files" >&2
  exit 2
fi
echo "ports ok: every literal port is inside the band 61400-61499 in $scanned files"
