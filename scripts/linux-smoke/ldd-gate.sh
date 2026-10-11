#!/usr/bin/env bash
# Fail when a binary links a library that is not installed: the package and its runtime libraries must be enough (REL-072).
#   ldd-gate.sh <binary>      prints the unresolved libraries; exit 1 when there are any, 2 on bad use
set -u
[ $# -eq 1 ] || { echo "usage: ldd-gate.sh <binary>" >&2; exit 2; }
[ -x "$1" ] || { echo "ldd-gate.sh: no executable at $1" >&2; exit 2; }
unresolved=$(ldd "$1" 2>&1 | grep 'not found')
[ -z "$unresolved" ] || { printf '%s\n' "$unresolved"; exit 1; }
