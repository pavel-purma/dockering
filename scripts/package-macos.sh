#!/usr/bin/env bash
# REL-017: retry only create-dmg's known "couldn't eject ... Resource busy" failure.
# Detach only the reported disk, after its image path is verified under this target's dist dir.
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^(x86_64|aarch64)-apple-darwin$ ]]; then
  echo 'usage: bash scripts/package-macos.sh <x86_64|aarch64-apple-darwin>' >&2
  exit 2
fi
target=$1
log=$(mktemp "${TMPDIR:-/tmp}/dockering-package.XXXXXX")
mounts=$(mktemp "${TMPDIR:-/tmp}/dockering-mounts.XXXXXX")
trap 'rm -f "$log" "$mounts"' EXIT

for attempt in 1 2 3; do
  set +e
  cargo xtask package --target "$target" 2>&1 | tee "$log"
  result=${PIPESTATUS[0]}
  set -e
  if [[ "$result" -eq 0 ]]; then exit 0; fi
  if [[ "$attempt" -eq 3 ]] || ! grep -Eq "hdiutil: couldn't eject \"disk[0-9]+\" - Resource busy" "$log"; then
    exit "$result"
  fi
  # If inspection, path verification, or cleanup cannot prove safety, preserve the cargo failure.
  if ! hdiutil info -plist > "$mounts"; then exit "$result"; fi
  if ! devices=$(python3 - "target/dist/$target" "$mounts" "$log" <<'PY'
import os
import plistlib
import re
import sys

root = os.path.realpath(sys.argv[1])
with open(sys.argv[2], 'rb') as f:
    images = plistlib.load(f).get('images', [])
with open(sys.argv[3]) as f:
    failed = set(re.findall(r'hdiutil: couldn\'t eject "(disk[0-9]+)" - Resource busy', f.read()))
verified = set()
for image in images:
    path = image.get('image-path')
    if not isinstance(path, str):
        continue
    path = os.path.realpath(path)
    if os.path.commonpath([root, path]) != root or path == root:
        continue
    for entity in image.get('system-entities', []):
        match = re.fullmatch(r'/dev/(disk[0-9]+)(?:s[0-9]+)*', entity.get('dev-entry', ''))
        if match and match[1] in failed:
            verified.add('/dev/' + match[1])
if not verified:
    raise SystemExit('No reported busy DMG disk has an image inside the target distribution directory; refusing cleanup')
print('\n'.join(sorted(verified)))
PY
  ); then
    exit "$result"
  fi
  while IFS= read -r device; do
    echo "Detaching verified busy release image $device before package retry ($attempt/3)"
    if ! hdiutil detach "$device" -force; then exit "$result"; fi
  done <<< "$devices"
done
