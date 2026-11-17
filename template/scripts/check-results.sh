#!/usr/bin/env bash
# Evidence files never name the machine or a person: no hostname, no home directory.
# Results record the CPU, cores, memory, OS, commit and load average, and nothing that
# identifies the host (Durable Runner's 2026-11-17 finding).
#
# Usage: scripts/check-results.sh [file ...]   (default: every file under perf/results)
set -euo pipefail
cd "$(dirname "$0")/.."

if (( $# )); then files=("$@"); else
  files=()
  while IFS= read -r f; do files+=("$f"); done < <(find perf/results -type f ! -name .gitkeep 2>/dev/null | sort)
fi
patterns=('/Users/' '/home/' "$(hostname -s 2>/dev/null || echo .no-host.)" "$(hostname 2>/dev/null || echo .no-host.)")
[ -n "${USER:-}" ] && patterns+=("$USER")
fail=0
for file in ${files[@]+"${files[@]}"}; do
  for pattern in "${patterns[@]}"; do
    [ -n "$pattern" ] || continue
    if grep -qF -- "$pattern" "$file"; then
      echo "error: $file names the machine or a person (matched a hostname, user or home path)" >&2
      fail=1
      break
    fi
  done
done
if (( fail )); then exit 1; fi
echo "results ok: ${#files[@]} evidence files name no host, user or home path"
