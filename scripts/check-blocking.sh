#!/usr/bin/env bash
# NFR-001: reject blocking APIs in code that can run on the GPUI thread.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scopes=(
  "$repo_root/crates/dockering/src"
  "$repo_root/crates/dk-terminal/src/view"
)
pattern='std::(fs|process|net)|block_on|thread::sleep'
failed=0

for scope in "${scopes[@]}"; do
  [[ -d "$scope" ]] || continue
  while IFS= read -r match; do
    # A narrowly reviewed startup-only exception must explain itself on the same line.
    if [[ "$match" =~ //[[:space:]]*nfr-001-allow:[[:space:]]*[^[:space:]].*$ ]]; then
      continue
    fi
    printf '%s\n' "$match" >&2
    failed=1
  done < <(grep -RInE --include='*.rs' "$pattern" "$scope" || true)
done

if (( failed )); then
  cat >&2 <<'MSG'
NFR-001 check failed: blocking API found in a UI-thread source scope.
Move the work behind HubHandle/background_spawn. Pre-window startup code may use
"// nfr-001-allow: <reason>" on the same line after review.
MSG
  exit 1
fi

echo 'NFR-001 blocking-call check passed.'
