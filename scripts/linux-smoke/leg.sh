#!/usr/bin/env bash
# Run one leg of the Linux smoke test: pull a distro image, then install and launch the package inside it.
# Plan: docs/plan/features/linux-install-smoke-test.md (REL-070..077). Used by linux-smoke.yml and by developers.
#   leg.sh <slug> <deb|tar> <pkg-dir> <out-dir> <version> <image> [<image>...]
# pkg-dir holds the packages and packages.sha256 (fetch.sh writes them); out-dir collects the evidence.
# Each image is a fallback for the one before it: the first that pulls is used.
# env:  SMOKE_LIBC_FLOOR  passed to the container (warn: a missing .deb glibc floor is INFO, not FAIL)
#       SMOKE_PULL_DELAY  seconds between pull attempts (default 10)
#       SMOKE_ENTRY       script of this directory that runs inside the container (default entry.sh; selftest.sh
#                         sets selftest-entry.sh)
#       SMOKE_SELFTEST_ONLY  passed to the container: the controls of selftest-entry.sh to run (default all)
# Exits with the container's status.
set -u

usage() { echo "usage: leg.sh <slug> <deb|tar> <pkg-dir> <out-dir> <version> <image> [<image>...]" >&2; exit 2; }
bad() { echo "leg.sh: $*" >&2; exit 2; }
[ $# -ge 6 ] || usage
slug=$1 kind=$2 pkg=$3 out=$4 version=$5
shift 5
slug_re='^[A-Za-z0-9][A-Za-z0-9._-]*$'
version_re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
image_re='^[A-Za-z0-9][A-Za-z0-9._/:@-]*$'
entry_re='^[a-z][a-z0-9-]*\.sh$'
entry=${SMOKE_ENTRY:-entry.sh}
[[ $slug =~ $slug_re ]] || bad "unexpected slug '$slug'"
[ "$kind" = deb ] || [ "$kind" = tar ] || bad "package kind must be deb or tar, not '$kind'"
[[ $version =~ $version_re ]] || bad "unexpected version '$version'"
for image in "$@"; do [[ $image =~ $image_re ]] || bad "unexpected image reference '$image'"; done
[[ $entry =~ $entry_re ]] || bad "unexpected entry script '$entry'"

# Git Bash on Windows must neither rewrite the container side of a mount nor hand docker a /c/... path
export MSYS_NO_PATHCONV=1
host_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi; }
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
[ -f "$here/$entry" ] || bad "no such entry script: $entry"
pkg_dir=$(cd "$pkg" 2>/dev/null && pwd) || bad "no such directory: $pkg"
mkdir -p "$out" || bad "cannot create $out"
out_dir=$(cd "$out" && pwd) || bad "cannot enter $out"

[ -f "$pkg_dir/packages.sha256" ] || { echo "leg.sh: $pkg_dir/packages.sha256 is missing" >&2; exit 1; }
(cd "$pkg_dir" && sha256sum -c packages.sha256) || { echo "leg.sh: the packages do not match packages.sha256" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || { echo "leg.sh: docker is not installed" >&2; exit 1; }
delay=${SMOKE_PULL_DELAY:-10}
[[ $delay =~ ^[0-9]+$ ]] || delay=10
ref=""
for image in "$@"; do
  for try in 1 2 3; do
    if docker pull --quiet "$image" >/dev/null; then ref=$image; break 2; fi
    echo "leg.sh: could not pull $image (try $try of 3)" >&2
    if [ "$try" -lt 3 ]; then sleep "$delay"; fi
  done
done
[ -n "$ref" ] || { echo "leg.sh: none of the images could be pulled: $*" >&2; exit 1; }

# the repo digest of the registry the image came from (a local image can carry several)
name=${ref%@*}
[[ ${name##*/} == *:* ]] && name=${name%:*}
digests=$(docker image inspect "$ref" --format '{{range .RepoDigests}}{{println .}}{{end}}' 2>/dev/null)
digest=$(grep -F -m 1 "$name@" <<<"$digests")
[ -n "$digest" ] || digest=$(head -n 1 <<<"$digests")
printf 'image=%s\ndigest=%s\n' "$ref" "${digest:-unknown}" >"$out_dir/image.txt"
echo "leg.sh: $slug runs on $ref (${digest:-no repo digest})" >&2

docker run --rm --name "smoke-$slug-$$" --shm-size=512m \
  -v "$(host_path "$pkg_dir"):/pkg:ro" -v "$(host_path "$here"):/scripts:ro" -v "$(host_path "$out_dir"):/out" \
  -e "EXPECTED_VERSION=$version" -e "LEG=$slug" -e SMOKE_LIBC_FLOOR -e SMOKE_SELFTEST_ONLY \
  "$ref" bash "/scripts/$entry" "$kind" 2>&1 | tee "$out_dir/leg.log"
exit "${PIPESTATUS[0]}"
