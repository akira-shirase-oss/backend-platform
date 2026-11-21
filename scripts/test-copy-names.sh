#!/usr/bin/env bash
# The rename is not text-neutral for rustfmt and Clippy: crate-name lengths change line
# widths, and a name-dependent wrap can produce a shape clippy rejects (seen:
# semicolon_if_nothing_returned when a 4-letter name let fmt collapse a match arm into a
# block that needed a ';'). This test copies the template under a 2-letter and a
# 16-letter name — the legal NAME extremes — and runs fmt --check and clippy
# -D warnings on each copy, so a template edit that is only stable at "starter"
# length fails here first.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
[ $# -eq 0 ] || { echo "test-copy-names.sh: takes no arguments (see --help)" >&2; exit 2; }
cd "$(dirname "$0")/.."

work=target/test-copy-names
rm -rf "$work"
mkdir -p "$work"
# The copies are throwaway: reserve their bands in a scratch registry, not the real one.
printf 'backend-platform 61400-61499\n' >"$work/port-bands"
export PORT_BANDS_FILE="$PWD/$work/port-bands"

band=62000
for name in ab abcdefghijklmnop; do
  dest="$PWD/$work/$name"
  scripts/copy-template.sh "$dest" "$name" "$band-$((band + 99))" >/dev/null
  band=$((band + 100))
  (
    cd "$dest"
    unset CARGO_TARGET_DIR
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -q -- -D warnings
  )
  # The same leftover grep --verify runs: a rename rule that misses a template name
  # (seen: starter-attacks) ships a copy that still links the template's crates.
  # -a and LC_ALL=C: GNU grep skips lines it cannot decode, which is exactly
  # where a non-UTF-8 leftover (a fuzz corpus seed) would hide.
  left="$(LC_ALL=C grep -rIna --exclude-dir=target -E '(^|[^A-Za-z0-9])starter[-_]|(^|[^A-Z0-9])STARTER_|starterctl|(id|nologin|--service|is-active|-u|--value|stop|enable) starter\b' "$dest" | grep -v TEMPLATE_ORIGIN || true)"
  if [ -n "$left" ]; then
    echo "error: names the rename missed:" >&2
    echo "$left" >&2
    exit 1
  fi
  echo "name ok at length ${#name}: fmt --check, clippy and the leftover grep clean ($name)"
done

# Every reservation the registry promises must actually refuse — each fixture
# below must fail the copy, not slip through and ship a taken band.
grep -q '^ab 62000-62099$' "$PORT_BANDS_FILE" || {
  echo "error: the successful copy did not record its band in the registry" >&2
  exit 1
}
refused() { # label dest name band
  local label=$1 dest=$2 name=$3 band=$4
  if scripts/copy-template.sh "$dest" "$name" "$band" >"$work/out" 2>&1; then
    echo "error: copy-template did not refuse $label ($name $band):" >&2
    cat "$work/out" >&2
    exit 1
  fi
  echo "refused: $label"
}
printf 'zz 61700-61799\n' >>"$PORT_BANDS_FILE"
refused "an overlapping band" "$work/o1" yy 61750-61850
refused "a duplicate product name" "$work/o2" zz 61900-61999
refused "the platform's own band" "$work/o3" yy 61400-61499
refused "a band at the ephemeral boundary" "$work/o4" yy 60900-60999
refused "a band above 65535" "$work/o5" yy 65500-65600
printf 'crlf 61500-61599\r\n' >>"$PORT_BANDS_FILE"
refused "a CRLF registry line" "$work/o6" yy 61600-61699
printf 'backend-platform 61400-61499\ndash-owner 61500\342\200\22361599\n' >"$PORT_BANDS_FILE"
refused "an en dash registry line" "$work/o7" yy 61600-61699
rm -rf "$work"
echo "copy-names ok: the template is fmt- and clippy-clean at NAME lengths 2 and 16"
