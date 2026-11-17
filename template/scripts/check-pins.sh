#!/usr/bin/env bash
# Supply chain: every container image in deploy/ and the workflows is pinned by digest, and
# every third-party GitHub Action by a full commit SHA. Known advisories are cargo-deny's job
# (deny.toml).
#
# Usage: scripts/check-pins.sh [file ...]   (default: deploy/*.yaml .github/workflows/*.yml)
# Passing files is how scripts/check.sh proves the check can fail.
set -euo pipefail
cd "$(dirname "$0")/.."

if (( $# )); then files=("$@"); else files=(deploy/*.yaml .github/workflows/*.yml); fi
fail=0
for file in "${files[@]}"; do
  while IFS= read -r line; do
    echo "error: $file: image not pinned by digest: $line" >&2
    fail=1
  done < <(grep -nE '^\s*(-\s*)?image:' "$file" | grep -vE '@sha256:[0-9a-f]{64}' || true)
  while IFS= read -r line; do
    echo "error: $file: action not pinned to a commit SHA: $line" >&2
    fail=1
  done < <(grep -nE '^\s*(-\s*)?uses:' "$file" | grep -vE '@[0-9a-f]{40}(\s|$)' || true)
done
if (( fail )); then exit 1; fi
echo "pins ok: every image pinned by digest and every action by commit SHA in ${#files[@]} files"
