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

for name in ab abcdefghijklmnop; do
  dest="$PWD/$work/$name"
  scripts/copy-template.sh "$dest" "$name" >/dev/null
  (
    cd "$dest"
    unset CARGO_TARGET_DIR
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -q -- -D warnings
  )
  echo "name ok at length ${#name}: fmt --check and clippy clean ($name)"
done
rm -rf "$work"
echo "copy-names ok: the template is fmt- and clippy-clean at NAME lengths 2 and 16"
