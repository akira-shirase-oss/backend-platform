#!/usr/bin/env bash
# Port band: every port this repository binds on a shared machine stays inside its band,
# so a second checkout or another project never collides with it. The band comes from
# PORT_BAND (default 61400-61499, this repository's own band; a copy sets its own —
# docs/COPYING.md). Linux's ephemeral range ends at 60999; pick a band above it.
# Scanned: deploy/, the workflows, the shell scripts and the k6 scenarios — a port
# mapping, a loopback address with a literal port, a *PORT variable's default, or a
# port= assignment outside the band fails. Rust sources are not scanned: tests bind
# 127.0.0.1:0 (ephemeral) and name deliberately unreachable ports.
#
# Usage: PORT_BAND=<low>-<high> scripts/check-ports.sh [file ...]   (default: the scanned set)
# Passing files is how scripts/check.sh proves the check can fail.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
case "${1:-}" in -*) echo "check-ports.sh: unknown argument $1 (see --help)" >&2; exit 2 ;; esac
cd "$(dirname "$0")/.."

band="${PORT_BAND:-$(cat scripts/port-band)}"
[[ "$band" =~ ^[0-9]{2,5}-[0-9]{2,5}$ ]] || { echo "check-ports.sh: PORT_BAND must be <low>-<high>, got '$band'" >&2; exit 2; }
band_low="${band%%-*}" band_high="${band##*-}"
# A band must be non-empty and above Linux's ephemeral range (which ends at 60999);
# 10# keeps a leading zero from being read as octal.
if (( 10#$band_low > 10#$band_high )); then
  echo "check-ports.sh: PORT_BAND low $band_low is above high $band_high" >&2; exit 2
fi
if (( 10#$band_low <= 60999 )); then
  echo "check-ports.sh: PORT_BAND must start above the ephemeral range (60999)" >&2; exit 2
fi

if (( $# )); then
  files=("$@")
else
  shopt -s nullglob
  files=(deploy/* .github/workflows/*.yml scripts/*.sh scripts/drills/*.sh perf/scenarios/*.js)
fi

in_band() { [ "$1" -ge "$band_low" ] && [ "$1" -le "$band_high" ]; }
fail=0
report() {
  echo "error: $file:$1: port $2 outside the port band $band: $3" >&2
  fail=1
}

scanned=0
for file in "${files[@]}"; do
  if [ ! -f "$file" ]; then
    if (( $# )); then
      echo "error: check-ports.sh: no such file: $file" >&2
      exit 2
    fi
    continue
  fi
  scanned=$((scanned + 1))

  # HOST:NNNN literals (two digits or more; :0 is ephemeral) — loopback, a wildcard
  # bind, and the host side of a three-part mapping like 0.0.0.0:NNNN:MMMM.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit##*:}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEo '(127\.0\.0\.1|localhost|0\.0\.0\.0|\[::1?\]):[0-9]{2,5}' "$file" || true)

  # A port mapping: - "NNNN:MMMM" — the host side. No end anchor, so a trailing comment
  # or a flow-sequence comma can't hide it; a host-prefixed three-part mapping is caught
  # by the literal rule above.
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    port="$(sed -n "${line}p" "$file" | sed -nE 's/^[[:space:]]*-[[:space:]]*"?([0-9]{2,5}):.*/\1/p')"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nE '^[[:space:]]*-[[:space:]]*"?[0-9]{2,5}:[0-9]+' "$file" | cut -d: -f1 || true)

  # An unquoted mapping in flow form: `ports: [NNNN:MMMM, ...]`.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    line="${hit%%:*}" port="${hit#*:[}"
    port="${port%%:*}"
    in_band "$port" || report "$line" "$port" "$(sed -n "${line}p" "$file")"
  done < <(grep -nEo '\[[0-9]{2,5}:[0-9]+\]' "$file" || true)

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
echo "ports ok: every literal port is inside the band $band in $scanned files"
