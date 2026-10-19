#!/usr/bin/env bash
# REL-084 / REL-086: evidence that every release binary matches the update channel.
#
#   release-evidence.sh scan <binary> <evidence-file>
#       Scan one UNPACKAGED release binary (target/<triple>/release/dockering[.exe]) and write
#       two lines to <evidence-file>:
#         updater=<true|false>   the binary contains the updater marker (the manifest URL)
#         seam=<true|false>      the binary contains the test-seam marker (REL-086)
#
#   release-evidence.sh check <artifacts-dir> <none|all>
#       Used by `assemble`. Needs <artifacts-dir>/release-package-<target>/updater-evidence-<target>.txt
#       for all six targets, every `updater=` equal to the channel (all -> true, none -> false)
#       and every `seam=false`. Reports every problem, then fails.
#
# Plain `grep -aqF` under LC_ALL=C: the same bytes-only scan on GNU grep (Linux, Git for Windows)
# and BSD grep (macOS), and no Python or Rust toolchain needed. Works with Bash 3.2.
set -euo pipefail

UPDATER_MARKER='https://github.com/pavel-purma/dockering/releases/latest/download/dockering-update.json'
SEAM_MARKER='dockering-update-e2e-seam-v1'
TARGETS=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-pc-windows-msvc
  aarch64-pc-windows-msvc
  aarch64-apple-darwin
  x86_64-apple-darwin
)

failures=0

usage() {
  echo "usage: release-evidence.sh scan <binary> <evidence-file>" >&2
  echo "       release-evidence.sh check <artifacts-dir> <none|all>" >&2
  exit 64
}

problem() {
  echo "::error::$1"
  failures=$((failures + 1))
}

# Prints true or false. grep exits 0 (found), 1 (not found) or 2 (error): an error must not
# be mistaken for "not found", or a broken scan could look like a clean updater-less build.
contains() {
  local status=0
  LC_ALL=C grep -aqF -e "$2" "$1" || status=$?
  case "$status" in
    0) echo true ;;
    1) echo false ;;
    *)
      echo "::error::could not scan $1 (grep exit status $status)" >&2
      exit 2
      ;;
  esac
}

scan() {
  local binary=$1 out=$2 updater seam
  if [[ ! -f "$binary" || ! -s "$binary" ]]; then
    echo "::error::release binary $binary is missing or empty"
    exit 1
  fi
  updater=$(contains "$binary" "$UPDATER_MARKER")
  seam=$(contains "$binary" "$SEAM_MARKER")
  mkdir -p "$(dirname "$out")"
  printf 'updater=%s\nseam=%s\n' "$updater" "$seam" > "$out"
  echo "release-evidence: $binary: updater=$updater seam=$seam -> $out"
}

check_one() {
  local dir=$1 expect=$2 channel=$3 target=$4
  local file="$dir/release-package-$target/updater-evidence-$target.txt"
  local line count=0 updater='' seam='' malformed=false
  if [[ ! -f "$file" ]]; then
    problem "missing updater evidence for $target ($file)"
    return 0
  fi
  # Exactly `updater=<bool>` then `seam=<bool>`; a CR from a Windows runner is tolerated.
  while IFS= read -r line || [[ -n "$line" ]]; do
    line=${line%$'\r'}
    count=$((count + 1))
    case "$count:$line" in
      1:updater=true | 1:updater=false) updater=${line#updater=} ;;
      2:seam=true | 2:seam=false) seam=${line#seam=} ;;
      *) malformed=true ;;
    esac
  done < "$file"
  if [[ "$malformed" == true || "$count" -ne 2 ]]; then
    problem "malformed updater evidence for $target ($file): expected the lines updater=<true|false> and seam=<true|false>"
    return 0
  fi
  if [[ "$updater" != "$expect" ]]; then
    problem "$target: the binary has updater=$updater but the release channel is updater=$channel (REL-084)"
  fi
  if [[ "$seam" != false ]]; then
    problem "$target: the release binary contains the update-e2e test seam (REL-086)"
  fi
}

check() {
  local dir=$1 channel=$2 expect target
  case "$channel" in
    all) expect=true ;;
    none) expect=false ;;
    *)
      echo "::error::the updater channel must be none or all, got '$channel'"
      exit 1
      ;;
  esac
  for target in "${TARGETS[@]}"; do
    check_one "$dir" "$expect" "$channel" "$target"
  done
  if [[ "$failures" -ne 0 ]]; then
    exit 1
  fi
  echo "release-evidence: all ${#TARGETS[@]} targets agree with updater=$channel and none contains the test seam"
}

case "${1:-}" in
  scan)
    [[ $# -eq 3 ]] || usage
    scan "$2" "$3"
    ;;
  check)
    [[ $# -eq 3 ]] || usage
    check "$2" "$3"
    ;;
  *) usage ;;
esac
